//! When the WAL goes back into the database file, and the boot record with it. A commit leaves
//! the WAL dirty; it's checkpointed once writes have been quiet for [`IDLE_CHECKPOINT_MS`]. The
//! quit doesn't checkpoint (`Writer::quit_finish`): it syncs the WAL when it holds commits not yet
//! synced, and the next launch checkpoints after its first frame ([`WalEvent::Opened`]). The
//! record is rebuilt only before a checkpoint or at the quit, when a commit since the last
//! checkpoint touched what it summarizes: keeping it current per commit would be wasted writes.
//! The search index's commits ([`WalEvent::Derived`]) start no checkpoint and need no sync: its
//! rows can be rebuilt. A clean WAL makes quit free: no checkpoint, no sync.

use rune::machine::{Attempt, Effects, Seq};

pub const IDLE_CHECKPOINT_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WalState {
    #[default]
    Clean,
    /// The WAL holds only frames that can be rebuilt (the search index's): no checkpoint is owed
    /// for them, and no sync; the next checkpoint something else starts takes them along.
    Derived,
    Dirty {
        attempt: Attempt,
        /// A commit since the last checkpoint changed what the boot record holds.
        boot_stale: bool,
        /// A commit since the WAL was last synced: a power cut could lose it.
        unsynced: bool,
    },
}

pub enum WalEvent {
    Committed { boot: bool },
    /// A commit of rebuildable rows only (an index step): it arms nothing.
    Derived,
    /// The WAL's frames reached the disk (an outbox row's sync, the quit's).
    Synced,
    /// The boot record was rebuilt outside a checkpoint (the quit's end, `Writer::quit_finish`).
    Rebuilt,
    /// The quit left the WAL with the record's copy beside it: no checkpoint this run (the next
    /// launch's writer does it after its first frame).
    Left,
    /// The first frame is up: a WAL the last run left is checkpointed after a quiet period, as
    /// if it were this run's commit.
    Opened,
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
            *state = WalState::Dirty { attempt, boot_stale: stale, unsynced: true };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        (WalState::Clean, WalEvent::Derived) => {
            *state = WalState::Derived;
            Effects::none()
        }
        (WalState::Derived | WalState::Dirty { .. }, WalEvent::Derived) => Effects::none(),
        (WalState::Dirty { attempt, boot_stale, .. }, WalEvent::Synced) => {
            *state = WalState::Dirty { attempt, boot_stale, unsynced: false };
            Effects::none()
        }
        (WalState::Derived | WalState::Dirty { .. }, WalEvent::Left) => Effects::one(WalEffect::CancelTimer),
        (WalState::Dirty { attempt, .. }, WalEvent::Rebuilt) => {
            // The rebuild just committed at synchronous=OFF. A sync that covered the previous
            // end of the WAL cannot cover this transaction, even if every earlier row was durable.
            *state = WalState::Dirty { attempt, boot_stale: false, unsynced: true };
            Effects::none()
        }
        (WalState::Dirty { attempt, .. }, WalEvent::Opened) => Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS }),
        (WalState::Dirty { attempt, boot_stale, .. }, WalEvent::TimerFired { attempt: fired }) if attempt == fired => Effects::one(WalEffect::Checkpoint { rebuild_boot: boot_stale }),
        (WalState::Dirty { boot_stale, .. }, WalEvent::QuitRequested) => Effects::two(WalEffect::CancelTimer, WalEffect::Checkpoint { rebuild_boot: boot_stale }),
        (WalState::Derived, WalEvent::QuitRequested) => Effects::one(WalEffect::Checkpoint { rebuild_boot: false }),
        (WalState::Derived | WalState::Dirty { .. }, WalEvent::Checkpointed) => {
            *state = WalState::Clean;
            Effects::one(WalEffect::CancelTimer)
        }
        // The file was busy: try again after another quiet period, the record still owed.
        (WalState::Derived, WalEvent::CheckpointFailed) => Effects::none(),
        (WalState::Dirty { boot_stale, unsynced, .. }, WalEvent::CheckpointFailed) => {
            let attempt = seq.next();
            *state = WalState::Dirty { attempt, boot_stale, unsynced };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        _ => Effects::none(),
    }
}
