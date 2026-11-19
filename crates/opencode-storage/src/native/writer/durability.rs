//! WAL durability tiers, write-behind flushing, blob GC, checkpointing, and repaging on close.

use std::time::{Duration, Instant};

use rune::machine::Effects;
use rusqlite::{Connection, Transaction, TransactionBehavior};

use super::reserve::Reservation;
use super::rows::{note, slow};
use super::{
    Batch, CACHES_MS, DOCS_MS, PREVIEWS_MS, STATE_MS, WINDOWS_MS, Writer, step_buffer,
};
use crate::machine::checkpoint::{self, WalEffect, WalEvent, WalState};
use crate::machine::draft_gc::{self, DraftGcEffect, DraftGcEvent};
use crate::machine::write_behind::{self as wb, WriteBehindEvent};
use crate::native::{boot_record, sql};
use crate::{FlushReason, FlushReport, StorageError};

/// The page size a file is rewritten to (`Writer::repage`), and the largest file rewritten.
const REPAGE_TO: i64 = 16384;
const REPAGE_MAX: i64 = 64 * 1024 * 1024;
/// An unsynced commit (`commit_soon`, an outbox row): SQLite's checkpoint off for it, as it would
/// run at OFF (`Writer::bound_unsynced`); and back.
pub(super) const UNSYNCED: &str = "PRAGMA synchronous = OFF; PRAGMA wal_autocheckpoint = 0";
pub(super) const SYNCED: &str = "PRAGMA synchronous = NORMAL; PRAGMA wal_autocheckpoint = 1000";
/// The longest unsynced commits pile up, and the most WAL they fill, before a checkpoint syncs them.
const UNSYNCED_FOR: Duration = Duration::from_secs(30);
const UNSYNCED_BYTES: u64 = 16 * 1024 * 1024;

impl Writer {
    /// A kill-safe write: committed to the WAL (handed to the operating system, no sync) before
    /// it returns; `sync_wal` makes it survive a power cut too.
    pub(super) fn committed(&mut self, boot: bool, f: impl FnOnce(&Connection, i64) -> rusqlite::Result<()>) -> Result<(), StorageError> {
        let now = (self.clock)();
        let result = (|| {
            // Unsynced, the WAL header included (see `commit_if_due`); `sync_wal` follows.
            self.conn.execute_batch(UNSYNCED)?;
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            f(&tx, now)?;
            tx.commit()
        })();
        note(&result);
        self.conn.execute_batch(SYNCED).ok();
        match result {
            Ok(()) => {
                self.wal_event(WalEvent::Committed { boot });
                Ok(())
            }
            Err(e) => Err(StorageError(e.to_string())),
        }
    }

