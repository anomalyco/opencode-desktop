//! When the document store next collects blobs no document (or outbox row) references, ported
//! from Desktop's `packages/desktop/src/main/storage/draft-gc-machine.ts` (lines 1-125). Any
//! document write can orphan a blob, and a sweep keeps orphans still inside the grace period,
//! so the machine stays `Scheduled` until a sweep finds none left.

use rune::machine::{Attempt, Effects, Seq};

/// Orphans left by an earlier session are collected once the window is up (`:3`).
pub const STARTUP_COLLECT_DELAY_MS: i64 = 10_000;
/// Editing a large paste retires one text chunk per save (`:6`).
pub const COLLECT_INTERVAL_MS: i64 = 60_000;
/// A blob stays safe from collection this long after its last upload or reference (`:11`).
pub const BLOB_GRACE_MS: i64 = 15 * 60_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftGcState {
    /// The last sweep left no unreferenced blob.
    Clean { attempt: Attempt, last_collected_ms: i64 },
    Scheduled { attempt: Attempt, last_collected_ms: i64, due_ms: i64 },
    Collecting { attempt: Attempt },
    Closed,
}

pub enum DraftGcEvent {
    BlobUploaded { at_ms: i64 },
    DocumentsWritten { at_ms: i64 },
    TimerFired { attempt: Attempt, at_ms: i64 },
    /// `oldest_orphan_ms`: the oldest `touched_at` among unreferenced blobs the sweep kept.
    CollectSucceeded { attempt: Attempt, at_ms: i64, oldest_orphan_ms: Option<i64> },
    CollectFailed { attempt: Attempt, at_ms: i64, error: String },
    CloseRequested,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DraftGcEffect {
    /// Replaces the armed timer, if any.
    ArmTimer { attempt: Attempt, delay_ms: i64 },
    CancelTimer,
    /// Deletes unreferenced blobs last touched before `before_ms`.
    Collect { attempt: Attempt, at_ms: i64, before_ms: i64 },
    ReportError(String),
}

/// The machine as the store opens (`initialDraftGcTransition`, `:48-50`).
pub fn initial(seq: &mut Seq, open_ms: i64, startup_delay_ms: i64) -> (DraftGcState, Effects<DraftGcEffect>) {
    schedule(seq, open_ms, open_ms + startup_delay_ms, open_ms)
}

pub fn step(seq: &mut Seq, state: &mut DraftGcState, event: DraftGcEvent) -> Effects<DraftGcEffect> {
    let (next, effects) = transition(seq, state, event);
    if let Some(next) = next {
        *state = next;
    }
    effects
}

fn transition(seq: &mut Seq, state: &DraftGcState, event: DraftGcEvent) -> (Option<DraftGcState>, Effects<DraftGcEffect>) {
    let (last_collected_ms, scheduled_due) = match state {
        DraftGcState::Closed => return (None, Effects::none()),
        _ if matches!(event, DraftGcEvent::CloseRequested) => {
            let cancel = if matches!(state, DraftGcState::Scheduled { .. }) { Effects::one(DraftGcEffect::CancelTimer) } else { Effects::none() };
            return (Some(DraftGcState::Closed), cancel);
        }
        DraftGcState::Collecting { attempt } => return collected(seq, *attempt, event),
        DraftGcState::Clean { last_collected_ms, .. } => (*last_collected_ms, None),
        DraftGcState::Scheduled { last_collected_ms, due_ms, .. } => (*last_collected_ms, Some(*due_ms)),
    };
    match event {
        DraftGcEvent::BlobUploaded { at_ms } => {
            // A scheduled sweep finds the new blob itself and schedules again while it is inside the grace period.
            if scheduled_due.is_some() {
                return (None, Effects::none());
            }
            some(schedule(seq, last_collected_ms, (last_collected_ms + COLLECT_INTERVAL_MS).max(at_ms + BLOB_GRACE_MS + 1), at_ms))
        }
        DraftGcEvent::DocumentsWritten { at_ms } => {
            // Only a document write can orphan a blob, so collect right after one when the interval allows.
            if at_ms - last_collected_ms >= COLLECT_INTERVAL_MS {
                return some(collect(seq, at_ms));
            }
            let due_ms = last_collected_ms + COLLECT_INTERVAL_MS;
            if scheduled_due.is_some_and(|d| d <= due_ms) {
                return (None, Effects::none());
            }
            some(schedule(seq, last_collected_ms, due_ms, at_ms))
        }
        DraftGcEvent::TimerFired { attempt, at_ms } if matches!(state, DraftGcState::Scheduled { attempt: a, .. } if *a == attempt) => some(collect(seq, at_ms)),
        _ => (None, Effects::none()),
    }
}

fn collected(seq: &mut Seq, current: Attempt, event: DraftGcEvent) -> (Option<DraftGcState>, Effects<DraftGcEffect>) {
    match event {
        DraftGcEvent::CollectFailed { attempt, at_ms, error } if attempt == current => {
            let (state, mut effects) = schedule(seq, at_ms, at_ms + COLLECT_INTERVAL_MS, at_ms);
            effects.push(DraftGcEffect::ReportError(error));
            (Some(state), effects)
        }
        DraftGcEvent::CollectSucceeded { attempt, at_ms, oldest_orphan_ms } if attempt == current => match oldest_orphan_ms {
            None => (Some(DraftGcState::Clean { attempt: current, last_collected_ms: at_ms }), Effects::none()),
            Some(oldest) => some(schedule(seq, at_ms, (at_ms + COLLECT_INTERVAL_MS).max(oldest + BLOB_GRACE_MS + 1), at_ms)),
        },
        _ => (None, Effects::none()),
    }
}

fn some((state, effects): (DraftGcState, Effects<DraftGcEffect>)) -> (Option<DraftGcState>, Effects<DraftGcEffect>) {
    (Some(state), effects)
}

fn schedule(seq: &mut Seq, last_collected_ms: i64, due_ms: i64, at_ms: i64) -> (DraftGcState, Effects<DraftGcEffect>) {
    let attempt = seq.next();
    (DraftGcState::Scheduled { attempt, last_collected_ms, due_ms }, Effects::one(DraftGcEffect::ArmTimer { attempt, delay_ms: (due_ms - at_ms).max(0) }))
}

fn collect(seq: &mut Seq, at_ms: i64) -> (DraftGcState, Effects<DraftGcEffect>) {
    let attempt = seq.next();
    (DraftGcState::Collecting { attempt }, Effects::two(DraftGcEffect::CancelTimer, DraftGcEffect::Collect { attempt, at_ms, before_ms: at_ms - BLOB_GRACE_MS }))
}
