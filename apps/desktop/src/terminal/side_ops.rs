//! The terminal as a side panel (Desktop's terminal plugin, `renderer.tsx`): one tab per
//! terminal of the session in the side region's shared strip, grouped so the view stays mounted
//! between them; key and IME dispatch to the terminal that has the keyboard, and the server's
//! `pty.*` events.

use opencode_kit::persistent_pty::parse_persistent_pty_info;
use rune::{El, Ime, KeyEvent, Tx};
use serde_json::Value;

use super::{grid, input, workspace_key};
use crate::App;
use crate::keymap::Bind;
use crate::palette::Pal;
use crate::panels::{
    PanelLauncherDesc, PanelSessionCtx, PanelTabDesc, PanelViewCtx, SessionPane, panel_key,
};

/// Every terminal tab's key starts so: `terminal:{pty id}`.
pub(crate) const PLUGIN: &str = "terminal";

/// One tab per terminal of the session. Until the session's terminals load, the stored tabs stay
/// listed, hidden, so a restore keeps them; a list that failed shows them, over its notice.
pub(super) fn list_tabs(s: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let wk = workspace_key(s.server_id_str(), ctx.session_id);
    let tab = |id: &str, title: String| PanelTabDesc::new(id, title).icon(&grid::TERMINAL_ICON).group("terminal");
    match s.terminal.workspaces.get(&wk) {
        Some(ws) if !ws.tabs.is_empty() || ws.failure.is_none() => ws.tabs.iter().map(|t| tab(&t.id, t.display_title())).collect(),
        Some(_) => ctx.open_ids.iter().map(|id| tab(id, "Terminal".into())).collect(),
        None => ctx.open_ids.iter().map(|id| tab(id, "Terminal".into()).hidden()).collect(),
    }
}

/// The `+` menu's and New tab's Terminal row (order 20): it opens a new terminal.
pub(super) fn launcher_desc(s: &App, _ctx: &PanelSessionCtx) -> Option<PanelLauncherDesc> {
    s.live_terminals().is_none_or(|(_, observer)| !observer).then(|| PanelLauncherDesc {
        id: "new",
        title: "Terminal".into(),
        description: None,
        icon: &grid::TERMINAL_ICON,
        shortcut: &[Bind::TerminalNew],
        order: 20,
        opened: false,
    })
}

/// The pane a new terminal opens in beside no other: Settings' terminal placement.
pub(super) fn placed(s: &App) -> SessionPane {
    match s.dock_placement() {
        crate::panels::DockPlacement::Side => SessionPane::Side,
        crate::panels::DockPlacement::Bottom => SessionPane::Bottom,
    }
}

pub(super) fn open_from_launcher(s: &mut App, pane: SessionPane, tx: &mut Tx) {
    s.create_terminal_tab(Some(pane), tx);
}

/// `terminal.new` (⌃⌥T): a new terminal, its tab selected.
pub(super) fn shortcut_new_terminal(s: &mut App, tx: &mut Tx) {
    s.create_terminal_tab(None, tx);
}

pub(super) fn render(s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    s.terminal.painted_dark.set(Some(p.dark));
    let ws_key = workspace_key(s.server_id_str(), ctx.session_id);
    grid::render_terminal_pane(s, p, &ws_key, Some(&ctx.tab.id), ctx.pane_size, ctx.env_scale, ctx.focused && !s.composer.focused)
}

/// A selected terminal tab selects its terminal, and takes the keyboard unless restored.
pub(super) fn on_focus(s: &mut App, session_id: &str, tab_id: &str, restored: bool) {
    let wk = s.terminal_workspace_key_for_session(session_id);
    s.ensure_terminal_workspace_hydrated(&wk);
    if let Some(ws) = s.terminal.workspaces.get_mut(&wk)
        && ws.tabs.iter().any(|t| t.id == tab_id)
        && ws.active.as_deref() != Some(tab_id)
    {
        ws.active = Some(tab_id.to_owned());
        s.queue_terminal_save();
    }
    if !restored {
        s.focus_terminal_surface();
    }
}

pub(super) fn on_enter(s: &mut App, session_id: &str, tab_id: &str) {
    on_focus(s, session_id, tab_id, false);
}

