//! Explicit dictation state machine (`Idle` → `Starting` → `Listening` → `Finishing` → `Idle`
//! or `Failed`), matching the iOS composer dictation phases.

use std::sync::mpsc::Sender;

use hex_client::{Cmd, Heard, Presence, Problem};
use rune::machine::Attempt;

use crate::watch::Backend;
use crate::{Dictation, FINISH_LIMIT, HOLD, PREPARING_AFTER, SubmitKind};

/// Where a dictation is, the machine every press, key and word from Hex goes through ([`step`]).
/// A press goes `Idle` (or `Failed`) → `Starting` → `Listening` → `Finishing` → `Idle` with the
/// words at the caret; Esc goes back to `Idle` from any of the three, and Hex or the microphone
/// refusing goes to `Failed`. Desktop has no dictation of its own; the phases are the iOS
/// app's, and each carries only what it needs: a recording's attempt fences what an earlier
/// one's thread still says, and a held shortcut's press time lives only while the microphone
/// is open.
pub(crate) enum Phase {
    Idle,
    /// Pressed: Hex is starting, or taking the microphone. `slow` once that has taken
    /// `PREPARING_AFTER`, so "Preparing dictation…" shows.
    Starting { rec: Rec, slow: bool, hold: Option<f64> },
    /// Hex is listening; `heard` is the words so far, when its model sends them. `hold`: when
    /// the shortcut that started it went down, while it is still held.
    Listening { rec: Rec, heard: Option<Words>, hold: Option<f64> },
    /// Stopped at `stopped`; the words are coming, and go out with a send when Send or ⏎ (`Some`)
    /// finished it. The words so far stay until they land.
    Finishing { rec: Rec, heard: Option<Words>, stopped: f64, send: Option<SubmitKind> },
    /// What went wrong, until the next press or send (as iOS keeps it).
    Failed(Problem),
}

/// Words so far: `stable` won't change, `tentative` may.
#[derive(Clone)]
pub(crate) struct Words {
    pub(crate) stable: String,
    pub(crate) tentative: String,
}

/// Which backend drives an active recording.
pub(crate) enum RecBackend {
    /// In-process fake Hex for headless runs, scenes, fixtures, and storybook.
    Fake,
    /// Hex backend selected; waiting for its worker thread to attach via [`Event::ThreadStarted`].
    Starting,
    /// Live Hex thread with its command channel and whether it holds a window waker.
    Hex { cmd: Sender<Cmd>, wakes: bool },
}

/// One recording: its attempt, when the press started it, and its backend transport.
pub(crate) struct Rec {
    pub(crate) attempt: Attempt,
    pub(crate) since: f64,
    pub(crate) backend: RecBackend,
}

impl Rec {
    pub(crate) fn is_fake(&self) -> bool {
        matches!(self.backend, RecBackend::Fake)
    }

    pub(crate) fn wakes(&self) -> bool {
        matches!(self.backend, RecBackend::Hex { wakes: true, .. })
    }

    pub(crate) fn cmd(&self) -> Option<&Sender<Cmd>> {
        match &self.backend {
            RecBackend::Hex { cmd, .. } => Some(cmd),
            RecBackend::Fake | RecBackend::Starting => None,
        }
    }

    pub(crate) fn into_cmd(self) -> Option<Sender<Cmd>> {
        match self.backend {
            RecBackend::Hex { cmd, .. } => Some(cmd),
            RecBackend::Fake | RecBackend::Starting => None,
        }
    }
}

/// What reaches the machine.
pub(crate) enum Event {
    /// The mic or the shortcut: starts, or finishes the recording that listens. `held`: the
    /// shortcut went down, so a long hold is push-to-talk.
    Toggle { now: f64, held: bool },
    /// The shortcut came up.
    Release { now: f64 },
    /// Send or ⏎ (`kind` for ⌘⏎ / Alt+⏎).
    Submit { now: f64, kind: SubmitKind },
    Escape,
    /// The Hex worker thread for `attempt` started, with its command channel and waker flag.
    ThreadStarted { attempt: Attempt, cmd: Sender<Cmd>, wakes: bool },
    /// A frame tick at `now`: marks slow starts and times out stalled finishes when no new
    /// events arrived (`heard_empty`).
    Tick { now: f64, heard_empty: bool },
    /// Clears a failed dictation error back to idle.
    DismissError,
    /// Cancels any active recording and transitions to `Failed`.
    Fail(Problem),
    /// Hex's thread (or the fake) heard something for `attempt`.
    Heard { attempt: Attempt, heard: Heard },
}

