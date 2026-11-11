//! One prompt's way to the server, as an explicit machine. Desktop's counterparts:
//! `composer/machine.ts` (`handing-off`, then `failSubmission`/`restoreSubmission` in
//! `composer/submit.ts:595-614`), `session/handoff-machine.ts` (the shown prompt waits for the
//! server's admission echo) and `composer/prompt-status-machine.ts` (`admitted` / `failed`). The
//! durable outbox, the retries and the receipt check are Rune's own (`send_failed.rs`): Desktop
//! puts a failed prompt back in the composer, where Rune keeps it in the outbox and sends it again.
//!
//! A journaled prompt's row is in `opencode.sqlite` (`outbox.rs`), and its write only ever leaves
//! from the storage thread once the row that says `Sending` is durable: a prompt is never on the
//! wire without a row that makes the next launch check for it, so none is lost or sent twice.

use rune::machine::Effects;

use super::outbox::Operation;
use super::{AfterCheck, Sent};

/// Where a prompt is on its way. Only `Sending` and `Unconfirmed` can hold an Edit or Delete the
/// user asked for while the server's answer was still out (`then`); every other phase acts on
/// one at once.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Saved, not sent: the outbox's next pump sends it. `why` says what it waits for (empty:
    /// only its turn).
    Waiting { why: String },
    /// On its way through the writer of window `by` (`windows::peer::token`; 0 is a window of
    /// its own, a fixture or story), as hand-off number `handoff`: its row commits, then the
    /// storage thread gives the write to the writer (`NotSaved` when the commit failed and the
    /// write never went, `NotSent` when the writer was gone).
    Sending { by: u64, then: Option<AfterCheck>, handoff: u32 },
    /// Its reply was lost: a receipt check decides whether the server has it. `why` is what the
    /// last check or failure said.
    Unconfirmed { why: String, then: Option<AfterCheck> },
    /// The server refused it, or a check found it never arrived: the user retries, edits or
    /// deletes it. `restored` when its words went back into the composer, so Enter on them as
    /// they are is Retry.
    Failed { why: String, restored: bool },
    /// The server has it.
    Accepted,
}

/// How a failed write ended, as far as its error proves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A 4xx other than 408, 425 or 429: the server refused this very prompt.
    Rejected,
    /// It never reached the server (connection refused before a start began, say).
    Unsent,
    /// Anything else: the server may have it.
    Lost,
}

/// What a receipt check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// In the inbox, or delivered as a message.
    Arrived,
    /// The session exists and has neither.
    Missing,
    /// The session itself isn't there (404).
    NoSession,
    /// The server couldn't answer; `why` is its error in words.
    Unknown(&'static str),
}

