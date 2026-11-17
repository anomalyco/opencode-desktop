//! When the WAL goes back into the database file, and the boot record with it. A commit leaves
//! the WAL dirty; it's checkpointed once writes have been quiet for [`IDLE_CHECKPOINT_MS`], or at
//! quit, so the next open finds it empty and reads the boot record raw. The record is rebuilt
//! only then, when a commit since the last checkpoint touched what it summarizes: while the WAL
//! holds frames the raw read refuses anyway, so keeping it current per commit would be wasted
//! writes. A clean WAL makes quit free: no checkpoint, no sync.

use rune::machine::{Attempt, Effects, Seq};

pub const IDLE_CHECKPOINT_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WalState {
    #[default]
    Clean,
    Dirty {
        attempt: Attempt,
        /// A commit since the last checkpoint changed what the boot record holds.
        boot_stale: bool,
    },
}

pub enum WalEvent {
    Committed { boot: bool },
    TimerFired { attempt: Attempt },
    QuitRequested,
    Checkpointed,
    CheckpointFailed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WalEffect {
    ArmTimer { attempt: Attempt, delay_ms: u64 },
    CancelTimer,
    /// Rebuild the boot record first when `rebuild_boot`, then `wal_checkpoint(TRUNCATE)`.
    Checkpoint { rebuild_boot: bool },
}

pub fn step(seq: &mut Seq, state: &mut WalState, event: WalEvent) -> Effects<WalEffect> {
    match (*state, event) {
        // Every commit pushes the quiet period back.
        (s, WalEvent::Committed { boot }) => {
            let stale = matches!(s, WalState::Dirty { boot_stale: true, .. }) || boot;
            let attempt = seq.next();
            *state = WalState::Dirty { attempt, boot_stale: stale };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        (WalState::Dirty { attempt, boot_stale }, WalEvent::TimerFired { attempt: fired }) if attempt == fired => Effects::one(WalEffect::Checkpoint { rebuild_boot: boot_stale }),
        (WalState::Dirty { boot_stale, .. }, WalEvent::QuitRequested) => Effects::two(WalEffect::CancelTimer, WalEffect::Checkpoint { rebuild_boot: boot_stale }),
        (WalState::Dirty { .. }, WalEvent::Checkpointed) => {
            *state = WalState::Clean;
            Effects::one(WalEffect::CancelTimer)
        }
        // The file was busy: try again after another quiet period, the record still owed.
        (WalState::Dirty { boot_stale, .. }, WalEvent::CheckpointFailed) => {
            let attempt = seq.next();
            *state = WalState::Dirty { attempt, boot_stale };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        _ => Effects::none(),
    }
}
