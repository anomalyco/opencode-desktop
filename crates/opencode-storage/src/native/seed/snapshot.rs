//! Read-only scratch-copy snapshot of Official Desktop's `drafts.sqlite` (+ `-wal`) inside OUR
//! profile directory so Official's folder is never written to, never has `-shm` created in it,
//! catches mid-checkpoint DB/WAL skew, recovers live WAL frames cleanly, and verifies
//! `PRAGMA integrity_check` before reading a row.

use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};

/// Size caps on Official Desktop files copied/read during first-run seed.
pub(super) const MAX_SEED_DB_BYTES: u64 = 128 * 1024 * 1024;
pub(super) const MAX_SEED_WAL_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_SETTINGS_JSON_BYTES: usize = 256 * 1024;
pub(super) const MAX_WINDOW_STATE_BYTES: usize = 64 * 1024;

const MAX_COPY_RETRIES: usize = 3;
const SEED_COPY_PREFIX: &str = ".seed-copy-";
static COPY_SEQ: AtomicU64 = AtomicU64::new(1);

/// Percent-encodes a filesystem path for a SQLite `file:` URI (`%`, `?`, `#`, spaces, non-ASCII).
pub(super) fn encode_uri_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut out = String::with_capacity(raw.len() + 16);
    for b in raw.bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'-' | b'_' | b'.' | b'~');
        if keep {
            out.push(b as char);
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    out
}

/// Bounded plain shared read of a UTF-8 file (never follows symlinks, never reads past `max_bytes`).
pub(super) fn read_capped(path: &Path, max_bytes: usize) -> Result<Option<String>, String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    if meta.len() > max_bytes as u64 {
        return Err(format!("{}: exceeds {} B cap ({} B)", path.display(), max_bytes, meta.len()));
    }
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = Vec::with_capacity((meta.len() as usize).min(max_bytes));
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if buf.len() > max_bytes {
        return Err(format!("{}: grew past {} B cap", path.display(), max_bytes));
    }
    String::from_utf8(buf).map(Some).map_err(|e| format!("{}: invalid utf-8: {e}", path.display()))
}

/// Bounded plain shared copy (`File::open` takes no exclusive lock and never writes to `src`'s dir).
/// Returns `Ok(false)` if `src` does not exist.
fn copy_file_capped(src: &Path, dst: &Path, max_bytes: u64) -> Result<bool, String> {
    let meta = match fs::symlink_metadata(src) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("stat {}: {e}", src.display())),
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(format!("{}: not a regular file", src.display()));
    }
    if meta.len() > max_bytes {
        return Err(format!("{}: exceeds {} B cap ({} B)", src.display(), max_bytes, meta.len()));
    }
    let mut reader = File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?.take(max_bytes + 1);
    let mut writer = File::create(dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
    let copied = io::copy(&mut reader, &mut writer).map_err(|e| format!("copy {}: {e}", src.display()))?;
    if copied > max_bytes {
        return Err(format!("{}: grew past {} B cap during copy", src.display(), max_bytes));
    }
    writer.flush().map_err(|e| format!("flush {}: {e}", dst.display()))?;
    Ok(true)
}

#[derive(Clone, PartialEq, Eq)]
struct DbWalStamp {
    db_len: u64,
    db_change_counter: [u8; 4],
    wal_len: u64,
    /// First 24 bytes of `-wal` (magic, format, page size, checkpoint seq, salt1, salt2).
    wal_header: Option<[u8; 24]>,
}

fn read_prefix<const N: usize>(path: &Path) -> Result<Option<(u64, [u8; N])>, String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("stat {}: {e}", path.display())),
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    let len = meta.len();
    let mut buf = [0u8; N];
    if len >= N as u64 {
        let mut f = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        f.read_exact(&mut buf).map_err(|e| format!("read header {}: {e}", path.display()))?;
    }
    Ok(Some((len, buf)))
}

fn read_stamp(src_db: &Path, src_wal: &Path) -> Result<DbWalStamp, String> {
    let Some((db_len, db_hdr)) = read_prefix::<28>(src_db)? else {
        return Err("drafts.sqlite vanished".into());
    };
    let db_change_counter = [db_hdr[24], db_hdr[25], db_hdr[26], db_hdr[27]];
    let (wal_len, wal_header) = match read_prefix::<24>(src_wal)? {
        Some((len, hdr)) if len >= 24 => (len, Some(hdr)),
        Some((len, _)) => (len, None),
        None => (0, None),
    };
    Ok(DbWalStamp { db_len, db_change_counter, wal_len, wal_header })
}

/// True when no checkpoint or WAL reset/truncation happened between `before` and `after`.
/// Normal concurrent WAL appends keep `db_len`, `db_change_counter`, and WAL header salts identical
/// while `wal_len` stays the same or grows (SQLite validates each copied frame's salt + checksum).
fn stamp_consistent(before: &DbWalStamp, after: &DbWalStamp) -> bool {
    before.db_len == after.db_len
        && before.db_change_counter == after.db_change_counter
        && before.wal_header == after.wal_header
        && after.wal_len >= before.wal_len
}

