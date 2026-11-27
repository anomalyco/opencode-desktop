//! The browser's part of every poll (`App::poll_browser`): the engine's messages, the tabs it
//! closed, popups and app-link reviews, multi-pane visibility sync, and the settings it reloads.

use std::time::Duration;

use rune::Tx;
use rune::time::Instant;

use super::PLUGIN;
use crate::App;
use crate::panels::{SESSION_PANES, SessionPane, panel_key, split_panel_key};

/// Grace window matching `features/panels/src/view/mod.rs::CLOSE_FADE` (400 ms) so closing
/// the side or bottom pane keeps its retained frame visible while sliding out instead of
/// flashing blank on `surface.clear()`.
const CLOSE_SLIDE_GRACE: Duration = Duration::from_millis(420);

/// Explicit OS window visibility state for throttling browser tabs when occluded or minimized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowVisState {
    #[default]
    Visible,
    Occluded,
    Minimized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowVisEvent {
    OsVisibility { shown: bool },
    Placement { minimized: bool },
}

impl WindowVisState {
    pub fn is_hidden(self) -> bool {
        !matches!(self, Self::Visible)
    }

    pub fn step(&mut self, event: WindowVisEvent) {
        *self = match (*self, event) {
            (_, WindowVisEvent::Placement { minimized: true }) => Self::Minimized,
            (Self::Minimized, WindowVisEvent::Placement { minimized: false }) => Self::Visible,
            (state, WindowVisEvent::Placement { minimized: false }) => state,
            (_, WindowVisEvent::OsVisibility { shown: false }) => Self::Occluded,
            (_, WindowVisEvent::OsVisibility { shown: true }) => Self::Visible,
        };
    }
}

/// Explicit close-slide grace state machine for smooth 400 ms side/bottom pane close animation.
#[derive(Clone, Debug, Default)]
pub enum CloseSlide {
    #[default]
    Idle,
    Sliding {
        session_id: String,
        tabs: Vec<u64>,
        until: Instant,
    },
}

#[derive(Clone, Debug)]
struct CloseSlideInput<'a> {
    now: Instant,
    hard_hidden: bool,
    session_id: Option<&'a str>,
    current_visible: &'a [u64],
    prev_session: Option<&'a str>,
    prev_side_open: bool,
    prev_bottom_open: bool,
    side_open: bool,
    bottom_open: bool,
    prev_visible: &'a [u64],
}

impl CloseSlide {
    fn step(&mut self, input: CloseSlideInput<'_>) {
        if input.hard_hidden {
            *self = Self::Idle;
            return;
        }
        let Some(sid) = input.session_id else {
            *self = Self::Idle;
            return;
        };
        let same_session = input.prev_session == Some(sid);
        let pane_closed = same_session
            && ((input.prev_side_open && !input.side_open)
                || (input.prev_bottom_open && !input.bottom_open));
        if pane_closed {
            let dropped: Vec<u64> = input
                .prev_visible
                .iter()
                .copied()
                .filter(|id| !input.current_visible.contains(id))
                .collect();
            if !dropped.is_empty() {
                *self = Self::Sliding {
                    session_id: sid.to_owned(),
                    tabs: dropped,
                    until: input.now + CLOSE_SLIDE_GRACE,
                };
                return;
            }
        }
        match self {
            Self::Idle => {}
            Self::Sliding { session_id, tabs, until } => {
                if session_id != sid || input.now >= *until {
                    *self = Self::Idle;
                } else {
                    tabs.retain(|id| !input.current_visible.contains(id));
                    if tabs.is_empty() {
                        *self = Self::Idle;
                    }
                }
            }
        }
    }

    fn wake_at(&self) -> Option<Instant> {
        match self {
            Self::Idle => None,
            Self::Sliding { until, .. } => Some(*until),
        }
    }
}

/// Per-window browser pane visibility tracker (stored on `BrowserPane`, zero fields on `App`).
#[derive(Clone, Debug, Default)]
pub struct BrowserVisibilityState {
    pub window: WindowVisState,
    pub slide: CloseSlide,
    prev_session: Option<String>,
    prev_side_open: bool,
    prev_bottom_open: bool,
    prev_visible: Vec<u64>,
}

