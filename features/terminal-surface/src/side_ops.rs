//! The terminal as a side panel (Desktop's terminal plugin, `renderer.tsx`): one tab per
//! terminal of the session in the side region's shared strip, grouped so the view stays mounted
//! between them; key and IME dispatch to the terminal that has the keyboard, and the server's
//! `pty.*` events.

use opencode_kit::persistent_pty::parse_persistent_pty_info;
use opencode_ui::keymap::Bind;
use rune::{Drag, El, Ime, KeyEvent, PointerPress, Tx, icon, row, text};
use serde_json::Value;
use opencode_ui::palette::Pal;

use crate::{
    PaneTarget, TAB_UI, TerminalSurfaceHost, calls, find, grid, input, ops, panel_key, persist, poll,
    rename,
};

/// Every terminal tab's key starts so: `terminal:{pty id}`.
pub const PLUGIN: &str = "terminal";

/// A terminal's label (`TerminalTabLabel`): Desktop's prompt glyph, then its title, at most 160
/// wide; a double-click on the title or the label's own menu renames it in a field over it.
pub fn tab_label<H: TerminalSurfaceHost + 'static>(
    s: &H,
    p: &Pal,
    id: &str,
    title: &str,
    active: bool,
) -> El {
    let field = rename::field::<H>(s, p, id);
    let (rename_id, menu_id) = (id.to_owned(), id.to_owned());
    let (drag_key, drag_title) = (panel_key(id), title.to_owned());
    let shown = active && s.is_side_focused();
    row()
        .key(format!("terminal-label-{id}"))
        .gap(6.0)
        .center_y()
        .min_w(0.0)
        .on_secondary_press(move |s: &mut H, press: PointerPress, tx: &mut Tx| rename::menu(s, &menu_id, press, tx))
        // At the strip's narrowest the glyph gives way with the title, each by its share of
        // their width, as Desktop's `<Icon>` and `truncate` span shrink in their flex row: a
        // 72-wide tab shows a dot and "C…".
        .child(icon(&grid::TERMINAL_ICON, 16.0).shrink().min_w(0.0).color(if active { p.icon } else { p.icon_muted }))
        .child(
            text(title.to_owned())
                .key("terminal-tab-title")
                // `terminal-tab-title` is tabular too (`tabs.css:313`).
                .font(TAB_UI)
                .color(if active { p.text } else { p.muted })
                .truncate()
                .max_w(160.0)
                .shrink()
                .opacity(if field.is_some() { 0.0 } else { 1.0 })
                .on_double_press_tx(move |s: &mut H, tx: &mut Tx| rename::start(s, &rename_id, rename::Source::Pointer, tx))
                // The title takes the press for its double-click, so it carries the tab's drag too:
                // the whole tab drags, as Desktop's does.
                .on_drag(move |s: &mut H, d: &mut Drag, tx: &mut Tx| s.side_tab_gesture(&drag_key, &drag_title, true, shown, d, tx)),
        )
        .children(field)
}

/// Terminal `id` of session `sid` filling a pane `size` big, at `scale` device pixels a point.
pub fn surface<H: TerminalSurfaceHost + 'static>(
    s: &H,
    p: &Pal,
    sid: &str,
    id: &str,
    size: (f32, f32),
    scale: f32,
    focused: bool,
) -> El {
    s.terminal().painted_dark.set(Some(p.dark));
    let ws_key = ops::terminal_workspace_key_for_session(s, sid);
    let shown = s.side_tab_visible(&panel_key(id));
    grid::render_terminal_pane(s, p, &ws_key, Some(id), size, scale, focused && !s.is_composer_focused(), shown)
}

/// A selected terminal tab selects its terminal, and takes the keyboard unless restored.
pub fn on_focus(host: &mut dyn TerminalSurfaceHost, session_id: &str, tab_id: &str, restored: bool) {
    let wk = ops::terminal_workspace_key_for_session(host, session_id);
    persist::ensure_terminal_workspace_hydrated(host, &wk);
    let mut changed = false;
    if let Some(ws) = host.terminal_mut().workspaces.get_mut(&wk)
        && ws.tabs.iter().any(|t| t.id == tab_id)
        && ws.active.as_deref() != Some(tab_id)
    {
        ws.active = Some(tab_id.to_owned());
        changed = true;
    }
    if changed {
        persist::queue_terminal_save(host);
    }
    if !restored {
        ops::focus_terminal_surface(host);
    }
}

