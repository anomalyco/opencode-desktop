//! When the WAL goes back into the database file. A commit leaves it dirty; it's checkpointed
//! once writes have been quiet for [`IDLE_CHECKPOINT_MS`], or at quit, so the next open finds an
//! empty WAL and has nothing to recover. A clean WAL makes quit free: no checkpoint, no sync.

use rune::machine::{Attempt, Effects, Seq};

pub const IDLE_CHECKPOINT_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WalState {
    #[default]
    Clean,
    Dirty { attempt: Attempt },
}

pub enum WalEvent {
    Committed,
    TimerFired { attempt: Attempt },
    QuitRequested,
    Checkpointed,
    CheckpointFailed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WalEffect {
    ArmTimer { attempt: Attempt, delay_ms: u64 },
    CancelTimer,
    Checkpoint,
}

pub fn step(seq: &mut Seq, state: &mut WalState, event: WalEvent) -> Effects<WalEffect> {
    match (*state, event) {
        // Every commit pushes the quiet period back.
        (_, WalEvent::Committed) => {
            let attempt = seq.next();
            *state = WalState::Dirty { attempt };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        (WalState::Dirty { attempt }, WalEvent::TimerFired { attempt: fired }) if attempt == fired => Effects::one(WalEffect::Checkpoint),
        (WalState::Dirty { .. }, WalEvent::QuitRequested) => Effects::two(WalEffect::CancelTimer, WalEffect::Checkpoint),
        (WalState::Dirty { .. }, WalEvent::Checkpointed) => {
            *state = WalState::Clean;
            Effects::one(WalEffect::CancelTimer)
        }
        // Readers held the WAL (or the file was busy): try again after another quiet period.
        (WalState::Dirty { .. }, WalEvent::CheckpointFailed) => {
            let attempt = seq.next();
            *state = WalState::Dirty { attempt };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        _ => Effects::none(),
    }
}
