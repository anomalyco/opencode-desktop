//! Moving a run's blocking work to the background, as OpenCode Desktop does
//! (`session/requests/steer-background.ts`, `requests/model.ts`, `timeline/message-timeline.tsx`).
//!
//! A steer moves the run's blocking shells and subagents to the background, so the agent reads
//! it at its next step. The move follows admission: the steer already waits in the inbox when
//! the moved work ends the step. It reads the blocking work once admitted, so work that started
//! meanwhile moves too, and makes no call when nothing blocks. Nothing is claimed: the server
//! moves each job once and ignores repeats. A steer's move drops its errors; ⌃B and the "Move
//! to background" hint beside Working report theirs.

use std::collections::HashSet;

use rune::{El, KeyEvent, Tx, row, text};

use opencode_kit::client::Write;
use opencode_kit::model::{Message, Part, ToolStatus};

use crate::palette::Pal;
use crate::view::UI;
use crate::{App, Route, Source};

pub const LABEL: &str = "Move to background";
pub const HINT: &str = "Press ⌃B to move running work to the background";
pub const FAILED: &str = "Request failed";

#[derive(Default)]
pub struct Background {
    /// Steers sent while their session worked, by session and inbox id, until the server admits
    /// them.
    awaiting: HashSet<(String, String)>,
    /// The sessions the fixture's server moved work for, in order (`served.rs`).
    pub fixture_moves: Vec<String>,
}

/// The session's blocking work: the running shell and subagent calls of its latest unfinished
/// reply, by part id. A subagent's own session has none: moving stays with the primary session.
pub fn blocking(s: &App, session: &str) -> Vec<String> {
    if s.root_of(session) != session {
        return Vec::new();
    }
    let Some(t) = s.transcripts.get(session) else { return Vec::new() };
    let reply = t.messages.iter().rev().find_map(|m| match m {
        Message::Assistant(a) if a.completed.is_none() => Some(a),
        _ => None,
    });
    reply.map_or_else(Vec::new, |a| {
        a.parts.iter().filter_map(|p| match p {
            Part::Tool { id, name, status: ToolStatus::Running, .. } if name == "shell" || name == "subagent" => Some(id.clone()),
            _ => None,
        }).collect()
    })
}

impl App {
    /// Steer `id` went to `session` while it worked: once the server admits it, it moves the
    /// blocking work. The fixture admits at once.
    pub fn steered(&mut self, session: &str, id: &str) {
        match self.source {
            Source::Fixture => {
                self.move_blocking(session, true);
            }
            Source::Live { .. } => {
                self.background.awaiting.insert((session.to_owned(), id.to_owned()));
            }
        }
    }

    /// ⌃B and the hint: moves the open session's blocking work, reporting a failure.
    pub fn move_to_background(&mut self, tx: &mut Tx) -> bool {
        let Route::Session(id) = self.route.clone() else { return false };
        tx.instant();
        self.move_blocking(&id, false)
    }

    fn move_blocking(&mut self, session: &str, quiet: bool) -> bool {
        let parts = blocking(self, session);
        if parts.is_empty() {
            return false;
        }
        match self.source {
            Source::Fixture => {
                self.fixture_background(session, &parts);
                true
            }
            Source::Live { .. } => self.write(Write::Background { session: session.to_owned(), quiet }),
        }
    }
}

/// `session.inbox.*`: the server admitted a steer this app sent (`enqueued`, or a queued prompt
/// turned into a steer), so its move runs now.
pub fn admitted(s: &mut App, ty: &str, data: &serde_json::Value, sid: &str) {
    let Some(id) = data["inboxID"].as_str() else { return };
    let key = (sid.to_owned(), id.to_owned());
    match ty {
        "session.inbox.enqueued" | "session.inbox.delivery.changed" if data["delivery"].as_str() != Some("queue") && data["item"]["delivery"].as_str() != Some("queue") => {
            if s.background.awaiting.remove(&key) {
                s.move_blocking(sid, true);
            }
        }
        "session.inbox.delivered" | "session.inbox.cancelled" => {
            s.background.awaiting.remove(&key);
        }
        _ => {}
    }
}

/// A status line from the server: a failed ⌃B or hint move reads "Request failed", as
/// Desktop's toast does. A steer's move never reports.
pub fn status(e: String) -> String {
    let moving = e.starts_with("POST /api/session/") && e.split(':').next().is_some_and(|p| p.trim_end().ends_with("/background"));
    if moving { FAILED.to_owned() } else { e }
}

/// ⌃B, Desktop's `session.background` keybind, while the open session has blocking work. With
/// none it passes on, so a text field's ⌃B still moves the caret back.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = ev.mods;
    let ctrl_b = matches!(&ev.key, rune::Key::Text(t) if t.eq_ignore_ascii_case("b")) && m.ctrl && !m.cmd && !m.alt && !m.shift;
    if !ctrl_b {
        return false;
    }
    let Route::Session(id) = s.route.clone() else { return false };
    if blocking(s, &id).is_empty() {
        return false;
    }
    s.move_to_background(tx);
    true
}

/// "Move to background ⌃B" beside Working while the open run has blocking work, as Desktop
/// shows it in the same lane, fading in and out as Desktop's does.
pub fn hint(s: &App, p: &Pal, session: &str, busy: bool) -> Option<El> {
    if !busy || blocking(s, session).is_empty() {
        return None;
    }
    // Working's ring, its gap and its label, then the lane's gap.
    let x = 14.0 + 8.0 + crate::working::width(s.working.label()) + 8.0;
    let hint = row()
        .key("background-hint")
        .role("button")
        .label(HINT)
        .absolute(x, -3.0)
        .h(22.0)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT)
        .transition(rune::Transition::opacity())
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            s.move_to_background(tx);
        })
        .child(text(LABEL).font(UI).color(p.faint))
        .child(crate::queue::keycap(p, "⌃"))
        .child(crate::queue::keycap(p, "B"));
    Some(hint)
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
