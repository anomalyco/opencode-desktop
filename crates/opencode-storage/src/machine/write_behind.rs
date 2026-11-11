//! A write-behind buffer, ported from Desktop's `packages/desktop/src/main/storage/write-behind-machine.ts`
//! (lines 9-214): rows wait in `Scheduled` for the timer, are written in one batch in
//! `Flushing`, and go back to `Scheduled` with a longer delay when the write fails, so no
//! acknowledged row is left without a pending write. The [`Attempt`] names the current timer
//! and write; a result for another attempt is stale.

use std::hash::Hash;

use indexmap::IndexMap;
use rune::machine::{Attempt, Effects, Seq};

/// A run of failed writes backs off up to `2 ** 6 = 64` times the base delay (`:2`).
pub const MAX_RETRY_EXPONENT: u32 = 6;

#[derive(Debug, Default)]
pub enum WriteBehindState<K, V> {
    #[default]
    Idle,
    Scheduled {
        pending: IndexMap<K, V>,
        attempt: Attempt,
        /// Consecutive failed writes, for the retry backoff.
        failures: u32,
    },
    Flushing {
        in_flight: IndexMap<K, V>,
        /// Rows set while the batch is written; they win over `in_flight` for the same key.
        queued: IndexMap<K, V>,
        attempt: Attempt,
        failures: u32,
        /// The buffer closes once this write settles.
        closing: bool,
    },
    Closed,
}

/// Which rows a [WriteBehindEvent::Drop] strips.
pub type Matcher<K, V> = Box<dyn Fn(&K, &V) -> bool>;

pub enum WriteBehindEvent<K, V> {
    Set { key: K, value: V },
    /// Strips the rows `drop` matches from what waits and what is being written, so a cleared
    /// row never comes back when a failed batch is queued again (`:92-100, 135-142`).
    Drop(Matcher<K, V>),
    TimerFired { attempt: Attempt },
    FlushRequested,
    CloseRequested,
    WriteSucceeded { attempt: Attempt },
    WriteFailed { attempt: Attempt, error: String },
}

#[derive(Debug)]
pub enum WriteBehindEffect<K, V> {
    /// Replaces the armed timer, if any.
    ArmTimer { attempt: Attempt, delay_ms: u64 },
    CancelTimer,
    WriteBatch { attempt: Attempt, batch: IndexMap<K, V> },
    ReportError(String),
    /// A row set after closing began: the caller must not acknowledge it.
    RejectClosed,
}

pub fn step<K: Hash + Eq + Clone, V: Clone>(seq: &mut Seq, state: &mut WriteBehindState<K, V>, event: WriteBehindEvent<K, V>, delay_ms: u64) -> Effects<WriteBehindEffect<K, V>> {
    match std::mem::take(state) {
        WriteBehindState::Closed => {
            *state = WriteBehindState::Closed;
            match event {
                WriteBehindEvent::Set { .. } => Effects::one(WriteBehindEffect::RejectClosed),
                _ => Effects::none(),
            }
        }
        WriteBehindState::Idle => match event {
            WriteBehindEvent::CloseRequested => {
                *state = WriteBehindState::Closed;
                Effects::none()
            }
            WriteBehindEvent::Set { key, value } => {
                let attempt = seq.next();
                *state = WriteBehindState::Scheduled { pending: IndexMap::from([(key, value)]), attempt, failures: 0 };
                Effects::one(WriteBehindEffect::ArmTimer { attempt, delay_ms })
            }
            _ => Effects::none(),
        },
        WriteBehindState::Scheduled { pending, attempt, failures } => scheduled(seq, state, pending, attempt, failures, event),
        WriteBehindState::Flushing { in_flight, queued, attempt, failures, closing } => flushing(seq, state, Flushing { in_flight, queued, attempt, failures, closing }, event, delay_ms),
    }
}

