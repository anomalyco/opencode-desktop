//! WAL durability tiers, write-behind flushing, blob GC, checkpointing, and repaging on close.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rune::machine::{Attempt, Effects, Seq};
use rusqlite::{Connection, Transaction, TransactionBehavior};

use super::reserve::Reservation;
use super::rows::{note, slow};
use super::{
    Batch, CACHES_MS, DOCS_MS, PREVIEWS_MS, STATE_MS, WINDOWS_MS, Writer, step_buffer,
};
use crate::machine::checkpoint::IDLE_CHECKPOINT_MS;
use crate::machine::draft_gc::{self, DraftGcEffect, DraftGcEvent};
use crate::machine::write_behind::{self as wb, WriteBehindEvent};
use crate::native::{boot_record, sql};
use crate::{FlushReason, FlushReport, StorageError};

/// The page size a file is rewritten to (`Writer::repage`), and the largest file rewritten.
const REPAGE_TO: i64 = 16384;
const REPAGE_MAX: i64 = 64 * 1024 * 1024;
/// An unsynced commit (`commit_soon`, write-behind): SQLite's checkpoint off for it, as it would
/// run at OFF (`Writer::wal_unsynced`); and back.
pub(super) const UNSYNCED: &str = "PRAGMA synchronous = OFF; PRAGMA wal_autocheckpoint = 0";
pub(super) const SYNCED: &str = "PRAGMA synchronous = NORMAL; PRAGMA wal_autocheckpoint = 1000";
const DURABLE: &str = "PRAGMA synchronous = FULL; PRAGMA wal_autocheckpoint = 0";
/// The longest unsynced commits pile up, and the most WAL they fill, before a checkpoint syncs them.
const UNSYNCED_FOR: Duration = Duration::from_secs(30);
const UNSYNCED_BYTES: u64 = 16 * 1024 * 1024;

/// Lazily opened handle to `<db>-wal`, kept open across `Clean` and `Dirty` because
/// `SQLITE_FCNTL_PERSIST_WAL` retains `-wal` on disk across truncating checkpoints.
#[derive(Default)]
pub(super) enum WalFile {
    #[default]
    Unopened,
    Open(std::fs::File),
}

impl WalFile {
    fn get_or_open(&mut self, conn: &Connection) -> Option<&std::fs::File> {
        if matches!(self, Self::Unopened)
            && let Some(file) = conn
                .path()
                .map(|p| format!("{p}-wal"))
                .and_then(|p| std::fs::OpenOptions::new().write(true).open(p).ok())
        {
            *self = Self::Open(file);
        }
        match self {
            Self::Open(file) => Some(file),
            Self::Unopened => None,
        }
    }

    fn len(&mut self, conn: &Connection) -> u64 {
        if let Some(len) = self.get_or_open(conn).and_then(|f| f.metadata().ok()).map(|m| m.len()) {
            return len;
        }
        // Fallback when `-wal` cannot be opened for write: `metadata` reads live NTFS FCB size
        // without needing write permission.
        conn.path().and_then(|p| std::fs::metadata(format!("{p}-wal")).ok()).map_or(0, |m| m.len())
    }

    fn sync_all(&mut self, conn: &Connection) -> Option<u64> {
        let file = self.get_or_open(conn)?;
        file.sync_all().ok()?;
        Some(file.metadata().ok().map_or(0, |m| m.len()))
    }

    fn close(&mut self) {
        *self = Self::Unopened;
    }
}

fn unsynced_wal_bytes(wal_bytes: u64, base_bytes: u64) -> u64 {
    if wal_bytes >= base_bytes {
        wal_bytes - base_bytes
    } else {
        // The WAL wrapped/truncated below the previous base; every current byte is new.
        wal_bytes
    }
}

/// Whether the dirty WAL holds any `synchronous = OFF` commits waiting for a sync or checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WalSync {
    /// Every committed frame in the WAL has been synced (via `sync_wal`, a completed `PASSIVE`
    /// checkpoint, or a synced commit). `base_bytes` is the `-wal` length when synced (0 after a
    /// completed `PASSIVE` checkpoint, since the next commit restarts `-wal` from 0).
    Synced { base_bytes: u64 },
    /// At least one unsynced commit landed at `since` and has not been synced or checkpointed yet.
    /// `base_bytes` is the `-wal` length at the previous sync so unsynced growth is bounded past
    /// `UNSYNCED_BYTES` without syncing on every commit when the WAL stays above 16 MiB.
    Unsynced { since: Instant, base_bytes: u64 },
}

