//! The writer's part of the app's quit: its end (the WAL synced with the record's copy beside
//! it, or the checkpoint as before) and the repage a quit may do.

use std::time::Instant;

use rusqlite::{Transaction, TransactionBehavior};

use super::{QUIT_WAL_BYTES, REPAGE_MAX, REPAGE_TO, SYNCED, UNSYNCED, Writer, boot_record, slow};
use crate::FlushReason;
use crate::machine::checkpoint::{WalEvent, WalState};

/// Where the writer is with the app's quit: running; quitting (rows still to land, or the
/// fallback checkpoint's); left (its rows landed and its end is written: the WAL synced with the
/// record's copy, or checkpointed).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Quit {
    Running,
    Quitting,
    Left,
}

pub(super) fn is_quit(reason: FlushReason) -> bool {
    matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart)
}

impl Writer {
    /// The quit's end, after its rows landed. On Windows and macOS (where the journal is proved):
    /// the boot record made current, the WAL synced (all a power cut needs: every commit is in
    /// it), and the record's copy written beside the file with where the WAL ends (`bootcopy`), so
    /// the next launch reads its first frame from the copy and checkpoints after it. One sync, no
    /// copy of the WAL into the file and no sync of the file. Otherwise (Linux; a file that's
    /// repaged at this quit; a WAL over `QUIT_WAL_BYTES`; a failed sync or copy): the synced
    /// checkpoint as before.
    pub(super) fn quit_finish(&mut self) {
        let t = Instant::now();
        if self.wal == WalState::Clean && self.wal_bytes() == 0 {
            self.quit = Quit::Left;
            return;
        }
        if let WalState::Dirty { boot_stale: true, .. } = self.wal {
            self.conn.execute_batch(UNSYNCED).ok();
            let rebuilt = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate).and_then(|tx| {
                boot_record::rebuild(&tx)?;
                tx.commit()
            });
            self.conn.execute_batch(SYNCED).ok();
            if rebuilt.is_ok() {
                self.wal_event(WalEvent::Rebuilt);
            }
        }
        match self.leave() {
            Ok(()) => {
                self.unsynced_since = None;
                self.wal_event(WalEvent::Left);
                // Closing the last connection would checkpoint the WAL into the file (and sync it):
                // the work this end leaves to the next launch.
                self.conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true).ok();
                self.quit = Quit::Left;
                rune::marks::mark("writer.left");
            }
            Err(why) => {
                if !why.is_empty() {
                    eprintln!("opencode-storage: the quit checkpoints the WAL: {why}");
                }
                self.wal_event(WalEvent::QuitRequested);
                self.quit = if self.wal == WalState::Clean { Quit::Left } else { Quit::Quitting };
            }
        }
        slow("the quit's last write", t);
    }

    /// The WAL synced and the record's copy written beside it, checked as the next launch reads
    /// it (`boot_record::read_raw` answering `Pending` with this record, at this end of the WAL).
    /// The error says why not (empty: not on this platform, or nothing that may be left).
    pub(super) fn leave(&mut self) -> Result<(), &'static str> {
        if !crate::native::journal::SUPPORTED || !matches!(self.wal, WalState::Derived | WalState::Dirty { boot_stale: false, .. }) {
            return Err("");
        }
        if self.repages() {
            return Err("");
        }
        let bytes = self.wal_bytes();
        if bytes > QUIT_WAL_BYTES {
            return Err("it's over the size the next launch checkpoints after its first frame");
        }
        let stamp = self.wal_file.as_ref().and_then(crate::native::walstamp::read).ok_or("it doesn't end in a commit")?;
        // Only commits not yet on disk need the sync: none when the WAL holds only the index's
        // (rebuildable; a power cut that loses them costs re-indexing) or was synced since.
        if let WalState::Dirty { unsynced: true, .. } = self.wal {
            self.wal_file.as_ref().ok_or("it can't be opened")?.sync_data().map_err(|_| "its sync failed")?;
            self.wal_event(WalEvent::Synced);
        }
        let record = boot_record::from_sqlite(&self.conn).ok().flatten().ok_or("no boot record")?;
        let path = self.conn.path().map(std::path::PathBuf::from).ok_or("no path")?;
        crate::native::bootcopy::write(&path, &record, &stamp).map_err(|_| "the record's copy couldn't be written")?;
        let file = crate::native::dbfile::get(&path).map_err(|_| "the file can't be read")?;
        match boot_record::read_file(&path, &file) {
            boot_record::FastBoot::Pending(copy, at) if copy == record && at == stamp => Ok(()),
            boot_record::FastBoot::Checkpointed(_, why) | boot_record::FastBoot::UseSqlite(why) => Err(why),
            _ => Err("the next launch wouldn't read the record's copy"),
        }
    }

    /// Whether this quit rewrites the file with 16 KiB pages (`repage`): that needs the WAL empty.
    pub(super) fn repages(&self) -> bool {
        let get = |pragma: &str| self.conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0);
        let (page, pages) = (get("page_size"), get("page_count"));
        page > REPAGE_TO && page * pages <= REPAGE_MAX
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
            && let Ok(version) = crate::native::schema::user_version(&self.conn)
        {
            crate::native::schema::set_flags(&self.conn, version & crate::native::schema::FLAG_MASK & !crate::native::schema::FLAG_BOOT_PAGE2).ok();
        }
        eprintln!("opencode-storage: {} {} pages of {page} B into {REPAGE_TO} B pages in {} us", if done.is_ok() { "rewrote" } else { "couldn't rewrite" }, pages, t.elapsed().as_micros());
    }
}