/// Sweeps any leftover `.seed-copy-*` directories left by a prior hard kill mid-copy.
fn sweep_stale_copies(scratch_parent: &Path) {
    let Ok(entries) = fs::read_dir(scratch_parent) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(SEED_COPY_PREFIX) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Private verified copy of Official Desktop's `drafts.sqlite` inside OUR profile directory.
/// Dropping closes the SQLite connection first, then deletes the scratch folder.
pub(super) struct Snapshot {
    dir: PathBuf,
    conn: Option<Connection>,
}

impl Snapshot {
    pub(super) fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("snapshot connection alive until drop")
    }

    pub(super) fn has_table(&self, name: &str) -> bool {
        self.conn()
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |_| Ok(()))
            .is_ok()
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        self.conn.take();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub(super) enum SnapshotErr {
    NotFound,
    Retryable(String),
}

/// Copies `official_dir/drafts.sqlite` (+ `-wal` if present; never `-shm`, which SQLite rebuilds
/// via `walIndexRecover` and which Windows locks on bytes 120..128) into a private directory under
/// `scratch_parent` (inside OUR profile), checks DB change counter + WAL salts across the copy to
/// detect mid-checkpoint skew, recovers the copied WAL, and verifies `PRAGMA integrity_check(1) == "ok"`.
pub(super) fn open_snapshot(official_dir: &Path, scratch_parent: &Path) -> Result<Snapshot, SnapshotErr> {
    let src_db = official_dir.join("drafts.sqlite");
    if !src_db.is_file() {
        return Err(SnapshotErr::NotFound);
    }
    fs::create_dir_all(scratch_parent).map_err(|e| SnapshotErr::Retryable(format!("scratch parent: {e}")))?;
    sweep_stale_copies(scratch_parent);

    let src_wal = official_dir.join("drafts.sqlite-wal");
    let mut last_err = String::new();
    for attempt in 0..MAX_COPY_RETRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(50u64 << (attempt - 1)));
        }
        let seq = COPY_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = scratch_parent.join(format!("{SEED_COPY_PREFIX}{}-{seq}-{attempt}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        if let Err(e) = fs::create_dir_all(&dir) {
            last_err = format!("create scratch dir: {e}");
            continue;
        }
        let dst_db = dir.join("drafts.sqlite");
        let dst_wal = dir.join("drafts.sqlite-wal");

        let copied = (|| -> Result<(), String> {
            let before = read_stamp(&src_db, &src_wal)?;
            if !copy_file_capped(&src_db, &dst_db, MAX_SEED_DB_BYTES)? {
                return Err("drafts.sqlite vanished".into());
            }
            let _ = copy_file_capped(&src_wal, &dst_wal, MAX_SEED_WAL_BYTES)?;
            let after = read_stamp(&src_db, &src_wal)?;
            if !stamp_consistent(&before, &after) {
                return Err("drafts.sqlite or WAL checkpointed mid-copy".into());
            }
            Ok(())
        })();
        if let Err(e) = copied {
            let _ = fs::remove_dir_all(&dir);
            last_err = e;
            continue;
        }

        let uri = format!("file:{}?mode=rw", encode_uri_path(&dst_db));
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
        match Connection::open_with_flags(&uri, flags) {
            Ok(conn) => {
                conn.busy_timeout(Duration::from_millis(250)).ok();
                let check: Result<String, _> = conn.query_row("PRAGMA integrity_check(1)", [], |r| r.get(0));
                match check {
                    Ok(ref s) if s.eq_ignore_ascii_case("ok") => {
                        return Ok(Snapshot { dir, conn: Some(conn) });
                    }
                    Ok(other) => {
                        drop(conn);
                        let _ = fs::remove_dir_all(&dir);
                        last_err = format!("integrity_check failed: {other}");
                    }
                    Err(e) => {
                        drop(conn);
                        let _ = fs::remove_dir_all(&dir);
                        last_err = format!("integrity_check error: {e}");
                    }
                }
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&dir);
                last_err = format!("open snapshot copy: {e}");
            }
        }
    }
    Err(SnapshotErr::Retryable(last_err))
}

pub(super) enum PickResult {
    None,
    Picked(PathBuf, Snapshot),
    Failed(String),
}

/// Picks the newest valid Official Desktop `userData` folder among `dirs`, supporting both modern
/// `state`-table databases and older pre-`state` (`document`/`blob`-only) databases.
pub(super) fn pick_dir(dirs: &[PathBuf], scratch_parent: &Path) -> PickResult {
    let mut best: Option<(i64, PathBuf, Snapshot)> = None;
    let mut last_err: Option<String> = None;

    for dir in dirs {
        let snap = match open_snapshot(dir, scratch_parent) {
            Ok(s) => s,
            Err(SnapshotErr::NotFound) => continue,
            Err(SnapshotErr::Retryable(why)) => {
                last_err = Some(format!("{}: {why}", dir.display()));
                continue;
            }
        };
        let has_state = snap.has_table("state");
        let has_doc = snap.has_table("document");
        if !has_state && !has_doc {
            continue;
        }
        let newest: i64 = if has_state {
            match snap.conn().query_row("SELECT COALESCE(MAX(updated_at), 0) FROM state", [], |r| r.get(0)) {
                Ok(n) => n,
                Err(e) => {
                    last_err = Some(format!("{}: query state: {e}", dir.display()));
                    continue;
                }
            }
        } else {
            fs::metadata(dir.join("drafts.sqlite"))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_millis()).ok())
                .unwrap_or(1)
        };
        if best.as_ref().is_none_or(|(b, _, _)| newest > *b) {
            best = Some((newest, dir.clone(), snap));
        }
    }

    match best {
        Some((_, d, s)) => PickResult::Picked(d, s),
        None => match last_err {
            Some(why) => PickResult::Failed(why),
            None => PickResult::None,
        },
    }
}