/// The writer's WAL checkpoint and sync-deadline state.
///
/// Holds the unsynced window (`WalSync`) inside `Dirty`, so a `Clean` WAL can never carry an
/// unsynced deadline (`crate::machine::checkpoint::WalState` stays unchanged for external callers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Wal {
    #[default]
    Clean,
    Dirty {
        attempt: Attempt,
        /// A commit since the last checkpoint changed what the boot record holds.
        boot_stale: bool,
        sync: WalSync,
    },
}

impl Wal {
    pub(super) fn is_clean(self) -> bool {
        self == Self::Clean
    }
}

pub(super) enum WalEvent {
    /// Database opened with pre-existing WAL frames or a stale boot record (unsynced until
    /// checkpointed or synced).
    OpenedDirty { boot_stale: bool, now: Instant },
    /// The boot record must be rebuilt at the next checkpoint (no new SQLite commit of its own,
    /// e.g. after attaching the draft journal).
    BootStale,
    /// A Tier 0 commit landed with `synchronous = FULL`, syncing the entire WAL up to `wal_bytes`.
    DurableCommitted { boot: bool, wal_bytes: u64 },
    /// An unsynced (`synchronous = OFF` or `NORMAL`) commit landed; `wal_bytes` is the `-wal` file length.
    UnsyncedCommitted { boot: bool, now: Instant, wal_bytes: u64 },
    /// `PRAGMA wal_checkpoint(PASSIVE)` checkpointed every frame in the WAL (`busy == 0`,
    /// `log >= 0`, `checkpointed >= log`).
    PassiveCheckpointed,
    /// `PRAGMA wal_checkpoint(PASSIVE)` errored, was busy, or left frames uncheckpointed.
    PassiveCheckpointIncomplete { now: Instant },
    /// `sync_wal` successfully synced `-wal` to disk (`File::sync_all`) at `wal_bytes`.
    WalSynced { wal_bytes: u64 },
    /// `sync_wal` could not open or sync `-wal`; data stays unsynced until a later sync succeeds.
    WalSyncFailed { now: Instant },
    TimerFired { attempt: Attempt },
    QuitRequested,
    Checkpointed,
    CheckpointFailed,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WalEffect {
    ArmTimer { attempt: Attempt, delay_ms: u64 },
    CancelTimer,
    /// Run `PRAGMA wal_checkpoint(PASSIVE)` to bound unsynced frames while writes keep coming.
    PassiveCheckpoint,
    /// Sync `<db>-wal` directly (`File::sync_all`) when a checkpoint could not complete.
    SyncWal,
    /// Rebuild the boot record first when `rebuild_boot`, then `PRAGMA wal_checkpoint(TRUNCATE)`.
    Checkpoint { rebuild_boot: bool },
}

fn wal_step(seq: &mut Seq, state: &mut Wal, event: WalEvent) -> Effects<WalEffect> {
    match (*state, event) {
        (Wal::Clean, WalEvent::OpenedDirty { boot_stale, now }) => {
            let attempt = seq.next();
            *state = Wal::Dirty { attempt, boot_stale, sync: WalSync::Unsynced { since: now, base_bytes: 0 } };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        // Marks the boot record stale without recording a new commit; preserves sync status.
        (s, WalEvent::BootStale) => {
            let sync = match s {
                Wal::Clean => WalSync::Synced { base_bytes: 0 },
                Wal::Dirty { sync, .. } => sync,
            };
            let attempt = seq.next();
            *state = Wal::Dirty { attempt, boot_stale: true, sync };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        (s, WalEvent::DurableCommitted { boot, wal_bytes }) => {
            let prev_boot = matches!(s, Wal::Dirty { boot_stale: true, .. });
            let attempt = seq.next();
            *state = Wal::Dirty { attempt, boot_stale: prev_boot || boot, sync: WalSync::Synced { base_bytes: wal_bytes } };
            Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
        }
        (s, WalEvent::UnsyncedCommitted { boot, now, wal_bytes }) => {
            let (prev_boot, since, base_bytes) = match s {
                Wal::Dirty { boot_stale, sync: WalSync::Unsynced { since, base_bytes }, .. } => (boot_stale, since, base_bytes),
                Wal::Dirty { boot_stale, sync: WalSync::Synced { base_bytes }, .. } => (boot_stale, now, base_bytes),
                Wal::Clean => (false, now, 0),
            };
            let attempt = seq.next();
            let boot_stale = prev_boot || boot;
            // Stay Unsynced until PassiveCheckpointed or WalSynced confirms durability.
            *state = Wal::Dirty { attempt, boot_stale, sync: WalSync::Unsynced { since, base_bytes } };
            if now.saturating_duration_since(since) >= UNSYNCED_FOR || unsynced_wal_bytes(wal_bytes, base_bytes) >= UNSYNCED_BYTES {
                Effects::two(WalEffect::PassiveCheckpoint, WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
            } else {
                Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
            }
        }
        (Wal::Dirty { attempt, boot_stale, .. }, WalEvent::PassiveCheckpointed) => {
            *state = Wal::Dirty { attempt, boot_stale, sync: WalSync::Synced { base_bytes: 0 } };
            Effects::none()
        }
        (Wal::Dirty { attempt, boot_stale, sync }, WalEvent::PassiveCheckpointIncomplete { now }) => {
            let sync = match sync {
                WalSync::Unsynced { since, base_bytes } => WalSync::Unsynced { since, base_bytes },
                WalSync::Synced { base_bytes } => WalSync::Unsynced { since: now, base_bytes },
            };
            *state = Wal::Dirty { attempt, boot_stale, sync };
            Effects::one(WalEffect::SyncWal)
        }
        (Wal::Dirty { attempt, boot_stale, .. }, WalEvent::WalSynced { wal_bytes }) => {
            *state = Wal::Dirty { attempt, boot_stale, sync: WalSync::Synced { base_bytes: wal_bytes } };
            Effects::none()
        }
        (Wal::Dirty { attempt, boot_stale, sync }, WalEvent::WalSyncFailed { now }) => {
            let sync = match sync {
                WalSync::Unsynced { since, base_bytes } => WalSync::Unsynced { since, base_bytes },
                WalSync::Synced { base_bytes } => WalSync::Unsynced { since: now, base_bytes },
            };
            *state = Wal::Dirty { attempt, boot_stale, sync };
            Effects::none()
        }
        (Wal::Dirty { attempt, boot_stale, .. }, WalEvent::TimerFired { attempt: fired }) if attempt == fired => {
            Effects::one(WalEffect::Checkpoint { rebuild_boot: boot_stale })
        }
        (Wal::Dirty { boot_stale, .. }, WalEvent::QuitRequested) => {
            Effects::two(WalEffect::CancelTimer, WalEffect::Checkpoint { rebuild_boot: boot_stale })
        }
        (Wal::Dirty { .. }, WalEvent::Checkpointed) => {
            *state = Wal::Clean;
            Effects::one(WalEffect::CancelTimer)
        }
        // The file was busy or failed: sync `-wal` directly if unsynced frames remain so idle or
        // quit never leaves data unsynced on disk, then retry checkpointing after a quiet period.
        (Wal::Dirty { boot_stale, sync, .. }, WalEvent::CheckpointFailed) => {
            let attempt = seq.next();
            *state = Wal::Dirty { attempt, boot_stale, sync };
            if matches!(sync, WalSync::Unsynced { .. }) {
                Effects::two(WalEffect::SyncWal, WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
            } else {
                Effects::one(WalEffect::ArmTimer { attempt, delay_ms: IDLE_CHECKPOINT_MS })
            }
        }
        _ => Effects::none(),
    }
}

impl Writer {
    /// Unsynced commits leave SQLite's own checkpoint off (`UNSYNCED`): one run at
    /// `synchronous = OFF` copies unsynced frames into the database file without a sync first,
    /// which a power cut could leave half-written. This runs it at NORMAL instead (the WAL synced,
    /// then copied, then the file synced) once unsynced commits have piled up for
    /// `UNSYNCED_FOR` or the WAL has passed `UNSYNCED_BYTES`, which bounds what a power cut can
    /// lose while changes keep coming; a quiet spell's checkpoint (`IDLE_CHECKPOINT_MS`) and the
    /// quit's come sooner.
    pub(super) fn wal_unsynced(&mut self, boot: bool) {
        let wal_bytes = self.wal_file.len(&self.conn);
        self.wal_event(WalEvent::UnsyncedCommitted { boot, now: Instant::now(), wal_bytes });
    }

    /// A Tier 0 write: committed and synced (`synchronous = FULL`) before it returns.
    pub(super) fn durable(&mut self, boot: bool, f: impl FnOnce(&Connection, i64) -> rusqlite::Result<()>) -> Result<(), StorageError> {
        let now = (self.clock)();
        let result = (|| {
            self.conn.execute_batch(DURABLE)?;
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            f(&tx, now)?;
            tx.commit()
        })();
        note(&result);
        self.conn.execute_batch(SYNCED).ok();
        match result {
            Ok(()) => {
                let wal_bytes = self.wal_file.len(&self.conn);
                self.wal_event(WalEvent::DurableCommitted { boot, wal_bytes });
                Ok(())
            }
            Err(e) => Err(StorageError(e.to_string())),
        }
    }

    pub(super) fn flush(&mut self, reason: FlushReason, started: Instant) -> FlushReport {
        let quit = matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart);
        let pending = wb::has_pending(&self.state) || wb::has_pending(&self.docs) || wb::has_pending(&self.windows) || wb::has_pending(&self.previews) || wb::has_pending(&self.caches);
        if !pending && (!quit || self.wal.is_clean()) {
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
        // At quit / session-end / update-restart, complete the checkpoint (or fallback WAL sync)
        // before returning FlushReport so callers and `close()`'s `journal::finish` never observe
        // a clean/checkpointed answer while frames are still unsynced in the WAL.
        let had_wal = !self.wal.is_clean();
        self.quit_checkpoint(reason);
        if quit {
            rune::marks::mark("writer.checkpointed");
        }
        let checkpointed = quit && had_wal && self.wal.is_clean();
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
        // Drop the extra handle to `<db>-wal` before switching journal modes so `-wal` cannot be
        // left delete-pending on Windows or bound to an unlinked inode if SQLite deletes it.
        self.wal_file.close();
        let done = self.conn.execute_batch(&format!("PRAGMA journal_mode = DELETE; PRAGMA page_size = {REPAGE_TO}; VACUUM; PRAGMA journal_mode = WAL;"));
        if done.is_err() {
            self.conn.execute_batch("PRAGMA journal_mode = WAL;").ok();
        }
        let root: i64 = self.conn.query_row("SELECT rootpage FROM sqlite_schema WHERE name = 'boot'", [], |r| r.get(0)).unwrap_or(0);
        if root != 2
            && let Ok(version) = super::super::schema::user_version(&self.conn)
        {
            super::super::schema::set_flags(&self.conn, version & super::super::schema::FLAG_MASK & !super::super::schema::FLAG_BOOT_PAGE2).ok();
        }
        // The WAL back to empty after any `user_version` update: the next launch reads the record raw.
        self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).ok();
        eprintln!("opencode-storage: {} {} pages of {page} B into {REPAGE_TO} B pages in {} us", if done.is_ok() { "rewrote" } else { "couldn't rewrite" }, pages, t.elapsed().as_micros());
    }

    /// The kill-safe tier (`Storage::commit_soon`): once the queue is drained, the state,
    /// document and window rows waiting in their buffers go to the WAL in one commit (no sync).
    pub(super) fn commit_buffered(&mut self) {
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
                        Ok((oldest, deleted)) => {
                            self.touched.retain(|_, t| *t >= before_ms);
                            if deleted > 0 {
                                self.wal_unsynced(false);
                            }
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
        let mut queue: VecDeque<WalEffect> = wal_step(&mut self.seq, &mut self.wal, event).into_iter().collect();
        while let Some(effect) = queue.pop_front() {
            match effect {
                WalEffect::ArmTimer { attempt, delay_ms } => {
                    self.timers.insert("wal", Some((attempt, Instant::now() + Duration::from_millis(delay_ms))));
                }
                WalEffect::CancelTimer => {
                    self.timers.insert("wal", None);
                }
                WalEffect::PassiveCheckpoint => {
                    // SQLite's PASSIVE checkpoint can return busy == 0 even when a reader blocked
                    // progress (checkpointed < log), so require all logged frames to be checkpointed.
                    let complete = matches!(
                        self.conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))),
                        Ok((0, log, ckpt)) if log >= 0 && ckpt >= log
                    );
                    let event = if complete { WalEvent::PassiveCheckpointed } else { WalEvent::PassiveCheckpointIncomplete { now: Instant::now() } };
                    queue.extend(wal_step(&mut self.seq, &mut self.wal, event));
                }
                WalEffect::SyncWal => {
                    let event = match self.wal_file.sync_all(&self.conn) {
                        Some(wal_bytes) => {
                            rune::marks::mark("wal.synced");
                            WalEvent::WalSynced { wal_bytes }
                        }
                        None => WalEvent::WalSyncFailed { now: Instant::now() },
                    };
                    queue.extend(wal_step(&mut self.seq, &mut self.wal, event));
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
                    let complete = rebuilt
                        && matches!(
                            self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))),
                            Ok((0, log, ckpt)) if log >= 0 && ckpt >= log
                        );
                    let event = if complete { WalEvent::Checkpointed } else { WalEvent::CheckpointFailed };
                    queue.extend(wal_step(&mut self.seq, &mut self.wal, event));
                }
            }
        }
    }
}
