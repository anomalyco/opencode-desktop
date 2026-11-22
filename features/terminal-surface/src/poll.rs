//! Per-frame terminal polling: mount state machine syncing, worker event draining,
//! `?1004` focus reports, theme updates, visual bell timeout, and phase-locked cursor blink.

use std::cell::Cell;
use std::sync::atomic::Ordering;

use opencode_kit::client::Server;
use rune::Tx;
use terminal::{SessionStatus, TerminalConfig, TerminalSession, VtEvent};

use crate::{
    BellEffect, BellEvent, BlinkEffect, BlinkEvent, FocusClaimEffect, FocusClaimEvent, PaintedTheme,
    TerminalSurfaceHost, TerminalTab, grid, mount, ops, panel_key, persist, side_ops,
};

/// Steps each session terminal's [`mount::Mount`]: a failed one leaves once it doesn't show,
/// and showing it again, or the server reconnecting, attaches it anew. Quiet (and free of
/// allocations) while every terminal is live.
fn sync_terminal_mounts(host: &mut dyn TerminalSurfaceHost) -> bool {
    let Some((srv, observer)) = ops::live_terminals(host) else { return false };
    let failed = |t: &TerminalTab| matches!(t.session.status(), SessionStatus::Failed(_));
    let quiet = host
        .terminal()
        .workspaces
        .values()
        .flat_map(|ws| &ws.tabs)
        .all(|t| matches!(t.mount, mount::Mount::Live { .. }) && !failed(t));
    if quiet {
        return false;
    }
    let routed = ops::active_terminal_workspace_key(host);
    let connection = host.terminal().connections;
    let mut attach = Vec::new();
    let mut changed = false;
    let ids: Vec<(String, String)> = host
        .terminal()
        .workspaces
        .iter()
        .flat_map(|(wk, ws)| ws.tabs.iter().map(|t| (wk.clone(), t.id.clone())))
        .collect();
    for (wk, id) in ids {
        let shown = routed.as_deref() == Some(wk.as_str()) && host.side_tab_visible(&panel_key(&id));
        let Some(tab) = ops::terminal_tab_mut(host, &wk, &id) else { continue };
        let event = if matches!(tab.mount, mount::Mount::Live { .. }) && failed(tab) {
            mount::MountEvent::Failed { shown, connection }
        } else {
            mount::MountEvent::Sync { shown, connection }
        };
        let before = tab.mount;
        for effect in tab.mount.step(event) {
            match effect {
                mount::MountEffect::Mount => attach.push((wk.clone(), id.clone())),
            }
        }
        changed |= tab.mount != before;
    }
    for (wk, id) in attach {
        attach_terminal(host, srv.clone(), &wk, &id, observer);
    }
    changed
}

/// A new attachment for terminal `id`, at the size its screen had.
fn attach_terminal(host: &mut dyn TerminalSurfaceHost, srv: Server, wk: &str, id: &str, observer: bool) {
    let theme = host.terminal_theme(ops::terminal_dark(host));
    let waker = ops::terminal_waker(host);
    host.fetch_terminal_fonts_if_wasm();
    let Some(tab) = ops::terminal_tab_mut(host, wk, id) else { return };
    let snap = tab.session.snapshot();
    let config = TerminalConfig {
        cols: snap.cols.max(1),
        rows: snap.rows.max(1),
        theme,
        ..TerminalConfig::default()
    };
    if let Ok(session) = TerminalSession::spawn_persistent_pty(srv, id.to_owned(), config, observer, waker) {
        tab.session = session;
        tab.fit = Cell::new(mount::Fit::Unmeasured);
        tab.drag = mount::TextDrag::Idle;
        tab.edge_scroll = None;
    }
}

