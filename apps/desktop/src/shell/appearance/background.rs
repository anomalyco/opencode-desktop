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
use opencode_kit::model::{Part, ToolStatus};

use crate::palette::Pal;
use crate::view::UI_MED;
use crate::{App, Route, Source};

/// The hint's accessible label: "Press ⌃B to move running work to the background".
pub fn hint_label() -> String {
    opencode_i18n::t!("session.background.moveInline", keybind = crate::keymap::Bind::Background.label())
}
const HINT_H: f32 = 24.0;
/// How long the latest blocking call runs before the hint shows, counted from when the call
/// started, so a quick command never flashes it and one already running shows it at once
/// (`message-timeline.tsx`'s `started + 300 - Date.now()`).
const HINT_DELAY_MS: i64 = 300;

#[derive(Default)]
pub struct Background {
    /// Steers sent while their session worked, by session and inbox id, until the server admits
    /// them.
    awaiting: HashSet<(String, String)>,
    /// The sessions the fixture's server moved work for, in order (`served.rs`).
    pub fixture_moves: Vec<String>,
    hint: Hint,
}

/// Whether the hint shows for the open session's latest blocking call, as Desktop's
/// `settledBlockingPartID` (`message-timeline.tsx`): it waits for the call to have run
/// [`HINT_DELAY_MS`], then shows until another call is the latest or none blocks.
#[derive(Clone, Default, PartialEq)]
enum Hint {
    #[default]
    Hidden,
    Waiting { session: String, part: String, until: i64 },
    Shown { session: String, part: String },
}

/// What a poll saw: the open session's latest blocking call, with when it started when a read
/// said, at `now`. The wake [`step`] asks for comes back as the next poll's `Seen`.
enum HintEvent {
    Seen { latest: Option<(String, String, Option<i64>)>, now: i64 },
}

#[derive(Default)]
struct HintEffects {
    changed: bool,
    wake_ms: Option<i64>,
}

fn step(state: &mut Hint, event: HintEvent) -> HintEffects {
    let HintEvent::Seen { latest, now } = event;
    let next = match (&*state, latest) {
        (_, None) => Hint::Hidden,
        (Hint::Shown { session, part }, Some((s, p, _))) if *session == s && *part == p => return HintEffects::default(),
        (Hint::Waiting { session, part, until }, Some((s, p, _))) if *session == s && *part == p => {
            if now < *until {
                return HintEffects { changed: false, wake_ms: Some(until - now) };
            }
            Hint::Shown { session: s, part: p }
        }
        (_, Some((session, part, started))) => {
            let until = started.unwrap_or(now) + HINT_DELAY_MS;
            if now < until { Hint::Waiting { session, part, until } } else { Hint::Shown { session, part } }
        }
    };
    let wake_ms = match &next {
        Hint::Waiting { until, .. } => Some(until - now),
        _ => None,
    };
    let changed = next != *state;
    *state = next;
    HintEffects { changed, wake_ms }
}

/// After the frame's events: moves the hint's machine on for the open session. True when the
/// hint's visibility changed.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let latest = match &*s.route {
        Route::Session { id, .. } => blocking_calls(s, id).last().map(|(part, started)| (id.clone(), part.clone(), *started)),
        _ => None,
    };
    let mut hint = s.appearance_state.background.hint.clone();
    let fx = step(&mut hint, HintEvent::Seen { latest, now: *s.now_ms });
    if let Some(ms) = fx.wake_ms {
        tx.wake_after(std::time::Duration::from_millis(ms.max(0) as u64));
    }
    if fx.changed {
        s.appearance_state.background.hint = hint;
    }
    fx.changed
}

/// The session's blocking work: the running shell calls and backgroundable Code Mode scripts
/// (`metadata.background == true`) of its latest unfinished reply, by part id
/// (`requests/background.ts`'s `blocking`, anomalyco/opencode#63391, #64790). Subagents always
/// run in the background, and a subagent's own session has no blocking moves.
pub fn blocking(s: &App, session: &str) -> Vec<String> {
    blocking_calls(s, session).into_iter().map(|(id, _)| id).collect()
}

/// [`blocking`], with when each call started running when a read said.
fn blocking_calls(s: &App, session: &str) -> Vec<(String, Option<i64>)> {
    if s.root_of(session) != session {
        return Vec::new();
    }
    let Some(t) = s.transcripts.get(session) else { return Vec::new() };
    let reply = t.open_reply();
    reply.map_or_else(Vec::new, |a| {
        a.parts.iter().filter_map(|p| match p {
            Part::Tool { id, name, status: ToolStatus::Running, metadata, .. }
                if name == "shell" || (name == "execute" && metadata["background"] == true) =>
            {
                Some((id.clone(), opencode_kit::model::tool_ran(metadata)))
            }
            _ => None,
        }).collect()
    })
}

impl App {
    /// Steer `id` went to `session` while it worked: once the server admits it, it moves the
    /// blocking work. The fixture admits at once.
    pub fn steered(&mut self, session: &str, id: &str) {
        match *self.source {
            Source::Fixture => {
                self.move_blocking(session, true);
            }
            Source::Live { .. } => {
                self.appearance_state.background.awaiting.insert((session.to_owned(), id.to_owned()));
            }
        }
    }

    /// ⌃B and the hint: moves the open session's blocking work, reporting a failure.
    pub fn move_to_background(&mut self, tx: &mut Tx) -> bool {
        let Route::Session { id, .. } = (*self.route).clone() else { return false };
        tx.instant();
        self.move_blocking(&id, false)
    }

    fn move_blocking(&mut self, session: &str, quiet: bool) -> bool {
        let parts = blocking(self, session);
        if parts.is_empty() {
            return false;
        }
        match *self.source {
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
            if s.appearance_state.background.awaiting.remove(&key) {
                s.move_blocking(sid, true);
            }
        }
        "session.inbox.delivered" | "session.inbox.cancelled" => {
            s.appearance_state.background.awaiting.remove(&key);
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
    let Route::Session { id, .. } = (*s.route).clone() else { return false };
    if blocking(s, &id).is_empty() {
        return false;
    }
    s.move_to_background(tx);
    true
}

/// "Move to background ⌃B" beside Working once the open run's latest blocking call has run a
/// moment ([`Hint`]), fading in and out as Desktop's does. The lane's row places it, 8 after
/// Working's words and centred on them, or at its start while a thought stands in for them
/// (`session_view::timeline::parts::status_row`). Desktop's small ghost-faint
/// `Button` (`message-timeline.tsx:57-68`, `button.css:6-26,46-50,304-322`): 24 tall, 9 a side,
/// 4 corners, 6 between its words and keys, 13 px at 530 in `text-faint`, `overlay-hover` under
/// the pointer and `overlay-pressed` held, with no fade; its keys the neutral `Keybind`.
pub fn hint(s: &App, p: &Pal, session: &str, busy: bool) -> Option<El> {
    let Hint::Shown { session: shown, part } = &s.appearance_state.background.hint else { return None };
    if !busy || shown != session || blocking(s, session).last() != Some(part) {
        return None;
    }
    let hint = row()
        .key("background-hint")
        .role("button")
        .label(hint_label())
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