pub fn poll_panel(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) -> bool {
    poll_asked(host, tx) | rename::poll(host, tx) | poll::poll_terminal(host, tx) | crate::grid::poll_edge_scroll(host, tx)
}

pub fn key_terminal(host: &mut dyn TerminalSurfaceHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // The terminal has the keys only while keyboard focus is on its viewport (which a click, a
    // toggle or Tab puts there): anywhere else, Tab, F6 and typing are that control's, so the
    // keyboard is never trapped in the shell.
    if tx.focused() != Some(grid::SCREEN_KEY) {
        return false;
    }
    let Some((wk, id)) = terminal_focus(host) else { return false };
    host.speech_terminal_key(&wk, &id);

    // ⌘W closes the terminal (Desktop's scoped `terminal.close`); the strip's tab keys stay the strip's.
    if Bind::TabClose.matches_tx(ev, tx) {
        host.close_side_tab(&panel_key(&id), tx);
        // The keyboard stays somewhere: on the terminal the pane shows next, else back in the
        // prompt, as Desktop's composer takes focus when its terminal closes.
        let h: &dyn TerminalSurfaceHost = &*host;
        match terminal_focus(h).filter(|(_, next)| h.side_tab_visible(&panel_key(next))) {
            Some(_) => tx.focus(grid::SCREEN_KEY, tx.focus_visible()),
            None => tx.focus("prompt", tx.focus_visible()),
        }
        return true;
    }
    if [Bind::PaneTabPrev, Bind::PaneTabNext].iter().any(|b| b.matches_tx(ev, tx)) {
        return false;
    }
    if host.plugin_takes_chord(ev, tx) {
        return false;
    }

    let option_meta = host.terminal_option_meta();
    let blink = {
        let Some(ws) = host.terminal().workspaces.get(&wk) else { return false };
        let Some(tab) = ws.tabs.iter().find(|t| t.id == id) else { return false };
        let rows = tab.session.snapshot().rows.max(1);
        input::dispatch_key(
            &tab.session,
            tab.backend.fixture_stream().map(|st| (st, ws.directory.as_str())),
            ev,
            rows,
            option_meta,
            tx,
        )
    };
    match blink {
        input::KeyOutcome::Ignored => false,
        input::KeyOutcome::Consumed => true,
        input::KeyOutcome::OpenFind => {
            find::open(host, &wk, &id);
            true
        }
        input::KeyOutcome::Paste => {
            host.read_clipboard_for_paste(wk, id, tx);
            true
        }
        input::KeyOutcome::ConsumedResetBlink => {
            let now = host.clock();
            host.terminal_mut().reset_blink(now);
            true
        }
    }
}

pub fn ime_terminal(host: &mut dyn TerminalSurfaceHost, ime: &Ime) -> bool {
    let Some((wk, id)) = terminal_focus(host) else { return false };
    host.speech_terminal_key(&wk, &id);
    let now = host.clock();
    let Some(tab) = ops::terminal_tab_mut(host, &wk, &id) else { return false };
    let consumed = input::dispatch_ime(tab, ime);
    if consumed {
        host.terminal_mut().reset_blink(now);
    }
    consumed
}

/// The terminal that has the keyboard: the one the focused pane shows, while that pane is on
/// screen and the composer isn't focused.
pub fn terminal_focus(host: &dyn TerminalSurfaceHost) -> Option<(String, String)> {
    if !host.is_side_focused() || host.is_composer_focused() {
        return None;
    }
    let sid = host.active_session_id()?;
    let key = host.focused_pane_shown_tab()?;
    let id = key.strip_prefix(PLUGIN)?.strip_prefix(':')?;
    Some((ops::terminal_workspace_key_for_session(host, sid), id.to_owned()))
}