/// Closing a terminal's tab ends its shell, as Desktop's does.
pub(super) fn on_close(s: &mut App, session_id: &str, tab_id: &str, tx: &mut Tx) {
    let wk = s.terminal_workspace_key_for_session(session_id);
    s.close_terminal_tab(&wk, tab_id, tx);
}

pub(super) fn poll_panel(s: &mut App, tx: &mut Tx) -> bool {
    s.poll_terminal(tx)
}

pub(super) fn key_terminal(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some((wk, id)) = s.terminal_focus() else { return false };

    // ⌘W closes the terminal (Desktop's scoped `terminal.close`); the strip's tab keys stay the strip's.
    if Bind::TabClose.matches_tx(ev, tx) {
        s.close_side_tab(&panel_key(PLUGIN, &id), tx);
        return true;
    }
    if [Bind::PaneTabPrev, Bind::PaneTabNext].iter().any(|b| b.matches_tx(ev, tx)) {
        return false;
    }

    let blink = {
        let Some(ws) = s.terminal.workspaces.get(&wk) else { return false };
        let Some(tab) = ws.tabs.iter().find(|t| t.id == id) else { return false };
        let rows = tab.session.snapshot().rows.max(1);
        input::dispatch_key(
            &tab.session,
            tab.stream.as_ref(),
            tab.fixture_shell.then_some(ws.directory.as_str()),
            Some(wk.clone()),
            Some(tab.id.clone()),
            ev,
            rows,
            tx,
        )
    };
    match blink {
        input::KeyOutcome::Ignored => false,
        input::KeyOutcome::Consumed => true,
        input::KeyOutcome::ConsumedResetBlink => {
            s.terminal.reset_blink(s.clock);
            true
        }
    }
}

pub(super) fn ime_terminal(s: &mut App, ime: &Ime) -> bool {
    let Some((wk, id)) = s.terminal_focus() else { return false };
    let consumed = {
        let Some(ws) = s.terminal.workspaces.get(&wk) else { return false };
        let Some(tab) = ws.tabs.iter().find(|t| t.id == id) else { return false };
        input::dispatch_ime(&tab.session, tab.stream.as_ref(), tab.fixture_shell, ime)
    };
    if consumed {
        s.terminal.reset_blink(s.clock);
    }
    consumed
}

impl App {
    /// The terminal that has the keyboard: the one the focused pane shows, while that pane is on
    /// screen and the composer isn't focused.
    pub(crate) fn terminal_focus(&self) -> Option<(String, String)> {
        if !self.side.focused || self.composer.focused {
            return None;
        }
        let sid = self.active_session_id()?;
        let pane = self.side.active_pane_focus;
        let visible = if pane == SessionPane::Bottom { self.bottom_open() } else { self.side_open() };
        let key = visible.then(|| self.pane_shown_tab(pane)).flatten()?;
        let id = key.strip_prefix(PLUGIN)?.strip_prefix(':')?;
        Some((self.terminal_workspace_key_for_session(sid), id.to_owned()))
    }

    /// Starts a terminal in the routed session and opens its tab, in `pane` or beside its other
    /// terminals (Desktop's `create`).
    pub(crate) fn create_terminal_tab(&mut self, pane: Option<SessionPane>, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let wk = self.terminal_workspace_key_for_session(&sid);
        tx.instant();
        self.ensure_terminal_workspace_hydrated(&wk);
        let Some(id) = self.new_terminal_in(&wk) else { return };
        self.show_side_tab(&sid, &panel_key(PLUGIN, &id), pane);
        self.focus_terminal_surface();
    }

    /// `terminal.toggle` (⌃`): a new terminal when the session has none; its active terminal's
    /// pane hides while that tab shows; otherwise the tab opens and shows.
    pub(crate) fn toggle_terminal(&mut self, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let wk = self.terminal_workspace_key_for_session(&sid);
        self.ensure_terminal_workspace_hydrated(&wk);
        let active = self.terminal.workspaces.get(&wk).and_then(|ws| ws.active.clone().or_else(|| ws.tabs.first().map(|t| t.id.clone())));
        let Some(id) = active else { return self.create_terminal_tab(None, tx) };
        let key = panel_key(PLUGIN, &id);
        if self.side_tab_visible(&key) {
            return self.hide_side_tab(&key, tx);
        }
        tx.instant();
        self.select_terminal_tab(&wk, &id);
        self.show_side_tab(&sid, &key, None);
        self.focus_terminal_surface();
    }
}

