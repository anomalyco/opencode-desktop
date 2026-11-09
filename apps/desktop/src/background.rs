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
use crate::view::UI_MED;
use crate::{App, Route, Source};


/// The hint's accessible label: "Press ⌃B to move running work to the background".
pub fn hint_label() -> String {
    opencode_i18n::t!("session.background.moveInline", keybind = crate::keymap::Bind::Background.label())
}
const HINT_H: f32 = 24.0;

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

/// ⌃B, Desktop's `session.background` keybind, while the open session has blocking work. With
/// none it passes on, so a text field's ⌃B still moves the caret back.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !crate::keymap::Bind::Background.matches_tx(ev, tx) {
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
/// shows it in the same lane, fading in and out as Desktop's does. Desktop's small ghost-faint
/// `Button` (`message-timeline.tsx:57-68`, `button.css:6-26,46-50,304-322`): 24 tall, 9 a side,
/// 4 corners, 6 between its words and keys, 13 px at 530 in `text-faint`, `overlay-hover` under
/// the pointer and `overlay-pressed` held, with no fade; its keys the neutral `Keybind`.
pub fn hint(s: &App, p: &Pal, session: &str, busy: bool) -> Option<El> {
    if !busy || blocking(s, session).is_empty() {
        return None;
    }
    // Working's label, which starts the column, then the lane's gap; the lane's start while
    // Working hides for a question or a permission.
    let x = if session_view::TimelineHost::session_asking(s, session) { 0.0 } else { crate::working::width(s.working.label()) + 8.0 };
    let y = crate::working::LANE_PAD + (crate::working::H - HINT_H) / 2.0;
    let hint = row()
        .key("background-hint")
        .role("button")
        .label(hint_label())
        .absolute(x, y)
        .h(HINT_H)
        .px(9.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT)
        .transition(rune::Transition::opacity())
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            s.move_to_background(tx);
        })
        .child(text(opencode_i18n::t!("session.background.moveRunning")).font(UI_MED).color(p.faint))
        .child(crate::keymap::keycaps(p, crate::keymap::Bind::Background));
    Some(hint)
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