/// A terminal asked for (a toggle, a new tab, a plugin's open) before the server listed its
/// session (`ops::waits_for_folder`). It's done once the session is listed, while the page
/// still shows that session; the newest ask replaces an older one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    Toggle { session: String },
    Create { session: String, pane: Option<PaneTarget> },
    Open { session: String, new_tab: bool },
    /// A terminal when the session has none (the narrow window's Terminal view).
    First { session: String },
}

/// Does the terminal asked for while its session wasn't listed, now that it is; forgets it
/// when the page moved to another session.
pub fn poll_asked(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) -> bool {
    let Some(asked) = host.terminal().asked.clone() else { return false };
    let (Asked::Toggle { session } | Asked::Create { session, .. } | Asked::Open { session, .. } | Asked::First { session }) = &asked;
    if host.active_session_id() != Some(session.as_str()) {
        host.terminal_mut().asked = None;
        return false;
    }
    if ops::waits_for_folder(host, session) || calls::listing(host, &ops::terminal_workspace_key_for_session(host, session)) {
        return false;
    }
    host.terminal_mut().asked = None;
    match asked {
        Asked::Toggle { .. } => toggle_terminal(host, tx),
        Asked::Create { pane, .. } => create_terminal_tab(host, pane, tx),
        Asked::Open { session, new_tab } => drop(ops::open_terminal_for_session(host, &session, new_tab)),
        Asked::First { .. } => first_terminal(host, tx),
    }
    true
}

/// Starts the routed session's first terminal when it has none and this window may (the narrow
/// window's Terminal view); with its terminals still being listed, once they are.
pub fn first_terminal(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    let wk = ops::terminal_workspace_key_for_session(host, &sid);
    if !ops::waits_for_folder(host, &sid) {
        persist::ensure_terminal_workspace_hydrated(host, &wk);
    }
    if ops::waits_for_folder(host, &sid) || calls::listing(host, &wk) {
        host.terminal_mut().asked = Some(Asked::First { session: sid });
        return;
    }
    let empty = host.terminal().workspaces.get(&wk).is_none_or(|w| w.tabs.is_empty() && w.failure.is_none());
    if empty && ops::live_terminals(host).is_some_and(|(_, observer)| !observer) {
        create_terminal_tab(host, None, tx);
    }
}

/// Starts a terminal in the routed session and opens its tab, in `pane` or beside its other
/// terminals (Desktop's `create`).
pub fn create_terminal_tab(host: &mut dyn TerminalSurfaceHost, pane: Option<PaneTarget>, tx: &mut Tx) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    if ops::waits_for_folder(host, &sid) {
        host.terminal_mut().asked = Some(Asked::Create { session: sid, pane });
        return;
    }
    let wk = ops::terminal_workspace_key_for_session(host, &sid);
    persist::ensure_terminal_workspace_hydrated(host, &wk);
    // Its number comes from the terminals it has: asked again when the server has listed them.
    if calls::listing(host, &wk) {
        host.terminal_mut().asked = Some(Asked::Create { session: sid, pane });
        return;
    }
    // On a live server its tab shows when the server has started it (`calls::After::Pane`).
    let after = calls::After::Pane { session: sid.clone(), pane };
    let Some(id) = ops::new_terminal_in(host, &wk, after) else { return };
    let key = panel_key(&id);
    host.show_pane_tab(&sid, &key, pane, tx);
    ops::focus_terminal_surface(host);
}

