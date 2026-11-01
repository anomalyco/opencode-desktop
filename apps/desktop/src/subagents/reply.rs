//! Answering a subagent from its own page. The page keeps the main composer, with the
//! subagent's agent and model in its pickers, and its status line under it: how the run went,
//! how long it took, and the way back (Esc with nothing typed).
//!
//! A reply goes to the subagent's session as its own agent and model. A builder whose worktree
//! was removed still hears it: the server moves the session to its project's checkout first
//! (`Server::send_moving_home`). A reply that fails comes back out of the timeline into the
//! composer, with a toast.

use std::collections::HashMap;

use rune::{El, Key, KeyEvent, Tx, col, el, icon, row, spacer, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use opencode_kit::client::{Prompt, Write};
use opencode_kit::model::{Message, SessionInfo};

use super::run_time;
use crate::palette::Pal;
use crate::view::{INSTANT, UI_SMALL, activity_ring};
use crate::{App, Route};

/// Replies on their way, by session: the prompt's id and the words typed.
#[derive(Default)]
pub struct Sent(pub(crate) HashMap<String, (String, String)>);

impl App {
    /// The open page's session when it's a subagent's.
    pub fn subagent_page(&self) -> Option<&SessionInfo> {
        let Route::Session(id) = &self.route else { return None };
        self.session(id).filter(|i| i.parent.is_some())
    }

    /// The write that sends `prompt` to `session`. A subagent's moves home first when its
    /// folder is gone, and is remembered until the server takes it.
    pub fn send_write(&mut self, session: &str, prompt: Prompt) -> Write {
        if !self.session(session).is_some_and(|i| i.parent.is_some()) {
            return Write::Send { session: session.to_owned(), prompt };
        }
        let words = prompt.display_text.clone().unwrap_or_else(|| prompt.text.clone());
        self.subagent_replies.0.insert(session.to_owned(), (prompt.id.clone().unwrap_or_default(), words));
        Write::SendMovingHome { session: session.to_owned(), prompt }
    }
}

/// Whether the open page is a subagent's with nothing typed in its composer.
pub fn composer_empty(s: &App) -> bool {
    s.subagent_page().is_some() && s.composer.text.trim().is_empty() && !s.attach.any() && !s.quote.any()
}

/// Esc in a subagent's empty composer: back to the session that started it.
pub fn esc(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = ev.mods;
    ev.key == Key::Escape && !m.cmd && !m.ctrl && !m.alt && !m.shift && composer_empty(s) && s.open_parent(tx)
}

/// `session.inbox.enqueued`: the server has the reply, so a later failure isn't its.
pub fn event(s: &mut App, ty: &str, data: &Value, sid: &str) {
    if ty == "session.inbox.enqueued" && s.subagent_replies.0.get(sid).is_some_and(|(id, _)| data["inboxID"] == id.as_str()) {
        s.subagent_replies.0.remove(sid);
    }
}

/// A failed write from the server's worker. When it's a reply to a subagent, the reply leaves
/// the timeline and its words go back into that subagent's composer. The caller shows the toast.
pub fn failed(s: &mut App, e: &str) {
    let Some(rest) = e.strip_prefix("POST /api/session/") else { return };
    let Some((sid, rest)) = rest.split_once('/') else { return };
    let call = rest.split([':', ' ', '?']).next().unwrap_or("");
    if !matches!(call, "prompt" | "agent" | "model" | "move" | "command") {
        return;
    }
    let Some((id, words)) = s.subagent_replies.0.remove(sid) else { return };
    if let Some(t) = s.transcripts.get_mut(sid) {
        t.messages.retain(|m| !matches!(m, Message::User { id: m, .. } if *m == id));
    }
    if let Some(inbox) = s.inboxes.get_mut(sid) {
        inbox.remove(&id);
    }
    s.attach.echoes.retain(|e| e.1 != id);
    if s.route == Route::Session(sid.to_owned()) {
        if s.composer.text.trim().is_empty() {
            s.composer.replace_text(words);
        }
    } else {
        s.restore.drafts.entry(sid.to_owned()).or_insert(words);
    }
}

/// Under a subagent's composer: "✓ Done · 26m 53s", then the way back to the main session.
/// Its agent and model are the composer's pickers.
pub fn status_line(s: &App, p: &Pal, id: &str) -> Option<El> {
    let info = s.session(id).filter(|i| i.parent.is_some())?;
    let running = s.subtree_running(id);
    let (glyph, status) = if running {
        (None, Some("Running"))
    } else {
        match info.outcome.as_deref() {
            Some("succeeded") => (Some(&ic::CHECK_SMALL), Some("Done")),
            Some("failed") => (Some(&ic::CIRCLE_X), Some("Failed")),
            Some("interrupted") => (Some(&ic::XMARK_SMALL), Some("Interrupted")),
            _ => (None, None),
        }
    };
    let mut line = row().key("subagent-bar").w_full().max_w(crate::view::DOCK_W).h(36.0).pl(12.0).pr(6.0).gap(8.0).center_y();
    if let Some(st) = status {
        let mark = el().key("status-icon").size(16.0, 16.0).center();
        line = line.child(match glyph {
            Some(g) => mark.child(icon(g, 16.0).color(p.icon_faint)),
            None => mark.child(activity_ring(p)),
        });
        let mut summary = row().key("summary").shrink().min_w(0.0).center_y().child(text(st.to_owned()).key("status").font(UI_SMALL).color(p.faint));
        if info.created > 0 {
            let end = if running { s.run_clock() } else { info.idle.unwrap_or(info.updated) };
            summary = summary
                .child(text(" · ").key("dot").font(UI_SMALL).color(p.faint))
                .child(run_time(format!("bar-time-{id}"), end - info.created, UI_SMALL, p.faint));
        }
        line = line.child(summary);
    }
    let back = row()
        .key("back")
        .role("button")
        .label("Main session")
        .h(24.0)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.open_parent(tx);
        })
        .child(text("Main session").font(UI_SMALL).color(p.faint))
        .child(row().key("esc").h(16.0).px(4.0).center().radius(4.0).fill(p.layer2).child(text("esc").font(UI_SMALL).color(p.faint)));
    Some(col().key("subagent-status").w_full().center().child(line.child(spacer()).child(back)))
}