/// What the caller does about it.
pub(crate) enum Effect {
    /// Start the Hex worker thread for `attempt`.
    Start(Attempt),
    /// Tell the recording's Hex thread to stop (finish) and hand over the words.
    Stop,
    /// Cancel a recording's Hex thread after leaving an active phase.
    Cancel(Sender<Cmd>),
    /// Put the words at the caret, then send them when `Some`.
    Words { words: String, send: Option<SubmitKind> },
    /// Finish timed out: cancel the Hex thread if any and insert any partial words heard so far.
    TimedOut { words: String, cancel: Option<Sender<Cmd>> },
}

/// Whether [`Event::Tick`] at `now` would transition `d` (lets `poll` skip mutating `Tracked`).
pub(crate) fn tick_due(d: &Dictation, now: f64, heard_empty: bool) -> bool {
    match &d.phase {
        Phase::Starting { rec, slow: false, .. } => now - rec.since >= PREPARING_AFTER,
        Phase::Finishing { stopped, .. } => heard_empty && now + 1e-9 >= stopped + FINISH_LIMIT,
        _ => false,
    }
}

/// Whether the machine took the event.
pub(crate) fn step(d: &mut Dictation, event: Event) -> (bool, Option<Effect>) {
    match (std::mem::replace(&mut d.phase, Phase::Idle), event) {
        (Phase::Idle | Phase::Failed(_), Event::Toggle { now, held }) => {
            // Off Windows the watcher knows; elsewhere the press's thread finds out.
            if d.presence == Some(Presence::Missing) {
                d.phase = Phase::Failed(if hex_client::MIC_WITHOUT_HEX {
                    Problem::not_installed()
                } else {
                    Problem::of(hex_client::NOT_RUNNING)
                });
                return (true, None);
            }
            let attempt = d.seq.next();
            let fake = matches!(d.backend.get(), Some(Backend::Fake));
            let rec = Rec {
                attempt,
                since: now,
                backend: if fake { RecBackend::Fake } else { RecBackend::Starting },
            };
            let hold = held.then_some(now);
            d.phase = if fake && d.prepare <= 0.0 {
                Phase::Listening { rec, heard: None, hold }
            } else {
                Phase::Starting { rec, slow: false, hold }
            };
            (true, (!fake).then_some(Effect::Start(attempt)))
        }
        (Phase::Starting { rec, .. }, Event::Toggle { now, .. }) => finish(d, rec, None, now, None),
        (Phase::Listening { rec, heard, .. }, Event::Toggle { now, .. }) => finish(d, rec, heard, now, None),
        (Phase::Starting { rec, .. }, Event::Submit { now, kind }) => finish(d, rec, None, now, Some(kind)),
        (Phase::Listening { rec, heard, .. }, Event::Submit { now, kind }) => finish(d, rec, heard, now, Some(kind)),
        // A hold past `HOLD` was push-to-talk, so the recording finishes as the key comes up.
        (Phase::Starting { rec, hold: Some(at), .. }, Event::Release { now }) if now - at >= HOLD => finish(d, rec, None, now, None),
        (Phase::Listening { rec, heard, hold: Some(at) }, Event::Release { now }) if now - at >= HOLD => finish(d, rec, heard, now, None),
        (Phase::Starting { rec, slow, hold: Some(_) }, Event::Release { .. }) => {
            d.phase = Phase::Starting { rec, slow, hold: None };
            (true, None)
        }
        (Phase::Listening { rec, heard, hold: Some(_) }, Event::Release { .. }) => {
            d.phase = Phase::Listening { rec, heard, hold: None };
            (true, None)
        }
        // While the words come, the mic and Send wait for them.
        (phase @ Phase::Finishing { .. }, Event::Toggle { .. } | Event::Submit { .. }) => {
            d.phase = phase;
            (true, None)
        }
        // A send clears what went wrong, as iOS's does, and goes ahead.
        (Phase::Failed(_), Event::Submit { .. }) => (false, None),
        (Phase::Failed(_), Event::DismissError) => (true, None),
        (phase, Event::Fail(why)) => {
            let cancel = match phase {
                Phase::Starting { rec, .. } | Phase::Listening { rec, .. } | Phase::Finishing { rec, .. } => rec.into_cmd(),
                Phase::Idle | Phase::Failed(_) => None,
            };
            d.phase = Phase::Failed(why);
            (true, cancel.map(Effect::Cancel))
        }
        (Phase::Starting { rec, .. } | Phase::Listening { rec, .. } | Phase::Finishing { rec, .. }, Event::Escape) => {
            (true, rec.into_cmd().map(Effect::Cancel))
        }
        (mut phase, Event::ThreadStarted { attempt, cmd, wakes }) => {
            let matched = match &mut phase {
                Phase::Starting { rec, .. } | Phase::Listening { rec, .. } | Phase::Finishing { rec, .. } if rec.attempt == attempt => {
                    rec.backend = RecBackend::Hex { cmd, wakes };
                    true
                }
                _ => false,
            };
            d.phase = phase;
            (matched, None)
        }
        (Phase::Starting { rec, slow: false, hold }, Event::Tick { now, .. }) if now - rec.since >= PREPARING_AFTER => {
            d.phase = Phase::Starting { rec, slow: true, hold };
            (true, None)
        }
        (Phase::Finishing { rec, heard, stopped, .. }, Event::Tick { now, heard_empty: true }) if now + 1e-9 >= stopped + FINISH_LIMIT => {
            let words = match heard {
                Some(w) => format!("{}{}", w.stable, w.tentative),
                None => String::new(),
            };
            let says = if words.trim().is_empty() {
                opencode_i18n::t!("rune.dictation.timedOutEmpty")
            } else {
                opencode_i18n::t!("rune.dictation.timedOut")
            };
            d.phase = Phase::Failed(Problem { says: says.to_owned(), fix: None });
            (true, Some(Effect::TimedOut { words, cancel: rec.into_cmd() }))
        }
        (phase, Event::Heard { attempt, heard }) => {
            if d_attempt(&phase) != Some(attempt) {
                d.phase = phase;
                return (false, None);
            }
            hear(d, phase, heard)
        }
        (phase, _) => {
            d.phase = phase;
            (false, None)
        }
    }
}

