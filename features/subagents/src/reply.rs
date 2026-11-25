//! Answering a subagent from its own page. The page keeps the main composer, with the
//! subagent's agent and model in its pickers, and its status line under it: how the run went,
//! how long it took, and the way back (Esc with nothing typed).
//!
//! A reply goes to the subagent's session as its own agent and model. A builder whose worktree
//! was removed still hears it: the server moves the session to its project's checkout first
//! (`Server::send_moving_home`). A reply is an outbox prompt like any other
//! (`send_failed::Phase`, `Operation::MovingHome`): kept across a lost connection or a restart,
//! and one the server refuses comes back out of the timeline into the composer.

use opencode_kit::client::{Prompt, Write};
use opencode_kit::model::SessionInfo;
use rune::{El, Key, KeyEvent, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use session_view::focus::FocusExt;
use session_view::palette::Pal;
use session_view::style::{INSTANT, activity_ring_sized};

use crate::{SubagentsHost, open_parent, run_clock, run_time, subtree_running};

/// The open page's session when it's a subagent's.
pub fn subagent_page(host: &dyn SubagentsHost) -> Option<&SessionInfo> {
    let id = host.active_session_id()?;
    host.session_info(id).filter(|i| i.parent.is_some())
}

/// The write that sends `prompt` to `session`. A subagent's moves home first when its folder is
/// gone.
pub fn send_write(host: &dyn SubagentsHost, session: &str, prompt: Prompt) -> Write {
    if !host.session_info(session).is_some_and(|i| i.parent.is_some()) {
        return Write::Send { session: session.to_owned(), prompt };
    }
    Write::SendMovingHome { session: session.to_owned(), prompt }
}

/// Whether the open page is a subagent's with nothing typed in its composer.
pub fn composer_empty(host: &dyn SubagentsHost) -> bool {
    subagent_page(host).is_some() && host.composer_is_empty()
}

/// Esc in a subagent's empty composer: back to the session that started it.
pub fn esc(host: &mut dyn SubagentsHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = ev.mods;
    ev.key == Key::Escape && !m.cmd && !m.ctrl && !m.alt && !m.shift && composer_empty(host) && open_parent(host, tx)
}

/// The bar's words (`subagent-bar.tsx`): 12 on 16 at 440, tabular, faint.
const BAR: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0).tracking(-0.04).features("\"tnum\"");
/// The back button's label (`Button size="small"`): 13 on 20 at 530.
const BACK: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
/// A keycap's label (`keybind-v2`): 11 at 530, upper case.
const KEY: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05);

/// Under a subagent's composer (Desktop's `SubagentBar`): "✓ Done · 26m 53s", then Main
/// session with its Esc keycap. The composer's pickers above it already name the agent, model
/// and effort, and it only ever draws with them, so the line doesn't say them again. Only
/// what's known shows, one separator between neighbours.
pub fn status_line<H: SubagentsHost + 'static>(host: &H, p: &Pal, id: &str) -> Option<El> {
    if host.session_info(id).is_none_or(|i| i.parent.is_none()) {
        return None;
    }
    let back = build_back_shell(p, host.shows_esc_keycap()).on_press_tx(|s: &mut H, tx: &mut Tx| back_to_parent(s, tx));
    build_status_line(host, p, id, back)
}

fn back_to_parent(host: &mut dyn SubagentsHost, tx: &mut Tx) {
    tx.instant();
    let _ = open_parent(host, tx);
}

fn build_back_shell(p: &Pal, show_esc: bool) -> El {
    row()
        .key("back")
        .role("button")
        .label_static(opencode_i18n::t!("session.child.backToParent"))
        .h(24.0)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .focus_default(p)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(text(opencode_i18n::t!("session.child.backToParent")).font(BACK).color(p.faint))
        // The key shows only in a window at least 640 wide (`hidden sm:block`, `subagent-bar.tsx:110`).
        .children(show_esc.then(|| row().key("esc").h(14.0).min_w(14.0).px(4.0).center().radius(2.0).fill(p.layer3).child(text("ESC").font(KEY).color(p.muted))))
}

fn build_status_line(host: &dyn SubagentsHost, p: &Pal, id: &str, back: El) -> Option<El> {
    let info = host.session_info(id).filter(|i| i.parent.is_some())?;
    let running = subtree_running(host, id);
    let (glyph, status) = if running {
        (None, Some(opencode_i18n::t!("session.child.status.running")))
    } else {
        match info.outcome.as_deref() {
            Some("succeeded") => (Some(&ic::CHECK), Some(opencode_i18n::t!("session.child.status.succeeded"))),
            // Official `subagent-bar.tsx:53-55` uses `close` for both failed and interrupted runs.
            Some("failed") => (Some(&ic::OUTLINE_XMARK), Some(opencode_i18n::t!("session.child.status.failed"))),
            Some("interrupted") => (Some(&ic::OUTLINE_XMARK), Some(opencode_i18n::t!("session.child.status.interrupted"))),
            _ => (None, None),
        }
    };
    let words = |t: &str| text(t.to_owned()).font(BAR).color(p.faint).single_line();
    let mut summary = row().key("summary").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y();
    if let Some(st) = status {
        let end = if running { run_clock(host) } else { info.idle.unwrap_or(info.updated) };
        summary = summary
            .child(words(st).key("status"))
            .child(words("\u{b7}").key("dot"))
            .child(run_time(format!("bar-time-{id}"), end - info.created.min(end), BAR, p.faint));
    }
    let mut line = row().key("subagent-bar").w_full().max_w(session_view::style::dock_w()).h(32.0).pl(12.0).pr(6.0).gap(8.0).center_y();
    if status.is_some() {
        let mark = el().key("status-icon").size(16.0, 16.0).center();
        line = line.child(match glyph {
            Some(g) => mark.child(icon(g, 14.0).color(p.icon_faint)),
            None => mark.child(activity_ring_sized(p, 16.0)),
        });
    }
    Some(col().key("subagent-status").w_full().center().child(line.child(summary).child(back)))
}
