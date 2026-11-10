//! Side region per-session state, resolved strip projection, keyboard shortcuts, and persistence.

use std::collections::{HashMap, HashSet};

use rune::Tx;

use super::closing::Event;
use super::tabs::{
    ClosedSideTab, SessionTabState, SessionTabs, close_session_tab, move_session_tab,
    open_session_tab, panel_key, preview_session_tab, push_closed_tab, session_pane_tab,
    session_tab_pane, split_panel_key, take_closed_tab,
};
use super::{
    ClosedTabInfo, DockPlacement, PanelLauncherDesc,
    PanelRegistration, PanelSessionCtx, PanelTabDesc, SessionPane, find_panel, registered_panels,
};
use crate::{App, Route};

/// `PANE_HEIGHT_MIN` (`side-region.tsx`): each pane of a split side region keeps this much.
pub const SPLIT_MIN_H: f32 = 120.0;
/// The side pane's share of a split side region until the divider is dragged.
pub const SPLIT_DEFAULT_RATIO: f32 = 0.5;
/// `DEFAULT_DOCK_HEIGHT` (`shell/state/layout.tsx`): the bottom pane's height until dragged.
pub const BOTTOM_DEFAULT_H: f32 = 280.0;
/// `BOTTOM_PANE_HEIGHT_MIN` (`session/screen.tsx`).
pub const BOTTOM_MIN_H: f32 = 120.0;

/// One session's side region, as Desktop keeps it per shell tab (`TabRegions`): whether its
/// panes show, its strip, and the sizes the user dragged (none until they do).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSideState {
    pub open: bool,
    pub bottom_open: bool,
    pub tab_state: SessionTabState,
    pub closed: Vec<ClosedSideTab>,
    /// The chat panel's width beside the side region (`sessionWidth`).
    pub session_width: Option<f32>,
    /// The side pane's share of the side region's height while a pane splits below it.
    pub split_ratio: Option<f32>,
    /// The bottom pane's height.
    pub bottom_height: Option<f32>,
}