impl App {
    /// Exact set of logically visible browser tabs across `Side`, `Split`, and `Bottom` panes
    /// (empty when on Home, Settings, narrow layout, hidden/minimized window, or covered by New Tab).
    pub fn visible_browser_tabs(&self) -> Vec<u64> {
        if self.window.hidden
            || self.workspace_panes.browser.visibility.window.is_hidden()
            || self.settings_open()
            || self.narrow()
        {
            return Vec::new();
        }
        let Some(sid) = self.panel_session_id() else {
            return Vec::new();
        };
        let side = self.kernel.slots.get(&crate::panels::SIDE);
        let Some(st) = side.sessions.get(sid) else {
            return Vec::new();
        };
        let side_is_open = self.side_open();
        let bottom_is_open = self.bottom_open();
        let tabs = &st.tab_state.tabs;
        let mut visible = Vec::new();
        for pane in SESSION_PANES {
            let pane_open = match pane {
                SessionPane::Side | SessionPane::Split => side_is_open,
                SessionPane::Bottom => bottom_is_open,
            };
            if !pane_open {
                continue;
            }
            let Some(key) = crate::panels::tabs::session_pane_tab(tabs, pane) else {
                continue;
            };
            if !tabs.all.iter().any(|k| k == key) {
                continue;
            }
            if self.shown_new_tab(sid, pane, Some(key)).is_some() {
                continue;
            }
            if let Some((plugin, tid)) = split_panel_key(key)
                && plugin == PLUGIN
                && let Ok(id) = tid.parse::<u64>()
                && self.workspace_panes.browser.host.tab(id).is_some()
                && !visible.contains(&id)
            {
                visible.push(id);
            }
        }
        visible
    }

    /// Computes effective visible tabs (honoring the 400 ms close-slide animation) and updates
    /// `BrowserHost::set_visible_tabs` (deduplicated; no-op when unchanged).
    pub fn sync_visible_browser_tabs(&mut self) {
        let hard_hidden = self.window.hidden
            || self.workspace_panes.browser.visibility.window.is_hidden()
            || self.settings_open()
            || self.narrow();
        let session_id = self.panel_session_id().map(str::to_owned);
        let side_open = self.side_open();
        let bottom_open = self.bottom_open();
        let current = self.visible_browser_tabs();

        // Fast read-only exit: avoid dirtying `Tracked<BrowserPane>` when nothing changed.
        {
            let vis = &self.workspace_panes.browser.visibility;
            let side_closing = self.kernel.slots.get(&crate::panels::SIDE).closing.session().is_some();
            if matches!(vis.slide, CloseSlide::Idle)
                && !side_closing
                && vis.prev_session.as_deref() == session_id.as_deref()
                && vis.prev_side_open == side_open
                && vis.prev_bottom_open == bottom_open
                && vis.prev_visible == current
                && self.workspace_panes.browser.host.visible_tabs() == current.as_slice()
            {
                return;
            }
        }

        let now = Instant::now();
        let mut effective = current.clone();
        {
            let browser = &mut *self.workspace_panes.browser;
            let vis = &mut browser.visibility;
            let host = &browser.host;
            vis.slide.step(CloseSlideInput {
                now,
                hard_hidden,
                session_id: session_id.as_deref(),
                current_visible: &current,
                prev_session: vis.prev_session.as_deref(),
                prev_side_open: vis.prev_side_open,
                prev_bottom_open: vis.prev_bottom_open,
                side_open,
                bottom_open,
                prev_visible: &vis.prev_visible,
            });
            if let CloseSlide::Sliding { tabs, .. } = &vis.slide {
                for &id in tabs {
                    if host.tab(id).is_some() && !effective.contains(&id) {
                        effective.push(id);
                    }
                }
            }
            vis.prev_session = session_id;
            vis.prev_side_open = side_open;
            vis.prev_bottom_open = bottom_open;
            vis.prev_visible = current;
        }
        // Also keep any live tab held by `side.closing` visible while its region slides shut.
        if !hard_hidden && let Some(sid) = self.panel_session_id() {
            let side = self.kernel.slots.get(&crate::panels::SIDE);
            for pane in SESSION_PANES {
                if let Some(entry) = side.closing.held(sid, pane)
                    && entry.plugin == PLUGIN
                    && let Ok(id) = entry.tab.id.parse::<u64>()
                    && self.workspace_panes.browser.host.tab(id).is_some()
                    && !effective.contains(&id)
                {
                    effective.push(id);
                }
            }
        }
        self.workspace_panes.browser.host.set_visible_tabs(&effective);
    }

    /// Next deadline at which a close-slide visibility grace expires.
    pub fn browser_visibility_wake(&self) -> Option<Instant> {
        self.workspace_panes.browser.visibility.slide.wake_at()
    }

    /// Handles Rune's OS window visibility hook (`on_visibility`) and immediately updates engine
    /// visibility when the window hides or returns.
    pub fn on_browser_window_visibility(&mut self, shown: bool) {
        self.workspace_panes
            .browser
            .visibility
            .window
            .step(WindowVisEvent::OsVisibility { shown });
        self.sync_visible_browser_tabs();
    }

