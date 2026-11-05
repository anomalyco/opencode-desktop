//! Side region per-session state, resolved strip projection, keyboard shortcuts, and persistence.

use std::collections::{HashMap, HashSet};

use rune::Tx;

use super::tabs::{
    ClosedSideTab, SessionTabState, SessionTabs, close_session_tab, move_session_tab,
    open_session_tab, panel_key, preview_session_tab, push_closed_tab, session_pane_tab,
    session_tab_pane, split_panel_key, take_closed_tab,
};
use super::{
    ClosedTabInfo, DockPlacement, PanelLauncherDesc, PanelRegion,
    PanelRegistration, PanelSessionCtx, PanelTabDesc, SessionPane, find_panel, registered_panels,
};
use crate::{App, Route};

pub const SIDE_DEFAULT_W: f32 = 460.0;
pub const SIDE_WIDE_W: f32 = 620.0;
pub const SIDE_MIN_W: f32 = 280.0;
pub const SIDE_MAX_W: f32 = 960.0;
pub const SESSION_MIN_W: f32 = 380.0;
pub const SPLIT_MIN_H: f32 = 120.0;
pub const DOCK_DEFAULT_H: f32 = 260.0;
pub const DOCK_MIN_H: f32 = 100.0;
pub const DOCK_COLLAPSE_H: f32 = 50.0;
pub const DOCK_MAX_H: f32 = 560.0;
pub const BOTTOM_DEFAULT_H: f32 = 220.0;
pub const BOTTOM_MIN_H: f32 = 120.0;
pub const BOTTOM_MAX_H: f32 = 520.0;

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSideState {
    pub open: bool,
    pub dock_open: bool,
    pub bottom_open: bool,
    pub tab_state: SessionTabState,
    pub closed: Vec<ClosedSideTab>,
}

