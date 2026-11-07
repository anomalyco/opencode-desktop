//! Dock key/IME dispatch, SSE `pty.exited` / `pty.updated` cleanup, and stale-PTY recovery.

use opencode_kit::persistent_pty::parse_persistent_pty_info;
use rune::{El, Ime, Key, KeyEvent, Tx};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use super::{grid, input, workspace_key};
use crate::App;
use crate::keymap::Bind;
use crate::palette::Pal;
use crate::panels::{
    PanelLauncherDesc, PanelSessionCtx, PanelTabDesc, PanelViewCtx, SessionPane,
};

pub(super) fn list_dock_tabs(_s: &App, _ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    vec![PanelTabDesc::new("main", "Terminal").icon(&ic::CONSOLE)]
}

pub(super) fn launcher_desc(s: &App, _ctx: &PanelSessionCtx) -> Option<PanelLauncherDesc> {
    Some(PanelLauncherDesc {
        id: "open",
        title: "Terminal".into(),
        description: Some("Open integrated terminal".into()),
        icon: &ic::CONSOLE,
        shortcut: &[Bind::DockToggle],
        order: 20,
        opened: s.dock_open(),
    })
}

pub(super) fn open_from_launcher(s: &mut App, _pane: SessionPane, tx: &mut Tx) {
    s.toggle_dock(tx);
}

pub(super) fn shortcut_new_terminal(s: &mut App, tx: &mut Tx) {
    let Some(wk) = s.active_terminal_workspace_key() else {
        return;
    };
    let had_tabs = s
        .terminal
        .workspaces
        .get(&wk)
        .is_some_and(|w| !w.tabs.is_empty())
        || s.terminal
            .stored
            .get(&wk)
            .is_some_and(|w| !w.all.is_empty());

    if !s.dock_open() {
        s.open_dock(tx);
        if had_tabs {
            s.new_terminal_in(&wk);
        }
        return;
    }

    tx.instant();
    s.ensure_terminal_workspace_hydrated(&wk);
    s.new_terminal_in(&wk);
    s.focus_terminal_surface();
}

pub(super) fn render_dock(s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    s.terminal.painted_dark.set(Some(p.dark));
    let ws_key = workspace_key(s.server_id_str(), ctx.session_id);
    grid::render_terminal_pane(
        s,
        p,
        &ws_key,
        ctx.pane_size,
        ctx.env_scale,
        (ctx.focused || !ctx.visible) && !s.composer.focused,
        ctx.reserve_top_right,
        // Alone in the right column, the strip is Desktop's: no hide button, the side toggle ends it.
        !ctx.reserve_top_right,
    )
}

pub(super) fn on_enter_dock(s: &mut App, session_id: &str, _tab_id: &str) {
    let wk = s.terminal_workspace_key_for_session(session_id);
    s.ensure_workspace_ready(&wk);
    s.focus_terminal_surface();
}

pub(super) fn poll_panel(s: &mut App, tx: &mut Tx) -> bool {
    s.poll_terminal(tx)
}

pub(super) fn key_terminal(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !s.dock_open() || !s.side.dock_focused || s.composer.focused {
        return false;
    }
    let Some(wk) = s.active_terminal_workspace_key() else {
        return false;
    };

    if Bind::TabClose.matches_tx(ev, tx) {
        let active_id = s.terminal.workspaces.get(&wk).and_then(|ws| {
            ws.active
                .clone()
                .or_else(|| ws.tabs.first().map(|t| t.id.clone()))
        });
        if let Some(tid) = active_id {
            s.close_terminal_tab(&wk, &tid, tx);
        } else {
            s.side.emptied_guard = true;
            s.close_dock(tx);
        }
        return true;
    }

    let prev_tab = Bind::PaneTabPrev.matches_tx(ev, tx);
    let next_tab = Bind::PaneTabNext.matches_tx(ev, tx);
    if (prev_tab || next_tab)
        && s.select_adjacent_terminal_tab(&wk, if prev_tab { -1 } else { 1 }, tx)
    {
        return true;
    }

    let blink = {
        let Some(ws) = s.terminal.workspaces.get(&wk) else {
            return false;
        };
        let Some(tab) = ws
            .active
            .as_deref()
            .and_then(|id| ws.tabs.iter().find(|t| t.id == id))
            .or_else(|| ws.tabs.first())
        else {
            if ev.key == Key::Escape {
                s.close_dock(tx);
                return true;
            }
            return false;
        };
        let rows = tab.session.snapshot().rows.max(1);
        input::dispatch_key(
            &tab.session,
            tab.stream.as_ref(),
            tab.fixture_shell.then_some(ws.directory.as_str()),
            Some(wk),
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
    if !s.dock_open() || !s.side.dock_focused || s.composer.focused {
        return false;
    }
    let Some(wk) = s.active_terminal_workspace_key() else {
        return false;
    };
    let consumed = {
        let Some(ws) = s.terminal.workspaces.get(&wk) else {
            return false;
        };
        let Some(tab) = ws
            .active
            .as_deref()
            .and_then(|id| ws.tabs.iter().find(|t| t.id == id))
            .or_else(|| ws.tabs.first())
        else {
            return false;
        };
        input::dispatch_ime(&tab.session, tab.stream.as_ref(), tab.fixture_shell, ime)
    };
    if consumed {
        s.terminal.reset_blink(s.clock);
    }
    consumed
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
        let active_wk = self.active_terminal_workspace_key();
        let mut removed = false;
        let mut active_ws_emptied = false;

        for (wk, ws) in &mut self.terminal.workspaces {
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
                if active_wk.as_deref() == Some(wk.as_str()) && ws.tabs.is_empty() {
                    active_ws_emptied = true;
                }
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

        if active_ws_emptied
            && self.stage.is_none()
            && let Some(sid) = self.active_session_id().map(str::to_owned)
            && self.side.sessions.get(&sid).is_some_and(|st| st.dock_open)
        {
            self.side.dock_last_solo = !self.side_open();
            self.session_side_mut(&sid).dock_open = false;
            self.side.dock_focused = false;
        }

        if removed {
            self.queue_terminal_save();
        }
        removed
    }
}