/// Drains worker updates, visual bell flashes, exited/recovered PTYs, and phase-locked cursor blink edges.
/// Zero heap allocations and `false` return on idle frames so `idle_poll` stays silent.
pub fn poll_terminal(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) -> bool {
    if host.terminal().focus_claim.is_pending() {
        for effect in host.terminal_mut().focus_claim.step(FocusClaimEvent::Poll) {
            match effect {
                FocusClaimEffect::FocusScreen => {
                    if tx.focused() != Some(grid::SCREEN_KEY) {
                        tx.focus(grid::SCREEN_KEY, tx.focus_visible());
                    }
                }
            }
        }
    }
    if host.poll_story_variant() {
        return true;
    }

    // A session whose strip holds terminal tabs loads its terminals, so the tabs can show.
    let stored_tabs = !host.is_story_stage()
        && match host.active_session_id() {
            Some(id) => host.session_has_terminal_tabs(id),
            None => false,
        };
    if stored_tabs
        && let Some(wk) = ops::active_terminal_workspace_key(host)
        && !host.terminal().workspaces.contains_key(&wk)
    {
        persist::ensure_terminal_workspace_hydrated(host, &wk);
        return true;
    }

    if !host.terminal().workspaces.is_empty() {
        let dark = ops::terminal_dark(host);
        let painted = Some(PaintedTheme {
            dark,
            theme: opencode_ui::themes::active(),
            generation: opencode_ui::themes::generation(),
        });
        if host.terminal().last_theme != painted {
            let theme = host.terminal_theme(dark);
            let pane = host.terminal_mut();
            pane.last_theme = painted;
            for ws in pane.workspaces.values() {
                for tab in &ws.tabs {
                    tab.session.set_theme(theme.clone());
                }
            }
        }
    }

    let mut changed = host.terminal().dirty_flag.swap(false, Ordering::AcqRel);

    if changed {
        let now = host.clock();
        let story = host.is_story_stage();
        let mut bell = false;
        let mut title_changed = false;
        let mut to_remove: Vec<String> = Vec::new();

        for ws in host.terminal_mut().workspaces.values_mut() {
            for tab in &mut ws.tabs {
                for ev in tab.session.drain_events() {
                    match ev {
                        VtEvent::Bell => bell = true,
                        VtEvent::TitleChanged(new_title)
                            if !tab.custom_title && !new_title.is_empty() && tab.title != new_title =>
                        {
                            tab.title = new_title;
                            title_changed = true;
                        }
                        _ => {}
                    }
                }
                if matches!(tab.session.status(), SessionStatus::Exited { .. }) && !story {
                    to_remove.push(tab.id.clone());
                }
            }
        }

        for tid in to_remove {
            let _ = side_ops::remove_exited_pty(host, &tid);
        }
        if title_changed {
            persist::queue_terminal_save(host);
        }
        if bell {
            for effect in host.terminal_mut().bell.step(BellEvent::Rang { now }) {
                match effect {
                    BellEffect::Wake(after) => tx.wake_after(after),
                    BellEffect::Expired => changed = true,
                }
            }
        }
    }

    changed |= sync_terminal_mounts(host);

    // Focus reports (`?1004`): the program hears its terminal gain and lose the keyboard, the
    // window's included (vim's FocusGained reloads a file the agent changed).
    if !host.terminal().workspaces.is_empty() {
        let focus = if host.is_window_blurred() { None } else { side_ops::terminal_focus(host) };
        for (wk, ws) in &host.terminal().workspaces {
            for tab in &ws.tabs {
                let has = focus.as_ref().is_some_and(|(fw, fid)| fw == wk && *fid == tab.id);
                if tab.focused.replace(has) != has && tab.session.peek().focus_reporting {
                    tab.session.write_bytes(if has { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
                }
            }
        }
    }

    let now = host.clock();
    if host.terminal().bell.expired(now) {
        for effect in host.terminal_mut().bell.step(BellEvent::Tick { now }) {
            match effect {
                BellEffect::Wake(after) => tx.wake_after(after),
                BellEffect::Expired => changed = true,
            }
        }
    }

    let visible_and_focused = if host.terminal().workspaces.is_empty() {
        false
    } else if host.is_story_stage() {
        host.terminal().story.is_focused()
    } else {
        side_ops::terminal_focus(host).is_some()
    };

    if visible_and_focused {
        let blinks = match ops::active_terminal_workspace_key(host) {
            Some(k) => host
                .terminal()
                .workspaces
                .get(&k)
                .and_then(|ws| {
                    let tab = ws
                        .active
                        .as_deref()
                        .and_then(|id| ws.tabs.iter().find(|t| t.id == id))
                        .or_else(|| ws.tabs.first())?;
                    let c = tab.session.snapshot().cursor;
                    Some(c.visible && c.in_viewport && c.blinking)
                })
                .unwrap_or(false),
            None => false,
        };

        if blinks {
            if host.terminal().blink.flips_at(now) {
                for effect in host.terminal_mut().blink.step(BlinkEvent::Tick { now }) {
                    match effect {
                        BlinkEffect::Redraw => changed = true,
                        BlinkEffect::Wake(after) => tx.wake_after(after),
                    }
                }
            } else {
                tx.wake_after(host.terminal().blink.next_edge(now));
            }
        }
    }

    changed
}