impl Default for SessionSideState {
    fn default() -> Self {
        Self {
            open: false,
            dock_open: false,
            bottom_open: true,
            tab_state: SessionTabState::default(),
            closed: Vec::new(),
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
    pub width: f32,
    pub resized: bool,
    pub drag_origin: Option<f32>,
    pub split_ratio: f32,
    pub split_drag_origin: Option<f32>,
    pub dock_height: f32,
    pub dock_drag_origin: Option<f32>,
    pub bottom_height: f32,
    pub bottom_drag_origin: Option<f32>,
    /// Once shown, the side region keeps its DOM/Rune subtree mounted while closed so springs
    /// never mount or unmount mid-flight (`side-region.tsx` `contentMounted`).
    pub mounted: bool,
    pub dock_mounted: bool,
    pub dock_last_solo: bool,
    /// Keyboard focus belongs to the side/bottom region rather than the session chat.
    pub focused: bool,
    pub dock_focused: bool,
    pub active_pane_focus: SessionPane,
    /// Blocks an extra `⌘W` right after closing the side region's last tab from closing the session.
    pub emptied_guard: bool,
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
            width: SIDE_DEFAULT_W,
            resized: false,
            drag_origin: None,
            split_ratio: 0.55,
            split_drag_origin: None,
            dock_height: DOCK_DEFAULT_H,
            dock_drag_origin: None,
            bottom_height: BOTTOM_DEFAULT_H,
            bottom_drag_origin: None,
            mounted: false,
            dock_mounted: false,
            dock_last_solo: false,
            focused: false,
            dock_focused: false,
            active_pane_focus: SessionPane::Side,
            emptied_guard: false,
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
    pub launchers: Vec<(&'static str, PanelLauncherDesc, fn(&mut App, SessionPane, &mut Tx))>,
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
    pub fn all_panels(&self) -> Vec<PanelRegistration> {
        #[allow(unused_mut)]
        let mut out = registered_panels().to_vec();
        #[cfg(test)]
        if !self.side.extra_panels.is_empty() {
            out.retain(|p| !self.side.extra_panels.iter().any(|e| e.plugin == p.plugin));
            out.extend(self.side.extra_panels.iter().copied());
        }
        out.sort_by_key(|p| (p.order, p.plugin));
        out
    }

    pub fn active_session_id(&self) -> Option<&str> {
        match &self.route {
            Route::Session(id) => Some(id.as_str()),
            _ => None,
        }
    }

    pub fn side_open(&self) -> bool {
        self.active_session_id()
            .and_then(|id| self.side.sessions.get(id))
            .is_some_and(|st| st.open)
    }

    pub fn has_dock_provider(&self) -> bool {
        self.all_panels().iter().any(|p| p.region == PanelRegion::Dock)
    }

    pub fn dock_placement(&self) -> DockPlacement {
        self.settings.terminal_placement
    }

    pub fn dock_open(&self) -> bool {
        self.has_dock_provider()
            && self
                .active_session_id()
                .and_then(|id| self.side.sessions.get(id))
                .is_some_and(|st| st.dock_open)
    }

    pub fn dock_side_open(&self) -> bool {
        self.dock_open() && self.dock_placement() == DockPlacement::Side
    }

    pub fn dock_bottom_open(&self) -> bool {
        self.dock_open() && self.dock_placement() == DockPlacement::Bottom
    }

    /// True when the right-hand column (`side-region`) is visible (`tabsOpen || dockSideOpen` in `screen-layout.ts`).
    pub fn right_column_open(&self) -> bool {
        self.side_open() || self.dock_side_open()
    }

    pub fn bottom_open(&self) -> bool {
        self.active_session_id()
            .and_then(|id| self.side.sessions.get(id))
            .is_some_and(|st| st.bottom_open)
    }

    pub fn resolve_dock_entry(&self) -> Option<(String, String, String, ResolvedEntry)> {
        let session_id = self.active_session_id()?.to_owned();
        let (directory, project_id) = match self.session(&session_id) {
            Some(info) => (info.directory.clone(), info.project.clone()),
            None => (String::new(), String::new()),
        };
        for p in self.all_panels().into_iter().filter(|p| p.region == PanelRegion::Dock) {
            let ctx = PanelSessionCtx {
                session_id: &session_id,
                directory: &directory,
                project_id: &project_id,
                open_ids: &[],
            };
            if let Some(tab) = (p.list)(self, &ctx).into_iter().next() {
                let key = panel_key(p.plugin, &tab.id);
                let entry = ResolvedEntry {
                    key,
                    plugin: p.plugin,
                    tab,
                    provider: p,
                    pane: SessionPane::Side,
                };
                return Some((session_id, directory, project_id, entry));
            }
        }
        None
    }

    pub fn open_dock(&mut self, _tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.side.mounted = true;
        self.side.dock_mounted = true;
        self.side.dock_last_solo = !self.side_open();
        self.session_side_mut(&sid).dock_open = true;
        self.side.dock_focused = true;
        self.side.focused = false;
        self.composer.focused = false;
        if let Some((_, _, _, entry)) = self.resolve_dock_entry()
            && let Some(on_enter) = entry.provider.on_enter
        {
            on_enter(self, &sid, &entry.tab.id);
        }
    }

    pub fn close_dock(&mut self, _tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.side.dock_last_solo = !self.side_open();
        self.session_side_mut(&sid).dock_open = false;
        self.side.dock_focused = false;
    }

    pub fn toggle_dock(&mut self, tx: &mut Tx) {
        if self.dock_open() {
            self.close_dock(tx);
        } else {
            self.open_dock(tx);
        }
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
                session_id: &session_id,
                directory: &directory,
                project_id: &project_id,
                open_ids: &open_ids,
            };
            if let Some(launcher_fn) = p.launcher
                && let Some(open_fn) = p.open_launcher
                && let Some(desc) = launcher_fn(self, &ctx)
            {
                launchers.push((p.plugin, desc, open_fn));
            }
            if p.region != PanelRegion::Side {
                continue;
            }
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
        let wide = active
            .as_deref()
            .and_then(|k| strip.iter().find(|e| e.key == k))
            .is_some_and(|e| (e.provider.wide)(self));

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
        self.side.dock_focused = false;
        self.side.dock_last_solo = false;
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
        let was_open = self.side_open();
        if was_open {
            self.side.dock_last_solo = false;
            self.session_side_mut(&sid).open = false;
            self.side.focused = false;
            self.browser.address.focused = false;
            if self.dock_open() {
                self.side.dock_focused = true;
            }
            return;
        }

        self.side.mounted = true;
        self.side.dock_last_solo = false;
        self.session_side_mut(&sid).open = true;
        self.side.focused = true;
        self.side.dock_focused = false;
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
            && let Some(on_enter) = entry.provider.on_enter
        {
            on_enter(self, &sid, &entry.tab.id);
        }
    }

    pub fn toggle_bottom_region(&mut self, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        tx.instant();
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
        self.side.dock_focused = false;
        self.side.dock_last_solo = false;
        self.side.active_pane_focus = actual_pane;

        if let Some((plugin, tab_id)) = split_panel_key(key)
            && let Some(prov) = self.all_panels().into_iter().find(|p| p.plugin == plugin)
            && let Some(on_focus) = prov.on_focus_tab
        {
            on_focus(self, session_id, tab_id, false);
        }
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
        if !key.starts_with("browser:") {
            self.browser.address.focused = false;
        }
        if key != crate::review::REVIEW_KEY {
            self.review.filter.focused = false;
        }
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
        self.side.dock_focused = false;
        self.side.active_pane_focus = pane;

        if let Some((plugin, tab_id)) = split_panel_key(key)
            && let Some(prov) = self.all_panels().into_iter().find(|p| p.plugin == plugin)
            && let Some(on_focus) = prov.on_focus_tab
        {
            on_focus(self, &sid, tab_id, false);
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
            let info = e
                .provider
                .closed_tab
                .map_or(ClosedTabInfo::Default, |f| f(self, &sid, &e.tab));
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

        if let Some(e) = &entry
            && let Some(on_close) = e.provider.on_close_tab
        {
            on_close(self, &sid, &e.tab.id, tx);
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
                // Closing the last tab collapses the side region smoothly on its spring.
                self.session_side_mut(&sid).open = false;
                self.side.focused = false;
            } else {
                // Remaining tabs close the gap at once (docs/kit-design.md: immediate).
                tx.instant();
                if let Some(r) = &next_reg
                    && let Some(act_key) = &r.active
                    && let Some((pl, tid)) = split_panel_key(act_key)
                    && let Some(prov) = self.all_panels().into_iter().find(|p| p.plugin == pl)
                    && let Some(on_focus) = prov.on_focus_tab
                {
                    on_focus(self, &sid, tid, false);
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
            && let Some(prov) = self.all_panels().into_iter().find(|p| p.plugin == plugin)
            && let Some(restore) = prov.on_restore_tab
        {
            restore(self, &sid, tab_id, closed.data.as_deref(), tx);
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