fn scheduled<K: Hash + Eq + Clone, V: Clone>(seq: &mut Seq, state: &mut WriteBehindState<K, V>, mut pending: IndexMap<K, V>, attempt: Attempt, failures: u32, event: WriteBehindEvent<K, V>) -> Effects<WriteBehindEffect<K, V>> {
    match event {
        WriteBehindEvent::Set { key, value } => {
            pending.insert(key, value);
            *state = WriteBehindState::Scheduled { pending, attempt, failures };
            Effects::none()
        }
        WriteBehindEvent::Drop(matches) => {
            pending.retain(|k, v| !matches(k, v));
            if pending.is_empty() {
                *state = WriteBehindState::Idle;
                return Effects::one(WriteBehindEffect::CancelTimer);
            }
            *state = WriteBehindState::Scheduled { pending, attempt, failures };
            Effects::none()
        }
        WriteBehindEvent::TimerFired { attempt: fired } if fired != attempt => {
            *state = WriteBehindState::Scheduled { pending, attempt, failures };
            Effects::none()
        }
        e @ (WriteBehindEvent::TimerFired { .. } | WriteBehindEvent::FlushRequested | WriteBehindEvent::CloseRequested) => {
            let attempt = seq.next();
            let batch = pending.clone();
            *state = WriteBehindState::Flushing { in_flight: pending, queued: IndexMap::new(), attempt, failures, closing: matches!(e, WriteBehindEvent::CloseRequested) };
            Effects::two(WriteBehindEffect::CancelTimer, WriteBehindEffect::WriteBatch { attempt, batch })
        }
        _ => {
            *state = WriteBehindState::Scheduled { pending, attempt, failures };
            Effects::none()
        }
    }
}

struct Flushing<K, V> {
    in_flight: IndexMap<K, V>,
    queued: IndexMap<K, V>,
    attempt: Attempt,
    failures: u32,
    closing: bool,
}

impl<K, V> From<Flushing<K, V>> for WriteBehindState<K, V> {
    fn from(f: Flushing<K, V>) -> Self {
        WriteBehindState::Flushing { in_flight: f.in_flight, queued: f.queued, attempt: f.attempt, failures: f.failures, closing: f.closing }
    }
}

fn flushing<K: Hash + Eq + Clone, V: Clone>(seq: &mut Seq, state: &mut WriteBehindState<K, V>, mut f: Flushing<K, V>, event: WriteBehindEvent<K, V>, delay_ms: u64) -> Effects<WriteBehindEffect<K, V>> {
    match event {
        WriteBehindEvent::Set { key, value } => {
            if f.closing {
                *state = f.into();
                return Effects::one(WriteBehindEffect::RejectClosed);
            }
            f.queued.insert(key, value);
            *state = f.into();
            Effects::none()
        }
        WriteBehindEvent::Drop(matches) => {
            f.in_flight.retain(|k, v| !matches(k, v));
            f.queued.retain(|k, v| !matches(k, v));
            *state = f.into();
            Effects::none()
        }
        WriteBehindEvent::CloseRequested => {
            f.closing = true;
            *state = f.into();
            Effects::none()
        }
        WriteBehindEvent::WriteSucceeded { attempt } if attempt == f.attempt => {
            if f.closing {
                *state = WriteBehindState::Closed;
                return Effects::none();
            }
            if f.queued.is_empty() {
                *state = WriteBehindState::Idle;
                return Effects::none();
            }
            let attempt = seq.next();
            *state = WriteBehindState::Scheduled { pending: f.queued, attempt, failures: 0 };
            Effects::one(WriteBehindEffect::ArmTimer { attempt, delay_ms })
        }
        WriteBehindEvent::WriteFailed { attempt, error } if attempt == f.attempt => {
            let report = WriteBehindEffect::ReportError(error);
            // The database is closing with the app; there is no later write to retry with.
            if f.closing {
                *state = WriteBehindState::Closed;
                return Effects::one(report);
            }
            // The caller already saw these rows saved, so they are written again; rows set since then win.
            let mut pending = f.in_flight;
            pending.extend(f.queued);
            if pending.is_empty() {
                *state = WriteBehindState::Idle;
                return Effects::one(report);
            }
            let attempt = seq.next();
            let backoff = delay_ms.saturating_mul(1 << f.failures.min(MAX_RETRY_EXPONENT));
            *state = WriteBehindState::Scheduled { pending, attempt, failures: f.failures + 1 };
            // The retry is armed before the error is reported (`:174-180`).
            Effects::two(WriteBehindEffect::ArmTimer { attempt, delay_ms: backoff }, report)
        }
        _ => {
            *state = f.into();
            Effects::none()
        }
    }
}

/// New rows are refused: the buffer closed, or closes once its last write settles (`:202-204`).
pub fn is_closed<K, V>(state: &WriteBehindState<K, V>) -> bool {
    matches!(state, WriteBehindState::Closed | WriteBehindState::Flushing { closing: true, .. })
}

/// Whether any row waits to be written or is being written.
pub fn has_pending<K, V>(state: &WriteBehindState<K, V>) -> bool {
    match state {
        WriteBehindState::Scheduled { pending, .. } => !pending.is_empty(),
        WriteBehindState::Flushing { in_flight, queued, .. } => !in_flight.is_empty() || !queued.is_empty(),
        WriteBehindState::Idle | WriteBehindState::Closed => false,
    }
}