    /// Updates minimized state from `App::poll_window`'s throttled OS placement check.
    pub fn set_browser_window_minimized(&mut self, minimized: bool) {
        let mut next = self.workspace_panes.browser.visibility.window;
        next.step(WindowVisEvent::Placement { minimized });
        if next == self.workspace_panes.browser.visibility.window {
            return;
        }
        self.workspace_panes.browser.visibility.window = next;
        self.sync_visible_browser_tabs();
    }

    /// The engine's messages and the page the engine paints: the
    /// routed session's selected one. True when what the pane shows changed.
    pub fn poll_browser(&mut self) -> bool {
        // Every poll comes here: the pane is written only when the engine has something.
        let mut changed = !self.workspace_panes.browser.host.quiet() && self.workspace_panes.browser.host.poll();
        changed |= self.pump_preview_fetches();
        changed |= self.pump_cookie_import();
        // Another browser's icon or the browsers' list, read off this thread, is in.
        changed |= self.workspace_panes.browser.app_icons.take_landed();
        let fallback_sources = self.workspace_panes.browser.host.drain_unsupported_local_previews();
        for (tab_id, doc) in fallback_sources {
            // Non-agent local HTML clicks on an older engine (`!Ready.preview`) open in the
            // Files code viewer ("show source", never `file://`). Agent tabs stay refused.
            let is_agent = self
                .workspace_panes
                .browser
                .host
                .tab(tab_id)
                .is_some_and(|t| t.prompts.agent_driven);
            if !is_agent
                && let Some(sid) = self.browser_tab_session(tab_id).map(str::to_owned)
            {
                self.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                    s.close_browser_tab_for_session(&sid, tab_id, tx);
                    s.open_workspace_file_source(&sid, &doc, false, None, true, tx);
                }));
                changed = true;
            }
        }
        // Prune `session_tabs` and side-panel strip entries for any tab closed by the engine,
        // surfacing a plain-language toast only for fatal proxy/context teardown (never `DropContext`).
        let engine_closed: Vec<(u64, bool)> =
            self.workspace_panes.browser.host.drain_closed_tabs().collect();
        for (closed_id, fatal_close) in engine_closed {
            self.workspace_panes.browser.closing_tabs.remove(&closed_id);
            self.workspace_panes.browser.page_opened_tabs.remove(&closed_id);
            self.workspace_panes.browser.active_app_reviews.borrow_mut().remove(&closed_id);
            self.revoke_preview_binding(closed_id);
            let owner_sid = self.browser_tab_session(closed_id).map(str::to_owned);
            for ids in self.workspace_panes.browser.session_tabs.values_mut() {
                ids.retain(|&x| x != closed_id);
            }
            if let Some(sid) = owner_sid {
                let key = panel_key(PLUGIN, &closed_id.to_string());
                let st = self.session_side_mut(&sid);
                st.tab_state = crate::panels::tabs::close_session_tab(&st.tab_state, &key);
                if st.tab_state.tabs.all.is_empty() {
                    st.visibility.step(false);
                }
                changed = true;
            }
            if fatal_close {
                crate::status_toast::fail(
                    self,
                    opencode_i18n::t!("rune.browser.proxy.closed").to_owned(),
                    String::new(),
                );
                changed = true;
            }
        }
        changed |= self.drain_browser_popups();
        changed |= self.poll_app_link_reviews();
        changed |= self.close_download_only_tabs();
        self.copy_browser_images();
        // Reload persisted site permissions when Settings > Browser edits `rune.sitePermissions`.
        let perm_ver = crate::plugin_host::services::store::store_version("browser", "rune.sitePermissions");
        if perm_ver != self.workspace_panes.browser.site_permissions_version {
            self.workspace_panes.browser.site_permissions_version = perm_ver;
            if self.persists() {
                self.workspace_panes.browser.host.remember_site_permissions(browser_pane::PermissionStore {
                    read: Box::new(|| crate::storage::text("plugin.browser", "rune.sitePermissions").map(|t| t.to_string())),
                    write: Box::new(|json| {
                        crate::storage::set("plugin.browser", "rune.sitePermissions", json);
                        crate::plugin_host::services::store::store_changed("browser", "rune.sitePermissions");
                    }),
                });
            }
        }
        // Keep keyboard focus on the routed session's active browser tab when visible.
        if self.side_open()
            && !self.window.hidden
            && !self.settings_open()
            && !self.narrow()
            && let Some(id) = self.active_browser_tab_id()
            && self.workspace_panes.browser.host.focused_tab_id() != Some(id)
        {
            self.workspace_panes.browser.host.focus_tab(id);
            changed = true;
        }
        // Sync the exact visible set across Side, Split, and Bottom panes (deduplicated).
        self.sync_visible_browser_tabs();
        changed |= self.prune_browser_unseen();
        changed |= self.sync_browser_openers();
        changed
    }
}