pub enum Event {
    /// The outbox's pump hands it to window `by`'s writer.
    Send { by: u64 },
    /// Hand-off `handoff`'s writer couldn't take it (the window's link is gone).
    NotSent { why: &'static str, handoff: u32 },
    /// Its row couldn't be committed: hand-off `handoff`'s write never went, or (`handoff` none)
    /// the row that holds it waiting was never saved. Either way nothing keeps it across a
    /// launch, so its words go back to the composer.
    NotSaved { handoff: Option<u32> },
    WriteFailed { why: String, verdict: Verdict },
    /// The server has it: its admission echo, a page or inbox listing with its id, the write's
    /// own reply, or a receipt.
    Admitted,
    Checked(Found),
    /// Window `by` closed (or left the server) with its write still out.
    Released { by: u64 },
    /// Read back from the journal at launch: a write out when the app quit may have landed.
    Reopened,
    /// Retry now, from the bubble or its menu.
    Retry,
    /// Edit in composer, or Delete, from its menu.
    Act(AfterCheck),
    /// Its Edit waits for the user to come back to its session.
    Parked,
    /// Its words went back into an empty composer.
    Restored,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// Write its row (journaled prompts only).
    Save,
    /// Ask the server for receipts of every unconfirmed prompt.
    Check,
    /// Back off before the outbox pumps again.
    Defer,
    /// Show it as not sent, back in the timeline (out of the inbox).
    Outbox,
    /// Show it as failed in the timeline (`App::fail_send`).
    ShowFailed,
    /// Put its words back where the user will see them (`App::restore_rejection`).
    Restore,
    /// Carry out an Edit or Delete the user asked for.
    Run(AfterCheck),
    /// Accepted: reset the backoff; `late` when an Edit or Delete waited on it, which the server
    /// beat ("Message already sent").
    Accepted { late: bool },
}

pub fn step(s: &mut Sent, event: Event) -> Effects<Effect> {
    let journaled = s.journaled;
    match (&mut s.phase, event) {
        (Phase::Accepted, _) => Effects::none(),
        (_, Event::Admitted) => {
            let late = matches!(&s.phase, Phase::Sending { then: Some(_), .. } | Phase::Unconfirmed { then: Some(_), .. });
            s.phase = Phase::Accepted;
            Effects::two(Effect::Save, Effect::Accepted { late })
        }
        // The pump commits its row, and the storage thread hands the write over once it's durable.
        (Phase::Waiting { .. }, Event::Send { by }) => {
            s.handoffs += 1;
            s.phase = Phase::Sending { by, then: None, handoff: s.handoffs };
            Effects::none()
        }
        (Phase::Sending { then, handoff, .. }, Event::NotSent { why, handoff: h }) if *handoff == h => {
            let then = then.take();
            // Its row still says Sending: a launch before the next hand-off checks for it first.
            s.phase = Phase::Waiting { why: why.into() };
            then.map_or(Effects::none(), |action| Effects::one(Effect::Run(action)))
        }
        (Phase::Sending { then, handoff, .. }, Event::NotSaved { handoff: Some(h) }) if *handoff == h => {
            let then = then.take();
            reject(s, "Couldn't save message".into(), then)
        }
        (Phase::Waiting { .. }, Event::NotSaved { handoff: None }) => reject(s, "Couldn't save message".into(), None),
        (Phase::Sending { then, .. }, Event::WriteFailed { why, verdict }) => {
            let then = then.take();
            failed(s, why, verdict, then)
        }
        (Phase::Sending { by, then, .. }, Event::Released { by: gone }) if *by == gone => {
            s.phase = Phase::Unconfirmed { why: String::new(), then: then.take() };
            Effects::two(Effect::Save, Effect::Check)
        }
        (Phase::Sending { then, .. }, Event::Reopened) => {
            s.phase = Phase::Unconfirmed { why: String::new(), then: then.take() };
            Effects::none()
        }
        (Phase::Unconfirmed { then, .. }, Event::Checked(found)) => {
            let then = then.take();
            checked(s, found, then)
        }
        (Phase::Failed { .. }, Event::Retry) => {
            s.phase = Phase::Waiting { why: String::new() };
            Effects::one(Effect::Save)
        }
        (Phase::Waiting { .. } | Phase::Failed { .. }, Event::Act(action)) => Effects::one(Effect::Run(action)),
        (Phase::Sending { then, .. }, Event::Act(action)) => {
            *then = Some(action);
            Effects::one(Effect::Save)
        }
        (Phase::Unconfirmed { then, .. }, Event::Act(action)) => {
            *then = Some(action);
            Effects::two(Effect::Save, Effect::Check)
        }
        (Phase::Waiting { .. } | Phase::Failed { .. }, Event::Parked) => {
            s.phase = Phase::Failed { why: "Ready to edit".into(), restored: false };
            Effects::one(Effect::Save)
        }
        (Phase::Failed { restored, .. }, Event::Restored) if journaled => {
            *restored = true;
            Effects::none()
        }
        _ => Effects::none(),
    }
}

/// A write that failed. A local prompt (no journal) fails at once when it surely never arrived
/// and is checked otherwise; a journaled one fails only when the server refused it, else waits
/// for the connection or a receipt.
fn failed(s: &mut Sent, why: String, verdict: Verdict, then: Option<AfterCheck>) -> Effects<Effect> {
    if verdict == Verdict::Rejected || (!s.journaled && verdict == Verdict::Unsent) {
        return reject(s, why, then);
    }
    if !s.journaled {
        s.phase = Phase::Unconfirmed { why, then };
        return Effects::one(Effect::Check);
    }
    // Not sent: an Edit or Delete waiting on it goes ahead. Maybe sent: it waits for the receipt.
    let mut effects = Effects::one(Effect::Outbox);
    match (verdict, then) {
        (Verdict::Unsent, Some(action)) => {
            s.phase = Phase::Waiting { why };
            effects.push(Effect::Run(action));
        }
        (Verdict::Unsent, None) => s.phase = Phase::Waiting { why },
        (_, then) => s.phase = Phase::Unconfirmed { why, then },
    }
    effects.push(Effect::Defer);
    effects.push(Effect::Save);
    effects
}

/// Failed for good. A journaled prompt's words go back to the composer (or its Edit or Delete
/// runs); a local one keeps its bubble with Retry and Edit.
fn reject(s: &mut Sent, why: String, then: Option<AfterCheck>) -> Effects<Effect> {
    s.phase = Phase::Failed { why, restored: false };
    if !s.journaled {
        return Effects::one(Effect::ShowFailed);
    }
    Effects::Many(vec![Effect::ShowFailed, then.map_or(Effect::Restore, Effect::Run), Effect::Save])
}

fn checked(s: &mut Sent, found: Found, then: Option<AfterCheck>) -> Effects<Effect> {
    match found {
        Found::Arrived => {
            s.phase = Phase::Accepted;
            Effects::two(Effect::Save, Effect::Accepted { late: then.is_some() })
        }
        Found::Missing if !s.journaled => reject(s, String::new(), then),
        // A revert commit isn't idempotent: never replay that destructive step.
        Found::Missing if matches!(s.operation, Operation::Revert) => {
            s.operation = Operation::Send;
            reject(s, "Check the rewind before resending".into(), then)
        }
        Found::Missing => {
            // The session exists, so a create went through before its reply was lost.
            if s.operation.creates() {
                s.operation = Operation::Send;
            }
            waiting(s, then)
        }
        Found::NoSession if matches!(s.operation, Operation::Start(_)) => waiting(s, then),
        Found::NoSession if s.journaled => {
            let why = if s.operation.creates() { "Couldn't confirm the new worktree" } else { "Session not found" };
            reject(s, why.into(), then)
        }
        Found::NoSession => {
            s.phase = Phase::Unconfirmed { why: String::new(), then };
            Effects::none()
        }
        // No receipt isn't a missing receipt: keep it unconfirmed, and never send it here.
        Found::Unknown(why) => {
            let changed = !matches!(&s.phase, Phase::Unconfirmed { why: was, .. } if was == why);
            s.phase = Phase::Unconfirmed { why: if s.journaled { why.into() } else { String::new() }, then };
            if changed && s.journaled { Effects::one(Effect::Save) } else { Effects::none() }
        }
    }
}

fn waiting(s: &mut Sent, then: Option<AfterCheck>) -> Effects<Effect> {
    s.phase = Phase::Waiting { why: String::new() };
    match then {
        Some(action) => Effects::two(Effect::Save, Effect::Run(action)),
        None => Effects::one(Effect::Save),
    }
}
