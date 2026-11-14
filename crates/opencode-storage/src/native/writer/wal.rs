//! The WAL's size and syncs, and its machine's effects (`machine::checkpoint`).

use std::time::{Duration, Instant};

use rusqlite::{Transaction, TransactionBehavior};

use super::{UNSYNCED_BYTES, UNSYNCED_FOR, Writer, boot_record};
use crate::machine::checkpoint::{self, WalEffect, WalEvent};

impl Writer {
    /// The WAL's frames to the disk (what `synchronous = FULL` does at each commit), through a
    /// handle of its own kept open; a failed sync waits for the next checkpoint's.
    pub(super) fn sync_wal(&mut self) {
        if self.wal_file.is_none() {
            let path = self.conn.path().map(|p| format!("{p}-wal"));
            self.wal_file = path.and_then(|p| std::fs::OpenOptions::new().read(true).write(true).open(p).ok());
        }
        if let Some(f) = &self.wal_file
            && f.sync_data().is_ok()
        {
            self.unsynced_since = None;
            self.wal_event(WalEvent::Synced);
        }
        rune::marks::mark("outbox.synced");
    }

    /// The WAL's size, through the handle kept for it.
    pub(super) fn wal_bytes(&mut self) -> u64 {
        if self.wal_file.is_none() {
            let path = self.conn.path().map(|p| format!("{p}-wal"));
            self.wal_file = path.and_then(|p| std::fs::OpenOptions::new().read(true).write(true).open(p).ok());
        }
        self.wal_file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len())
    }

    /// Unsynced commits leave SQLite's own checkpoint off (`UNSYNCED`): one run at
    /// `synchronous = OFF` copies unsynced frames into the database file without a sync first,
    /// which a power cut could leave half-written. This runs it at NORMAL instead (the WAL synced,
    /// then copied, then the file synced) once unsynced commits have piled up for
    /// `UNSYNCED_FOR` or the WAL has passed `UNSYNCED_BYTES`, which bounds what a power cut can
    /// lose while changes keep coming; a quiet spell's checkpoint (`IDLE_CHECKPOINT_MS`) and the
    /// quit's come sooner.
    pub(super) fn bound_unsynced(&mut self) {
        let since = *self.unsynced_since.get_or_insert_with(Instant::now);
        let bytes = self.wal_bytes();
        if since.elapsed() < UNSYNCED_FOR && bytes < UNSYNCED_BYTES {
            return;
        }
        // PASSIVE copies what no reader holds; with this one connection that's everything, and
        // the WAL restarts from the top at the next commit.
        let done = self.conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| Ok(r.get::<_, i64>(0)? == 0 && r.get::<_, i64>(1)? == r.get::<_, i64>(2)?)).unwrap_or(false);
        self.unsynced_since = None;
        if done {
            self.wal_event(WalEvent::Synced);
            self.search.checkpointed();
        }
    }

    pub(super) fn wal_event(&mut self, event: WalEvent) {
        let mut queue: Vec<WalEffect> = checkpoint::step(&mut self.seq, &mut self.wal, event).into_iter().collect();
        while let Some(effect) = queue.pop() {
            match effect {
                WalEffect::ArmTimer { attempt, delay_ms } => {
                    self.timers.insert("wal", Some((attempt, Instant::now() + Duration::from_millis(delay_ms))));
                }
                WalEffect::CancelTimer => {
                    self.timers.insert("wal", None);
                }
                WalEffect::Checkpoint { rebuild_boot } => {
                    // The record goes current in the transaction right before the checkpoint, so
                    // whenever the WAL is empty the record on page 2 is the truth.
                    let rebuilt = !rebuild_boot
                        || (|| -> rusqlite::Result<()> {
                            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
                            boot_record::rebuild(&tx)?;
                            tx.commit()
                        })()
                        .is_ok();
                    rune::marks::mark("writer.rebuilt");
                    let busy = if rebuilt { self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0)).unwrap_or(1) } else { 1 };
                    if busy == 0 {
                        self.unsynced_since = None;
                        self.search.checkpointed();
                    }
                    let event = if busy == 0 { WalEvent::Checkpointed } else { WalEvent::CheckpointFailed };
                    queue.extend(checkpoint::step(&mut self.seq, &mut self.wal, event));
                }
            }
        }
    }
}
