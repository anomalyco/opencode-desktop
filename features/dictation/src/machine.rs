//! Explicit dictation state machine (`Idle` → `Starting` → `Listening` → `Finishing` → `Idle`
//! or `Failed`), matching the iOS composer dictation phases.

use std::sync::mpsc::Sender;

use hex_client::{Cmd, Heard, Presence, Problem};
use rune::machine::Attempt;

use crate::watch::Backend;
use crate::{Dictation, HOLD};

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
    /// Stopped at `stopped`; the words are coming, and go out with a send when Send or ⏎ (`Some`,
    /// true for ⌘⏎) finished it. The words so far stay until they land.
    Finishing { rec: Rec, heard: Option<Words>, stopped: f64, send: Option<bool> },
    /// What went wrong, until the next press or send (as iOS keeps it).
    Failed(Problem),
}

/// Words so far: `stable` won't change, `tentative` may.
#[derive(Clone)]
pub(crate) struct Words {
    pub(crate) stable: String,
    pub(crate) tentative: String,
}

/// One recording: its attempt, when the press started it, and how to reach its Hex thread
/// (`None` with the fake).
pub(crate) struct Rec {
    pub(crate) attempt: Attempt,
    pub(crate) since: f64,
    pub(crate) cmd: Option<Sender<Cmd>>,
}

/// What reaches the machine.
pub(crate) enum Event {
    /// The mic or the shortcut: starts, or finishes the recording that listens. `held`: the
    /// shortcut went down, so a long hold is push-to-talk.
    Toggle { now: f64, held: bool },
    /// The shortcut came up.
    Release { now: f64 },
    /// Send or ⏎ (`alternate` for ⌘⏎).
    Submit { now: f64, alternate: bool },
    Escape,
    /// Hex's thread (or the fake) heard something for `attempt`.
    Heard { attempt: Attempt, heard: Heard },
}

/// What the caller does about it.
pub(crate) enum Effect {
    /// Start a recording: the Hex thread for `attempt`, or the fake.
    Start(Attempt),
    /// Tell the recording's Hex thread to stop (finish) and hand over the words.
    Stop,
    /// Put the words at the caret, then send them when `Some` (true for ⌘⏎).
    Words { words: String, send: Option<bool> },
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
            let rec = Rec { attempt, since: now, cmd: None };
            let hold = held.then_some(now);
            let fake_ready = matches!(d.backend.get(), Some(Backend::Fake)) && d.prepare <= 0.0;
            d.phase = if fake_ready {
                Phase::Listening { rec, heard: None, hold }
            } else {
                Phase::Starting { rec, slow: false, hold }
            };
            (true, Some(Effect::Start(attempt)))
        }
        (Phase::Starting { rec, .. }, Event::Toggle { now, .. }) => finish(d, rec, None, now, None),
        (Phase::Listening { rec, heard, .. }, Event::Toggle { now, .. }) => finish(d, rec, heard, now, None),
        (Phase::Starting { rec, .. }, Event::Submit { now, alternate }) => finish(d, rec, None, now, Some(alternate)),
        (Phase::Listening { rec, heard, .. }, Event::Submit { now, alternate }) => finish(d, rec, heard, now, Some(alternate)),
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
        (phase @ (Phase::Starting { .. } | Phase::Listening { .. } | Phase::Finishing { .. }), Event::Escape) => {
            d.phase = phase;
            d.cancel_thread();
            d.phase = Phase::Idle;
            (true, None)
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
fn finish(d: &mut Dictation, rec: Rec, heard: Option<Words>, now: f64, send: Option<bool>) -> (bool, Option<Effect>) {
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