impl Default for SessionSideState {
    fn default() -> Self {
        Self {
            open: false,
            bottom_open: true,
            tab_state: SessionTabState::default(),
            closed: Vec::new(),
            session_width: None,
            split_ratio: None,
            bottom_height: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewTabEntry {
    pub id: String,
    pub pane: SessionPane,
    pub session_id: String,
    pub over: Option<String>,
}

pub struct SideState {
    /// The chat panel's width as a divider drag began, and what the session stored then.
    pub drag_origin: Option<(f32, Option<f32>)>,
    /// The side pane's share as a split divider drag began, and what the session stored then.
    pub split_drag_origin: Option<(f32, Option<f32>)>,
    /// The bottom pane's height as a drag began, and what the session stored then.
    pub bottom_drag_origin: Option<(f32, Option<f32>)>,
    /// The window's width at the last layout, for sizes estimated outside the view.
    pub laid_w: std::cell::Cell<f32>,
    /// Until when (`App::clock`) the left sidebar's flight moves the side region's edge, so its
    /// contents glide with the edge rather than land at once.
    pub edge_flight: Option<f64>,
    /// A tab dragged between panes.
    pub pane_drag: super::drag::PaneDrag,
    /// Once shown, the side region keeps its DOM/Rune subtree mounted while closed so springs
    /// never mount or unmount mid-flight (`side-region.tsx` `contentMounted`).
    pub mounted: bool,
    /// Keyboard focus belongs to the side/bottom region rather than the session chat.
    pub focused: bool,
    pub active_pane_focus: SessionPane,
    /// Blocks an extra `⌘W` right after closing the side region's last tab from closing the session.
    pub emptied_guard: bool,
    /// The side region's last tab, still drawn while closing it slides the region shut.
    pub closing: super::closing::State,
    /// Right-click context menu on a side tab: `(tab_key, x, y)`.
    pub tab_menu: Option<(String, f32, f32)>,
    /// `+` launcher dropdown menu anchor `(pane, x, y)`.
    pub add_menu: Option<(SessionPane, f32, f32)>,
    pub sessions: HashMap<String, SessionSideState>,
    pub adding: Vec<NewTabEntry>,
    pub chosen_new_tab: HashMap<SessionPane, String>,
    next_new_serial: u64,
    #[cfg(test)]
    pub extra_panels: Vec<PanelRegistration>,
}

impl Default for SideState {
    fn default() -> Self {
        Self {
            drag_origin: None,
            split_drag_origin: None,
            bottom_drag_origin: None,
            laid_w: std::cell::Cell::new(1280.0),
            edge_flight: None,
            pane_drag: Default::default(),
            mounted: false,
            focused: false,
            active_pane_focus: SessionPane::Side,
            emptied_guard: false,
            closing: Default::default(),
            tab_menu: None,
            add_menu: None,
            sessions: HashMap::new(),
            adding: Vec::new(),
            chosen_new_tab: HashMap::new(),
            next_new_serial: 1,
            #[cfg(test)]
            extra_panels: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct ResolvedEntry {
    pub key: String,
    pub plugin: &'static str,
    pub tab: PanelTabDesc,
    pub provider: PanelRegistration,
    pub pane: SessionPane,
}

pub struct ResolvedRegion {
    pub session_id: String,
    pub directory: String,
    pub project_id: String,
    pub entries: Vec<ResolvedEntry>,
    pub strip: Vec<ResolvedEntry>,
    pub active: Option<String>,
    pub active_pane: SessionPane,
    pub wide: bool,
    pub launchers: Vec<(&'static str, PanelLauncherDesc, super::Launch)>,
}

impl ResolvedRegion {
    pub fn pane_entries(&self, pane: SessionPane) -> Vec<&ResolvedEntry> {
        self.strip.iter().filter(|e| e.pane == pane && !e.tab.hidden).collect()
    }

    pub fn pane_active(&self, pane: SessionPane, stored: &SessionTabs) -> Option<String> {
        let members: Vec<&ResolvedEntry> = self.strip.iter().filter(|e| e.pane == pane).collect();
        if let Some(act) = self.active.as_deref()
            && members.iter().any(|e| e.key == act)
        {
            return Some(act.to_owned());
        }
        if let Some(shown) = stored.selected.get(&pane)
            && members.iter().any(|e| e.key == *shown)
        {
            return Some(shown.clone());
        }
        if pane != SessionPane::Side {
            return members.first().map(|e| e.key.clone());
        }
        let eligible: Vec<&&ResolvedEntry> = members.iter().filter(|e| e.tab.fallback).collect();
        eligible
            .iter()
            .find(|e| !e.tab.pinned && !e.tab.first)
            .or_else(|| eligible.iter().find(|e| !e.tab.pinned && e.tab.first))
            .or_else(|| eligible.iter().find(|e| e.tab.pinned))
            .map(|e| e.key.clone())
            .or_else(|| members.first().map(|e| e.key.clone()))
    }

    pub fn entry(&self, key: &str) -> Option<&ResolvedEntry> {
        self.entries.iter().find(|e| e.key == key)
    }
}

impl App {
    /// The panel providers: the app's own, and each running Luau plugin that adds panels.
    pub fn all_panels(&self) -> Vec<PanelRegistration> {
        let mut out = registered_panels().to_vec();
        out.extend(crate::plugin_host::luau::panels::providers(self));
        #[cfg(test)]
        if !self.side.extra_panels.is_empty() {
            out.retain(|p| !self.side.extra_panels.iter().any(|e| e.plugin == p.plugin));
            out.extend(self.side.extra_panels.iter().copied());
        }
        out.sort_by_key(|p| (p.order, p.plugin));
        out
    }

    /// The provider of namespace `plugin`.
    pub fn find_panel(&self, plugin: &str) -> Option<PanelRegistration> {
        self.all_panels().into_iter().find(|p| p.plugin == plugin)
    }

    pub fn active_session_id(&self) -> Option<&str> {
        match &self.route {
            Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }

    /// The side region shows: open for the routed session, in a wide window (a narrow one folds
    /// it into its views and keeps it open for when the window widens, as Desktop's `sideVisible`).
    pub fn side_open(&self) -> bool {
        self.side_stored_open() && !self.narrow()
    }

    /// The routed session's side region is open, shown or folded away by a narrow window.
    pub fn side_stored_open(&self) -> bool {
        self.active_session_id()
            .and_then(|id| self.side.sessions.get(id))
            .is_some_and(|st| st.open)
    }

    pub fn dock_placement(&self) -> DockPlacement {
        self.settings.terminal_placement
    }

    /// The bottom pane shows: open for the routed session, in a wide window (`screen.tsx:545`).
    pub fn bottom_open(&self) -> bool {
        self.bottom_stored_open() && !self.narrow()
    }

    pub fn bottom_stored_open(&self) -> bool {
        self.active_session_id()
            .and_then(|id| self.side.sessions.get(id))
            .is_some_and(|st| st.bottom_open)
    }

    /// Lands a tab shown with `show` at once when its pane was already on screen, as switching
    /// tabs does; a pane it opens flies on `SIDE_SPRING`, as the region's toggle does.
    pub fn show_pane_tab(&mut self, tx: &mut Tx, show: impl FnOnce(&mut Self)) {
        let was = self.panes_shown();
        show(self);
        if self.panes_shown() == was {
            tx.instant();
        }
    }

    /// Whether the side region and the bottom pane are on screen. The bottom pane can be stored
    /// open with no tab in it, which draws nothing: its first tab opens it.
    fn panes_shown(&self) -> (bool, bool) {
        let bottom = self.bottom_stored_open()
            && self.resolve_side_region().is_some_and(|r| !r.pane_entries(SessionPane::Bottom).is_empty());
        (self.side_stored_open(), bottom)
    }

    pub fn session_side(&self, session_id: &str) -> Option<&SessionSideState> {
        self.side.sessions.get(session_id)
    }

    pub fn session_side_mut(&mut self, session_id: &str) -> &mut SessionSideState {
        self.side.sessions.entry(session_id.to_owned()).or_default()
    }

    pub fn resolve_side_region(&self) -> Option<ResolvedRegion> {
        let session_id = self.active_session_id()?.to_owned();
        let (directory, project_id) = match self.session(&session_id) {
            Some(info) => (info.directory.clone(), info.project.clone()),
            None => (String::new(), String::new()),
        };
        let empty_tabs = SessionTabs::default();
        let stored = self
            .side
            .sessions
            .get(&session_id)
            .map_or(&empty_tabs, |s| &s.tab_state.tabs);

        let providers = self.all_panels();
        let mut entries = Vec::new();
        let mut launchers = Vec::new();

        for p in providers {
            let prefix = format!("{}:", p.plugin);
            let open_ids: Vec<String> = stored
                .all
                .iter()
                .filter_map(|k| k.strip_prefix(&prefix).map(str::to_owned))
                .collect();
            let ctx = PanelSessionCtx {
                plugin: p.plugin,
                session_id: &session_id,
                directory: &directory,
                project_id: &project_id,
                open_ids: &open_ids,
            };
            launchers.extend(p.launcher_entries(self, &ctx).into_iter().map(|(desc, launch)| (p.plugin, desc, launch)));
            for tab in (p.list)(self, &ctx) {
                let key = panel_key(p.plugin, &tab.id);
                let pane = if tab.pinned {
                    SessionPane::Side
                } else if stored.all.contains(&key) {
                    session_tab_pane(stored, &key)
                } else if let Some(sib) = stored.all.iter().rev().find(|k| k.starts_with(&prefix)) {
                    session_tab_pane(stored, sib)
                } else {
                    p.default_pane
                };
                entries.push(ResolvedEntry { key, plugin: p.plugin, tab, provider: p, pane });
            }
        }
        launchers.sort_by_key(|(_, d, _)| d.order);

        // Strip order: pinned first, then stored `first` tabs, then regular stored tabs.
        let mut strip: Vec<ResolvedEntry> =
            entries.iter().filter(|e| e.tab.pinned).cloned().collect();
        let mut first_stored = Vec::new();
        let mut rest_stored = Vec::new();
        for key in &stored.all {
            if let Some(entry) = entries.iter().find(|e| e.key == *key && !e.tab.pinned) {
                if entry.tab.first {
                    first_stored.push(entry.clone());
                } else {
                    rest_stored.push(entry.clone());
                }
            }
        }
        strip.extend(first_stored);
        strip.extend(rest_stored);

        let side_strip: Vec<&ResolvedEntry> =
            strip.iter().filter(|e| e.pane != SessionPane::Bottom).collect();
        let active = if let Some(act) = stored.active.as_deref()
            && strip.iter().any(|e| e.key == act)
        {
            Some(act.to_owned())
        } else {
            let eligible: Vec<&&ResolvedEntry> =
                side_strip.iter().filter(|e| e.pane == SessionPane::Side && e.tab.fallback).collect();
            eligible
                .iter()
                .find(|e| !e.tab.pinned && !e.tab.first)
                .or_else(|| eligible.iter().find(|e| !e.tab.pinned && e.tab.first))
                .or_else(|| eligible.iter().find(|e| e.tab.pinned))
                .map(|e| e.key.clone())
                .or_else(|| side_strip.first().map(|e| e.key.clone()))
        };

        let active_pane = active
            .as_deref()
            .and_then(|k| strip.iter().find(|e| e.key == k))
            .map_or(SessionPane::Side, |e| e.pane);
        let wide = self.side_wide();

        Some(ResolvedRegion {
            session_id,
            directory,
            project_id,
            entries,
            strip,
            active,
            active_pane,
            wide,
            launchers,
        })
    }

    pub fn shown_new_tab(&self, session_id: &str, pane: SessionPane, pane_active: Option<&str>) -> Option<&NewTabEntry> {
        let live: Vec<&NewTabEntry> = self
            .side
            .adding
            .iter()
            .filter(|n| n.session_id == session_id && n.pane == pane && n.over.as_deref() == pane_active)
            .collect();
        if let Some(chosen) = self.side.chosen_new_tab.get(&pane)
            && let Some(found) = live.iter().copied().find(|n| n.id == *chosen)
        {
            return Some(found);
        }
        live.last().copied()
    }

    pub fn live_new_tabs<'a>(
        &'a self,
        session_id: &str,
        pane: SessionPane,
        pane_active: Option<&str>,
    ) -> Vec<&'a NewTabEntry> {
        self.side
            .adding
            .iter()
            .filter(|n| n.session_id == session_id && n.pane == pane && n.over.as_deref() == pane_active)
            .collect()
    }

    pub fn open_side_new_tab(&mut self, pane: SessionPane) -> String {
        let Some(sid) = self.active_session_id().map(str::to_owned) else {
            return String::new();
        };
        let over = self.resolve_side_region().and_then(|reg| {
            let empty = SessionTabs::default();
            let stored = self.side.sessions.get(&sid).map_or(&empty, |s| &s.tab_state.tabs);
            reg.pane_active(pane, stored)
        });
        let id = format!("new:{}", self.side.next_new_serial);
        self.side.next_new_serial += 1;
        self.side.adding.push(NewTabEntry {
            id: id.clone(),
            pane,
            session_id: sid.clone(),
            over,
        });
        self.side.chosen_new_tab.insert(pane, id.clone());
        self.side.focused = true;
        self.side.active_pane_focus = pane;
        self.composer.focused = false;
        self.side.mounted = true;
        let st = self.session_side_mut(&sid);
        if pane == SessionPane::Bottom {
            st.bottom_open = true;
        } else {
            st.open = true;
        }
        id
    }

    pub fn dismiss_side_new_tab(&mut self, new_id: &str) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.side.adding.retain(|n| n.id != new_id);
        let reg = self.resolve_side_region();
        let side_strip_empty = reg
            .as_ref()
            .is_none_or(|r| r.strip.iter().all(|e| e.pane == SessionPane::Bottom));
        let any_live_new = self.side.adding.iter().any(|n| n.session_id == sid && n.pane != SessionPane::Bottom);
        if side_strip_empty && !any_live_new {
            self.session_side_mut(&sid).open = false;
            self.side.focused = false;
        }
    }

    /// Toggles the right-hand side region on its smooth spring (`⌥⌘B`, `⇧⌘R`, or header toggle).
    pub fn toggle_side_region(&mut self, _tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.side.tab_menu = None;
        self.side.add_menu = None;
        let was_open = self.side_stored_open();
        if was_open {
            self.session_side_mut(&sid).open = false;
            self.side.focused = false;
            crate::plugin_host::luau::blur_fields(self);
            return;
        }

        self.side.mounted = true;
        self.session_side_mut(&sid).open = true;
        self.side.focused = true;
        self.side.active_pane_focus = SessionPane::Side;

        let reg = self.resolve_side_region();
        let has_side_tabs = reg
            .as_ref()
            .is_some_and(|r| r.strip.iter().any(|e| e.pane != SessionPane::Bottom));
        if !has_side_tabs {
            let empty_tabs = SessionTabs::default();
            let has_new = self.shown_new_tab(&sid, SessionPane::Side, None).is_some()
                || !self.live_new_tabs(&sid, SessionPane::Side, session_pane_tab(&empty_tabs, SessionPane::Side)).is_empty();
            if !has_new {
                self.open_side_new_tab(SessionPane::Side);
            }
        } else if let Some(r) = reg
            && let Some(act_key) = &r.active
            && let Some(entry) = r.entry(act_key)
        {
            entry.provider.enter(self, &sid, &entry.tab.id);
        }
    }

    pub fn toggle_bottom_region(&mut self) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let next = {
            let st = self.session_side_mut(&sid);
            st.bottom_open = !st.bottom_open;
            st.bottom_open
        };
        self.side.focused = next;
        if next {
            self.side.active_pane_focus = SessionPane::Bottom;
        }
    }

    /// Opens or focuses `key` (`"{plugin}:{tab_id}"`) in `session_id` (defaults to active session).
    pub fn open_side_tab_for_session(
        &mut self,
        session_id: &str,
        key: &str,
        target_pane: Option<SessionPane>,
        preview: bool,
        first: bool,
    ) {
        let was = self.side.sessions.get(session_id).map_or((false, false), |st| (st.open, st.bottom_open));
        let transient_launchers: HashSet<String> = self
            .resolve_side_region()
            .map(|r| {
                r.entries
                    .into_iter()
                    .filter(|e| e.tab.transient)
                    .map(|e| e.key)
                    .collect()
            })
            .unwrap_or_default();

        let default_pane = split_panel_key(key)
            .and_then(|(pl, _)| self.all_panels().into_iter().find(|p| p.plugin == pl))
            .map(|p| p.default_pane);
        let pane_choice = target_pane.or(default_pane);

        let st = self.session_side_mut(session_id);
        st.tab_state = if preview {
            preview_session_tab(&st.tab_state, key, &transient_launchers, pane_choice)
        } else {
            open_session_tab(&st.tab_state, key, &transient_launchers, first, pane_choice)
        };
        let actual_pane = session_tab_pane(&st.tab_state.tabs, key);
        if actual_pane == SessionPane::Bottom {
            st.bottom_open = true;
        } else {
            st.open = true;
            self.side.mounted = true;
        }
        self.side.adding.retain(|n| !(n.session_id == session_id && n.pane == actual_pane));
        self.side.focused = true;
        self.side.active_pane_focus = actual_pane;

        if let Some((plugin, tab_id)) = split_panel_key(key)
            && let Some(prov) = self.find_panel(plugin)
        {
            prov.focus(self, session_id, tab_id, false);
        }
        self.place_narrow(session_id, key, was);
    }

    /// Where tab `key` is in `session`: 0 not in the strip, 1 listed, 2 its pane's shown tab
    /// with the pane hidden, 3 on screen (Desktop's `layout.state`).
    pub fn tab_state(&self, session: &str, key: &str) -> u64 {
        let Some(st) = self.side.sessions.get(session) else { return 0 };
        let tabs = &st.tab_state.tabs;
        if !tabs.all.iter().any(|k| k == key) {
            return 0;
        }
        let pane = session_tab_pane(tabs, key);
        if session_pane_tab(tabs, pane) != Some(key) {
            return 1;
        }
        let shown = match pane {
            SessionPane::Bottom => st.bottom_open,
            SessionPane::Side | SessionPane::Split => st.open,
        };
        if shown { 3 } else { 2 }
    }

    /// The tab pane `pane` of the routed session shows, if any.
    pub fn pane_shown_tab(&self, pane: SessionPane) -> Option<String> {
        let reg = self.resolve_side_region()?;
        let empty = SessionTabs::default();
        let stored = self.side.sessions.get(&reg.session_id).map_or(&empty, |s| &s.tab_state.tabs);
        reg.pane_active(pane, stored)
    }

    /// Whether `key` is its pane's shown tab and that pane is on screen (Desktop's `"visible"`).
    pub fn side_tab_visible(&self, key: &str) -> bool {
        let Some(sid) = self.active_session_id() else { return false };
        let Some(st) = self.side.sessions.get(sid) else { return false };
        let pane = session_tab_pane(&st.tab_state.tabs, key);
        let open = if pane == SessionPane::Bottom { st.bottom_open } else { st.open };
        open && self.pane_shown_tab(pane).as_deref() == Some(key)
    }

    /// Hides the pane showing `key` (Desktop's `layout.hide`): the bottom pane, or the side region.
    pub fn hide_side_tab(&mut self, key: &str, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        if session_tab_pane(&self.session_side_mut(&sid).tab_state.tabs, key) == SessionPane::Bottom {
            self.session_side_mut(&sid).bottom_open = false;
            self.side.focused = false;
            return;
        }
        if self.side_stored_open() {
            self.toggle_side_region(tx);
        }
    }

    /// Desktop's `layout.open(key, session, { tab: "select" })`: adds `key` at the strip's end
    /// when it isn't there, without replacing a preview tab, selects it, and shows its pane. A
    /// new tab lands in `target`, else beside its plugin's last tab, else in its panel's default
    /// pane.
    pub fn show_side_tab(&mut self, session_id: &str, key: &str, target: Option<SessionPane>) {
        let was = self.side.sessions.get(session_id).map_or((false, false), |st| (st.open, st.bottom_open));
        let fallback = split_panel_key(key)
            .and_then(|(pl, _)| self.all_panels().into_iter().find(|p| p.plugin == pl))
            .map(|p| p.home_pane(self));
        let st = self.session_side_mut(session_id);
        let prefix = split_panel_key(key).map(|(pl, _)| format!("{pl}:")).unwrap_or_default();
        let sibling = st.tab_state.tabs.all.iter().any(|k| k.starts_with(&prefix));
        let mut next = st.tab_state.tabs.clone();
        if !next.all.iter().any(|k| k == key) {
            next.all.push(key.to_owned());
        }
        next.active = Some(key.to_owned());
        let target = target.or(if sibling { None } else { fallback });
        st.tab_state.tabs = super::tabs::place_session_tabs(&st.tab_state.tabs, next, target);
        let pane = session_tab_pane(&st.tab_state.tabs, key);
        if pane == SessionPane::Bottom {
            st.bottom_open = true;
        } else {
            st.open = true;
            self.side.mounted = true;
        }
        self.side.adding.retain(|n| !(n.session_id == session_id && n.pane == pane));
        if let Some((plugin, tab_id)) = split_panel_key(key)
            && let Some(prov) = self.find_panel(plugin)
        {
            prov.focus(self, session_id, tab_id, false);
        }
        self.place_narrow(session_id, key, was);
    }

    pub fn open_side_tab(
        &mut self,
        key: &str,
        target_pane: Option<SessionPane>,
        preview: bool,
        first: bool,
        _tx: &mut Tx,
    ) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.open_side_tab_for_session(&sid, key, target_pane, preview, first);
    }

    pub fn select_side_tab(&mut self, key: &str, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        // Switching tabs is always instant (rules.md).
        tx.instant();
        self.side.tab_menu = None;
        // A plugin's field in the tab left behind gives up the keyboard, as the app's own do.
        crate::plugin_host::luau::blur_fields(self);
        if !key.starts_with("file:") {
            self.files.filter.focused = false;
        }
        let pane = {
            let st = self.session_side_mut(&sid);
            let p = session_tab_pane(&st.tab_state.tabs, key);
            st.tab_state.tabs.active = Some(key.to_owned());
            st.tab_state.tabs.selected.insert(p, key.to_owned());
            p
        };
        self.side.adding.retain(|n| !(n.session_id == sid && n.pane == pane));
        self.side.focused = true;
        self.side.active_pane_focus = pane;

        if let Some((plugin, tab_id)) = split_panel_key(key)
            && let Some(prov) = self.find_panel(plugin)
        {
            prov.focus(self, &sid, tab_id, false);
        }
    }

    pub fn move_side_tab(&mut self, key: &str, pane: SessionPane, before: Option<&str>, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        tx.instant();
        let st = self.session_side_mut(&sid);
        st.tab_state.tabs = move_session_tab(&st.tab_state.tabs, key, pane, before);
        if pane == SessionPane::Bottom {
            st.bottom_open = true;
        } else {
            st.open = true;
            self.side.mounted = true;
        }
    }

    pub fn promote_preview_tab(&mut self, key: &str) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let st = self.session_side_mut(&sid);
        if st.tab_state.preview.as_deref() == Some(key) {
            st.tab_state.preview = None;
        }
    }

    pub fn close_side_tab(&mut self, key: &str, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.side.tab_menu = None;
        let reg = self.resolve_side_region();
        let entry = reg.as_ref().and_then(|r| r.entry(key)).cloned();
        if entry.as_ref().is_some_and(|e| !e.tab.can_close()) {
            return;
        }

        // Capture ClosedSideTab before removing the tab so the provider can still read its URL/state.
        let closed_record = entry.as_ref().and_then(|e| {
            let info = e.provider.closed_info(self, &sid, &e.tab);
            match info {
                ClosedTabInfo::Deny => None,
                ClosedTabInfo::Default if e.provider.transient || e.tab.transient => None,
                ClosedTabInfo::Default => Some(ClosedSideTab {
                    key: key.to_owned(),
                    title: e.tab.title.clone(),
                    icon_name: None,
                    file: e.tab.file.clone(),
                    detail: None,
                    data: None,
                    closed_ms: self.now_ms,
                }),
                ClosedTabInfo::Allow(custom) => Some(ClosedSideTab {
                    key: key.to_owned(),
                    title: custom.title.unwrap_or_else(|| e.tab.title.clone()),
                    icon_name: custom.icon_name,
                    file: e.tab.file.clone(),
                    detail: custom.detail,
                    data: custom.data,
                    closed_ms: self.now_ms,
                }),
            }
        });

        let was_pane = {
            let st = self.session_side_mut(&sid);
            if let Some(rec) = closed_record {
                push_closed_tab(&mut st.closed, rec);
            }
            let p = session_tab_pane(&st.tab_state.tabs, key);
            st.tab_state = close_session_tab(&st.tab_state, key);
            p
        };

        if let Some(e) = &entry {
            e.provider.close(self, &sid, &e.tab.id, tx);
        }

        // Check whether the side/split region or bottom dock is now completely empty.
        let next_reg = self.resolve_side_region();
        if was_pane == SessionPane::Bottom {
            let any_bottom = next_reg
                .as_ref()
                .is_some_and(|r| r.strip.iter().any(|e| e.pane == SessionPane::Bottom));
            if !any_bottom {
                self.session_side_mut(&sid).bottom_open = false;
                self.side.focused = false;
            }
        } else {
            let any_side = next_reg
                .as_ref()
                .is_some_and(|r| r.strip.iter().any(|e| e.pane != SessionPane::Bottom));
            let any_new = self
                .side
                .adding
                .iter()
                .any(|n| n.session_id == sid && n.pane != SessionPane::Bottom);
            if !any_side && !any_new {
                // Closing the last tab collapses the side region smoothly on its spring, its
                // contents staying on screen until it lands.
                self.session_side_mut(&sid).open = false;
                self.side.focused = false;
                if let Some(entry) = entry {
                    let at = self.clock;
                    let effects = self.side.closing.step(Event::LastTabClosed { session_id: sid, entry, at });
                    self.run_closing(effects, tx);
                }
            } else {
                // Remaining tabs close the gap at once (docs/kit-design.md: immediate).
                tx.instant();
                if let Some(r) = &next_reg
                    && let Some(act_key) = &r.active
                    && let Some((pl, tid)) = split_panel_key(act_key)
                    && let Some(prov) = self.find_panel(pl)
                {
                    prov.focus(self, &sid, tid, false);
                }
            }
        }
    }

    pub fn reopen_closed_side_tab(&mut self, key: Option<&str>, tx: &mut Tx) -> bool {
        let Some(sid) = self.active_session_id().map(str::to_owned) else {
            return false;
        };
        let Some(closed) = take_closed_tab(&mut self.session_side_mut(&sid).closed, key) else {
            return false;
        };
        if self.side_open() {
            tx.instant();
        }
        if let Some((plugin, tab_id)) = split_panel_key(&closed.key)
            && let Some(prov) = self.find_panel(plugin)
            && prov.restore(self, &sid, tab_id, closed.data.as_deref(), tx)
        {
            return true;
        }
        self.open_side_tab_for_session(&sid, &closed.key, None, false, false);
        true
    }
}

#[allow(dead_code)]
fn _use_find_panel() {
    let _ = find_panel("browser");
}