impl App {
    /// Handles server SSE `pty.exited` and `pty.updated` events (`packages/schema/src/pty.ts`).
    pub fn apply_pty_event(&mut self, ty: &str, data: &Value) -> bool {
        let terminal = if data["terminal"].is_object() { &data["terminal"] } else { &data["properties"]["terminal"] };
        match ty {
            // Another client's terminal joins the session's strip at the end.
            "persistent-pty.added" => {
                let Some(info) = parse_persistent_pty_info(terminal) else { return false };
                let wk = self.terminal_workspace_key_for_session(&info.session_id);
                let Some((srv, _)) = self.live_terminals() else { return false };
                let known = self.terminal.workspaces.get(&wk).map(|ws| ws.tabs.iter().any(|t| t.id == info.id));
                if known != Some(false) {
                    return false;
                }
                let Ok(tab) = self.persistent_tab(srv, info) else { return false };
                if let Some(ws) = self.terminal.workspaces.get_mut(&wk) {
                    ws.failure = None;
                    ws.active.get_or_insert_with(|| tab.id.clone());
                    ws.tabs.push(tab);
                }
                true
            }
            "persistent-pty.removed" => {
                let id = data["ptyID"].as_str().or_else(|| data["properties"]["ptyID"].as_str()).unwrap_or_default();
                !id.is_empty() && self.remove_exited_pty(id)
            }
            "persistent-pty.updated" => {
                let Some(info) = parse_persistent_pty_info(terminal) else { return false };
                let mut changed = false;
                for tab in self.terminal.workspaces.values_mut().flat_map(|ws| ws.tabs.iter_mut()) {
                    if tab.id == info.id && !info.title.is_empty() && tab.title != info.title {
                        tab.title.clone_from(&info.title);
                        changed = true;
                    }
                }
                changed
            }
            "pty.exited" => {
                let Some(pty_id) = data["id"]
                    .as_str()
                    .or_else(|| data["properties"]["id"].as_str())
                    .filter(|s| !s.is_empty())
                else {
                    return false;
                };
                self.remove_exited_pty(pty_id)
            }
            "pty.updated" => {
                let info = if data["info"].is_object() {
                    &data["info"]
                } else {
                    &data["properties"]["info"]
                };
                let Some(pty_id) = info["id"].as_str().filter(|s| !s.is_empty()) else {
                    return false;
                };
                let Some(new_title) = info["title"].as_str().filter(|s| !s.is_empty()) else {
                    return false;
                };
                let mut changed = false;
                for ws in self.terminal.workspaces.values_mut() {
                    for tab in &mut ws.tabs {
                        if tab.id == pty_id && tab.title != new_title {
                            tab.title = new_title.to_owned();
                            changed = true;
                        }
                    }
                }
                if changed {
                    self.queue_terminal_save();
                }
                changed
            }
            _ => false,
        }
    }

    pub(super) fn remove_exited_pty(&mut self, pty_id: &str) -> bool {
        let mut removed = false;

        for ws in self.terminal.workspaces.values_mut() {
            if let Some(idx) = ws.tabs.iter().position(|t| t.id == pty_id) {
                ws.tabs.remove(idx);
                if ws.active.as_deref() == Some(pty_id) {
                    ws.active = if idx > 0 {
                        ws.tabs.get(idx - 1).map(|t| t.id.clone())
                    } else {
                        ws.tabs.first().map(|t| t.id.clone())
                    };
                }
                removed = true;
            }
        }
        for sw in self.terminal.stored.values_mut() {
            if let Some(idx) = sw.all.iter().position(|p| p.id == pty_id) {
                sw.all.remove(idx);
                if sw.active.as_deref() == Some(pty_id) {
                    sw.active = sw.all.first().map(|p| p.id.clone());
                }
                removed = true;
            }
        }

        // An exited terminal's tab leaves the strip with the panel's listing (it's transient).
        if removed {
            self.queue_terminal_save();
        }
        removed
    }
}