    /// The WAL's frames to the disk (what `synchronous = FULL` does at each commit), through a
    /// handle of its own kept open; a failed sync waits for the next checkpoint's.
    pub(super) fn sync_wal(&mut self) {
        if self.wal_file.is_none() {
            let path = self.conn.path().map(|p| format!("{p}-wal"));
            self.wal_file = path.and_then(|p| std::fs::OpenOptions::new().write(true).open(p).ok());
        }
        if let Some(f) = &self.wal_file
            && f.sync_data().is_ok()
        {
            self.unsynced_since = None;
        }
        rune::marks::mark("outbox.synced");
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
        if self.wal_file.is_none() {
            let path = self.conn.path().map(|p| format!("{p}-wal"));
            self.wal_file = path.and_then(|p| std::fs::OpenOptions::new().write(true).open(p).ok());
        }
        let bytes = self.wal_file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
        if since.elapsed() < UNSYNCED_FOR && bytes < UNSYNCED_BYTES {
            return;
        }
        self.conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(())).ok();
        self.unsynced_since = None;
    }

    /// A Tier 0 write: committed and synced before it returns.
    pub(super) fn durable(&mut self, boot: bool, f: impl FnOnce(&Connection, i64) -> rusqlite::Result<()>) -> Result<(), StorageError> {
        let now = (self.clock)();
        let result = (|| {
            self.conn.execute_batch("PRAGMA synchronous = FULL")?;
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            f(&tx, now)?;
            tx.commit()
        })();
        note(&result);
        self.conn.execute_batch("PRAGMA synchronous = NORMAL").ok();
        match result {
            Ok(()) => {
                self.wal_event(WalEvent::Committed { boot });
                Ok(())
            }
            Err(e) => Err(StorageError(e.to_string())),
        }
    }

    pub(super) fn flush(&mut self, reason: FlushReason, started: Instant) -> FlushReport {
        let quit = matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart);
        let pending = wb::has_pending(&self.state) || wb::has_pending(&self.docs) || wb::has_pending(&self.windows) || wb::has_pending(&self.previews) || wb::has_pending(&self.caches);
        if !pending && (!quit || self.wal == WalState::Clean) {
            return FlushReport { clean: true, micros: super::super::micros(started), ..FlushReport::default() };
        }
        self.written = 0;
        let mut batches: Vec<Batch> = Vec::new();
        batches.extend(step_buffer(&mut self.seq, &mut self.state, WriteBehindEvent::FlushRequested, STATE_MS, "state", &mut self.timers).into_iter().map(|(a, m)| Batch::State(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.docs, WriteBehindEvent::FlushRequested, DOCS_MS, "docs", &mut self.timers).into_iter().map(|(a, m)| Batch::Docs(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.windows, WriteBehindEvent::FlushRequested, WINDOWS_MS, "windows", &mut self.timers).into_iter().map(|(a, m)| Batch::Windows(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.previews, WriteBehindEvent::FlushRequested, PREVIEWS_MS, "previews", &mut self.timers).into_iter().map(|(a, m)| Batch::Previews(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.caches, WriteBehindEvent::FlushRequested, CACHES_MS, "caches", &mut self.timers).into_iter().map(|(a, m)| Batch::Caches(a, m)));
        self.commit(batches);
        // At quit the WAL goes back into the file (`quit_checkpoint`) after the caller has its
        // answer: the rows are committed now, and the synced checkpoint overlaps the windows
        // closing; closing the storage waits for it.
        let checkpointed = quit && self.wal != WalState::Clean;
        FlushReport { clean: false, rows: self.written, micros: super::super::micros(started), checkpointed, timed_out: false }
    }

    /// A file made before 16 KiB pages is rewritten with them as the storage closes, once, while
    /// it's small enough to take a few ms (`REPAGE_MAX`): a commit writes whole pages to the WAL,
    /// and the kill-safe tier commits a keystroke's draft at a time. A bigger file keeps its 64 KiB
    /// pages. Rewritten with `VACUUM` under the rollback journal, so a kill midway leaves the old
    /// file; the boot record's raw read is kept only if its table's root is still page 2.
    pub(super) fn repage(&mut self) {
        let get = |pragma: &str| self.conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0);
        let (page, pages) = (get("page_size"), get("page_count"));
        if page <= REPAGE_TO || page * pages > REPAGE_MAX {
            return;
        }
        let t = Instant::now();
        // VACUUM moves every page: the draft journal's place goes with it, so it's reserved
        // again at the next launch (closed and clean by now: `journal::finish`).
        self.conn.execute_batch("DELETE FROM journal_meta; DELETE FROM journal;").ok();
        let done = self.conn.execute_batch(&format!("PRAGMA journal_mode = DELETE; PRAGMA page_size = {REPAGE_TO}; VACUUM; PRAGMA journal_mode = WAL;"));
        // The WAL back to empty: the next launch reads the record raw.
        self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).ok();
        let root: i64 = self.conn.query_row("SELECT rootpage FROM sqlite_schema WHERE name = 'boot'", [], |r| r.get(0)).unwrap_or(0);
        if root != 2
            && let Ok(version) = super::super::schema::user_version(&self.conn)
        {
            super::super::schema::set_flags(&self.conn, version & super::super::schema::FLAG_MASK & !super::super::schema::FLAG_BOOT_PAGE2).ok();
        }
        eprintln!("opencode-storage: {} {} pages of {page} B into {REPAGE_TO} B pages in {} us", if done.is_ok() { "rewrote" } else { "couldn't rewrite" }, pages, t.elapsed().as_micros());
    }

    /// The kill-safe tier (`Storage::commit_soon`): once the queue is drained, the state,
    /// document and window rows waiting in their buffers go to the WAL in one commit (no sync).
    pub(super) fn commit_if_due(&mut self) {
        if !std::mem::take(&mut self.commit_due) {
            return;
        }
        let mut batches: Vec<Batch> = Vec::new();
        batches.extend(step_buffer(&mut self.seq, &mut self.state, WriteBehindEvent::FlushRequested, STATE_MS, "state", &mut self.timers).into_iter().map(|(a, m)| Batch::State(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.docs, WriteBehindEvent::FlushRequested, DOCS_MS, "docs", &mut self.timers).into_iter().map(|(a, m)| Batch::Docs(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.windows, WriteBehindEvent::FlushRequested, WINDOWS_MS, "windows", &mut self.timers).into_iter().map(|(a, m)| Batch::Windows(a, m)));
        // No sync at all, not even the WAL header's after a checkpoint (1,400 µs at NORMAL
        // against 145-165 µs, `proof\storage-2\first-commit.txt`): the frames are the operating
        // system's once written; the next checkpoint (at NORMAL) syncs them (`commit`).
        self.commit(batches);
    }

    /// The quit's checkpoint, after its flush answered: the next open finds the WAL empty and
    /// reads the boot record raw.
    pub(super) fn quit_checkpoint(&mut self, reason: FlushReason) {
        if matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart) {
            self.wal_event(WalEvent::QuitRequested);
        }
    }

    pub(super) fn close_machines(&mut self) {
        let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::CloseRequested);
        self.gc_effects(effects);
        self.state_event(WriteBehindEvent::CloseRequested);
        self.docs_event(WriteBehindEvent::CloseRequested);
        self.windows_event(WriteBehindEvent::CloseRequested);
        self.previews_event(WriteBehindEvent::CloseRequested);
        self.caches_event(WriteBehindEvent::CloseRequested);
    }

    /// The reservation's pages reached the file through an unsynced checkpoint that also emptied
    /// the WAL: until the file is synced, a power cut could keep the WAL's truncation and lose the
    /// pages. One sync of the file closes that (a kill never could: the OS has the writes).
    pub(super) fn sync_reservation(&mut self) {
        let t = Instant::now();
        // Through the storage's kept descriptor (`dbfile`): one opened and closed here would drop the
        // connection's locks on POSIX.
        let synced = self.conn.path().map(std::path::PathBuf::from).ok_or(std::io::ErrorKind::NotFound.into()).and_then(|p| super::super::dbfile::get(&p)).and_then(|f| f.sync_all());
        rune::marks::mark("storage.reservation.synced");
        match synced {
            Ok(()) => self.reservation = Reservation::Durable,
            Err(e) => eprintln!("opencode-storage: couldn't sync the draft journal's reservation ({e}); the next synced checkpoint does"),
        }
        slow("syncing the draft journal's reservation", t);
    }

    pub(super) fn gc_effects(&mut self, effects: Effects<DraftGcEffect>) {
        for effect in effects {
            match effect {
                DraftGcEffect::ArmTimer { attempt, delay_ms } => {
                    self.timers.insert("gc", Some((attempt, Instant::now() + Duration::from_millis(u64::try_from(delay_ms).unwrap_or(0)))));
                }
                DraftGcEffect::CancelTimer => {
                    self.timers.insert("gc", None);
                }
                DraftGcEffect::Collect { attempt, at_ms, before_ms } => {
                    let event = match sql::collect_blobs(&self.conn, before_ms) {
                        Ok(oldest) => {
                            self.touched.retain(|_, t| *t >= before_ms);
                            DraftGcEvent::CollectSucceeded { attempt, at_ms, oldest_orphan_ms: oldest }
                        }
                        Err(e) => DraftGcEvent::CollectFailed { attempt, at_ms, error: e.to_string() },
                    };
                    let next = draft_gc::step(&mut self.seq, &mut self.gc, event);
                    self.gc_effects(next);
                }
                DraftGcEffect::ReportError(e) => eprintln!("opencode-storage: blob sweep failed: {e}"),
            }
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
                    }
                    let event = if busy == 0 { WalEvent::Checkpointed } else { WalEvent::CheckpointFailed };
                    queue.extend(checkpoint::step(&mut self.seq, &mut self.wal, event));
                }
            }
        }
    }
}