/// `terminal.toggle` (⌃`): a new terminal when the session has none; its active terminal's
/// pane hides while that tab shows; otherwise the tab opens and shows.
pub fn toggle_terminal(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    if ops::waits_for_folder(host, &sid) {
        host.terminal_mut().asked = Some(Asked::Toggle { session: sid });
        return;
    }
    let wk = ops::terminal_workspace_key_for_session(host, &sid);
    persist::ensure_terminal_workspace_hydrated(host, &wk);
    // Whether it has a terminal to show isn't known yet: asked again when the server has listed them.
    if calls::listing(host, &wk) {
        host.terminal_mut().asked = Some(Asked::Toggle { session: sid });
        return;
    }
    let active = host
        .terminal()
        .workspaces
        .get(&wk)
        .and_then(|ws| ws.active.clone().or_else(|| ws.tabs.first().map(|t| t.id.clone())));
    let Some(id) = active else {
        return create_terminal_tab(host, None, tx);
    };
    let key = panel_key(&id);
    if host.side_tab_visible(&key) {
        return host.hide_side_tab(&key, tx);
    }
    ops::select_terminal_tab(host, &wk, &id);
    host.show_pane_tab(&sid, &key, None, tx);
    ops::focus_terminal_surface(host);
}

/// Handles server SSE `persistent-pty.*`, `pty.exited`, and `pty.updated` events (`packages/schema/src/pty.ts`).
pub fn apply_pty_event(host: &mut dyn TerminalSurfaceHost, ty: &str, data: &Value) -> bool {
    let terminal = if data["terminal"].is_object() { &data["terminal"] } else { &data["properties"]["terminal"] };
    match ty {
        // Another client's terminal joins the session's strip at the end.
        "persistent-pty.added" => {
            let Some(info) = parse_persistent_pty_info(terminal) else { return false };
            let wk = ops::terminal_workspace_key_for_session(host, &info.session_id);
            let Some((srv, _)) = ops::live_terminals(host) else { return false };
            let known = host.terminal().workspaces.get(&wk).map(|ws| ws.tabs.iter().any(|t| t.id == info.id));
            if known != Some(false) {
                return false;
            }
            let Ok(tab) = ops::persistent_tab(host, srv, info) else { return false };
            if let Some(ws) = host.terminal_mut().workspaces.get_mut(&wk) {
                ws.failure = None;
                ws.active.get_or_insert_with(|| tab.id.clone());
                ws.tabs.push(tab);
            }
            true
        }
        "persistent-pty.removed" => {
            let id = data["ptyID"].as_str().or_else(|| data["properties"]["ptyID"].as_str()).unwrap_or_default();
            !id.is_empty() && remove_exited_pty(host, id)
        }
        "persistent-pty.updated" => {
            let Some(info) = parse_persistent_pty_info(terminal) else { return false };
            let mut changed = false;
            for tab in host.terminal_mut().workspaces.values_mut().flat_map(|ws| ws.tabs.iter_mut()) {
                if tab.id == info.id && !info.title.is_empty() && tab.title != info.title {
                    tab.title.clone_from(&info.title);
                    tab.custom_title = info.user_title.is_some();
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
            remove_exited_pty(host, pty_id)
        }
        "pty.updated" => {
            let info = if data["info"].is_object() { &data["info"] } else { &data["properties"]["info"] };
            let Some(pty_id) = info["id"].as_str().filter(|s| !s.is_empty()) else {
                return false;
            };
            let Some(new_title) = info["title"].as_str().filter(|s| !s.is_empty()) else {
                return false;
            };
            let mut changed = false;
            for ws in host.terminal_mut().workspaces.values_mut() {
                for tab in &mut ws.tabs {
                    if tab.id == pty_id && tab.title != new_title {
                        tab.title = new_title.to_owned();
                        changed = true;
                    }
                }
            }
            if changed {
                persist::queue_terminal_save(host);
            }
            changed
        }
        _ => false,
    }
}

pub(crate) fn remove_exited_pty(host: &mut dyn TerminalSurfaceHost, pty_id: &str) -> bool {
    let mut removed = false;
    {
        let pane = host.terminal_mut();
        for ws in pane.workspaces.values_mut() {
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
        for sw in pane.stored.values_mut() {
            if let Some(idx) = sw.all.iter().position(|p| p.id == pty_id) {
                sw.all.remove(idx);
                if sw.active.as_deref() == Some(pty_id) {
                    sw.active = sw.all.first().map(|p| p.id.clone());
                }
                removed = true;
            }
        }
    }

    // An exited terminal's tab leaves the strip with the panel's listing (it's transient).
    if removed {
        persist::queue_terminal_save(host);
    }
    removed
}