/// Into `Finishing`: the microphone stops and the words are coming.
fn finish(d: &mut Dictation, rec: Rec, heard: Option<Words>, now: f64, send: Option<SubmitKind>) -> (bool, Option<Effect>) {
    d.phase = Phase::Finishing { rec, heard, stopped: now, send };
    (true, Some(Effect::Stop))
}

fn d_attempt(phase: &Phase) -> Option<Attempt> {
    match phase {
        Phase::Starting { rec, .. } | Phase::Listening { rec, .. } | Phase::Finishing { rec, .. } => Some(rec.attempt),
        Phase::Idle | Phase::Failed(_) => None,
    }
}

/// One thing the recording's thread heard, moving the phase on.
fn hear(d: &mut Dictation, phase: Phase, h: Heard) -> (bool, Option<Effect>) {
    d.phase = match (phase, h) {
        (Phase::Starting { rec, hold, .. }, Heard::Ready) => Phase::Listening { rec, heard: None, hold },
        (Phase::Listening { rec, hold, .. }, Heard::Partial { stable, tentative }) => {
            Phase::Listening { rec, heard: Some(Words { stable, tentative }), hold }
        }
        (Phase::Finishing { rec, stopped, send, .. }, Heard::Partial { stable, tentative }) => {
            Phase::Finishing { rec, heard: Some(Words { stable, tentative }), stopped, send }
        }
        (phase, Heard::Words(words)) => {
            let send = match phase {
                Phase::Finishing { send, .. } => send,
                _ => None,
            };
            return (true, Some(Effect::Words { words, send }));
        }
        (_, Heard::Failed(why)) => Phase::Failed(why),
        (phase, _) => phase,
    };
    (true, None)
}
