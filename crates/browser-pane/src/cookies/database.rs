//! Private SQLite database snapshot helper for reading live browser cookie databases without
//! acquiring locks or mutating the source browser's files (`profile/database.ts`), plus bounded
//! no-symlink file reading and the startup persistence disk probe (`profile/probe.ts`).

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::{ImportFailure, PROBE_COOKIE_HOST, PROBE_COOKIE_NAME};

const PREFIX: &str = "opencode-browser-import-";
/// Hard upper bound per copied SQLite cookie database or WAL/journal sidecar (128 MiB).
pub const MAX_COOKIE_DB_BYTES: u64 = 128 * 1024 * 1024;
static NEXT_SNAP_ID: AtomicU64 = AtomicU64::new(1);

/// Private scratch root where temporary SQLite copies live only for the duration of one read.
pub struct SnapshotScratch {
    pub directory: PathBuf,
    live: Mutex<HashSet<PathBuf>>,
}

impl SnapshotScratch {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            live: Mutex::new(HashSet::new()),
        }
    }

    /// Default scratch directory inside the app's own private state dir (never shared `/tmp`).
    pub fn default_dir() -> PathBuf {
        crate::profile::try_shared_profile_dir()
            .and_then(|p| p.parent().map(|d| d.join("import-scratch")))
            .unwrap_or_else(|| std::env::temp_dir().join("opencode-browser-import-scratch"))
    }
}

/// Maps an OS I/O error opening/copying another browser's cookie DB to `ImportFailure`.
pub fn io_failure(err: &io::Error) -> ImportFailure {
    if err.kind() == io::ErrorKind::PermissionDenied {
        return ImportFailure::Access;
    }
    #[cfg(windows)]
    {
        // ERROR_SHARING_VIOLATION (32) or ERROR_LOCK_VIOLATION (33): Chromium holds `Cookies`
        // open without sharing while running on Windows.
        if matches!(err.raw_os_error(), Some(32 | 33)) {
            return ImportFailure::Locked;
        }
    }
    #[cfg(unix)]
    {
        if matches!(err.raw_os_error(), Some(libc::EBUSY)) {
            return ImportFailure::Locked;
        }
        if matches!(err.raw_os_error(), Some(libc::EACCES | libc::EPERM)) {
            return ImportFailure::Access;
        }
    }
    ImportFailure::Read
}

/// Opens a regular, non-symlink file read-only (with `O_NOFOLLOW | O_NONBLOCK` on Unix) and
/// verifies the open handle's own metadata is a regular file within `max_bytes`.
fn open_regular_nofollow(path: &Path, max_bytes: u64) -> Result<File, ImportFailure> {
    let lmeta = fs::symlink_metadata(path).map_err(|e| io_failure(&e))?;
    if lmeta.file_type().is_symlink() || !lmeta.is_file() || lmeta.len() > max_bytes {
        return Err(ImportFailure::Read);
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = opts.open(path).map_err(|e| io_failure(&e))?;
    let fmeta = file.metadata().map_err(|e| io_failure(&e))?;
    if !fmeta.is_file() || fmeta.len() > max_bytes {
        return Err(ImportFailure::Read);
    }
    Ok(file)
}

/// Reads a regular, non-symlink file into memory, capped strictly at `max_bytes` on the open handle.
pub fn read_regular_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, ImportFailure> {
    let file = open_regular_nofollow(path, max_bytes)?;
    let mut buf = Vec::new();
    let limit = max_bytes.saturating_add(1);
    file.take(limit)
        .read_to_end(&mut buf)
        .map_err(|e| io_failure(&e))?;
    if (buf.len() as u64) > max_bytes {
        return Err(ImportFailure::Read);
    }
    Ok(buf)
}

/// Copies a regular, non-symlink file into `dst` (created with mode `0o600` on Unix), capped at
/// `max_bytes` directly on the open read handle.
fn copy_regular_bounded(src: &Path, dst: &Path, max_bytes: u64) -> Result<(), ImportFailure> {
    let mut reader = open_regular_nofollow(src, max_bytes)?.take(max_bytes.saturating_add(1));
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut writer = opts.open(dst).map_err(|e| io_failure(&e))?;
    let copied = io::copy(&mut reader, &mut writer).map_err(|e| io_failure(&e))?;
    if copied > max_bytes {
        return Err(ImportFailure::Read);
    }
    Ok(())
}

/// Copies `file` (and optional `-wal`, `-shm`, `-journal` sidecars) into a `0o700` private
/// subdirectory of `scratch`, runs `read(&conn)` on the copy, and deletes the copy unconditionally.
pub fn snapshot_database<T, F>(
    file: &Path,
    scratch: &SnapshotScratch,
    read: F,
) -> Result<T, ImportFailure>
where
    F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
{
    create_private_dir(&scratch.directory).map_err(|e| io_failure(&e))?;
    sweep_snapshots(scratch);

    let seq = NEXT_SNAP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = scratch
        .directory
        .join(format!("{PREFIX}{}-{seq}", std::process::id()));
    if let Ok(mut live) = scratch.live.lock() {
        live.insert(dir.clone());
    }

    struct Cleanup<'a> {
        dir: PathBuf,
        scratch: &'a SnapshotScratch,
    }
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            remove_dir_retry(&self.dir);
            if let Ok(mut live) = self.scratch.live.lock() {
                live.remove(&self.dir);
            }
        }
    }
    let _guard = Cleanup {
        dir: dir.clone(),
        scratch,
    };

    create_private_dir(&dir).map_err(|e| io_failure(&e))?;
    let base_name = file.file_name().ok_or(ImportFailure::Read)?;
    let copy = dir.join(base_name);
    copy_regular_bounded(file, &copy, MAX_COOKIE_DB_BYTES)?;

    for suffix in ["-wal", "-shm", "-journal"] {
        let mut src = file.as_os_str().to_os_string();
        src.push(suffix);
        let mut dst = copy.as_os_str().to_os_string();
        dst.push(suffix);
        copy_regular_bounded(&PathBuf::from(src), &PathBuf::from(dst), MAX_COOKIE_DB_BYTES).ok();
    }

    // Open read-write on our private copy so SQLite can replay any copied `-wal` frames cleanly.
    let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .or_else(|_| Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY))
        .map_err(|_| ImportFailure::Read)?;
    let result = read(&conn).map_err(|_| ImportFailure::Read);
    drop(conn);
    result
}

/// Copies the regular, non-symlink files of `directory` (not its `LOCK`, not subdirectories) into
/// a `0o700` private subdirectory of `scratch`, at most `max_bytes` in all, runs `read` on the copy,
/// and deletes the copy unconditionally: a LevelDB folder read without touching the live one.
pub fn snapshot_directory<T>(
    directory: &Path,
    scratch: &SnapshotScratch,
    max_bytes: u64,
    read: impl FnOnce(&Path) -> Result<T, ImportFailure>,
) -> Result<T, ImportFailure> {
    create_private_dir(&scratch.directory).map_err(|e| io_failure(&e))?;
    sweep_snapshots(scratch);
    let seq = NEXT_SNAP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = scratch.directory.join(format!("{PREFIX}{}-{seq}", std::process::id()));
    if let Ok(mut live) = scratch.live.lock() {
        live.insert(dir.clone());
    }
    struct Cleanup<'a> {
        dir: PathBuf,
        scratch: &'a SnapshotScratch,
    }
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            remove_dir_retry(&self.dir);
            if let Ok(mut live) = self.scratch.live.lock() {
                live.remove(&self.dir);
            }
        }
    }
    let _guard = Cleanup { dir: dir.clone(), scratch };
    create_private_dir(&dir).map_err(|e| io_failure(&e))?;
    let meta = fs::symlink_metadata(directory).map_err(|e| io_failure(&e))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(ImportFailure::Read);
    }
    let mut left = max_bytes;
    for entry in fs::read_dir(directory).map_err(|e| io_failure(&e))?.flatten() {
        let name = entry.file_name();
        let Ok(kind) = entry.file_type() else { continue };
        if !kind.is_file() || name == "LOCK" {
            continue;
        }
        // A live log may grow while it is copied: each copy is held to what the budget has left.
        let copy = dir.join(&name);
        copy_regular_bounded(&entry.path(), &copy, left)?;
        left = left.saturating_sub(fs::metadata(&copy).map_or(left, |m| m.len()));
    }
    read(&dir)
}

/// Sweeps any stale non-symlink snapshot directories left behind by a crashed process.
pub fn sweep_snapshots(scratch: &SnapshotScratch) {
    let Ok(entries) = fs::read_dir(&scratch.directory) else {
        return;
    };
    let live: HashSet<PathBuf> = scratch
        .live
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_ours = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(PREFIX));
        let real_dir = fs::symlink_metadata(&path)
            .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir());
        if is_ours && real_dir && !live.contains(&path) {
            remove_dir_retry(&path);
        }
    }
}

/// Checks whether the short-lived `PROBE_COOKIE` actually reached Chromium's SQLite cookie DB on
/// disk under `cache_dir` (`profile/probe.ts`).
///
/// Returns `true` if the probe cookie is found on disk OR if the DB cannot be read (e.g. Windows
/// exclusive lock, matching upstream `read.length === 0 || read.some(c => c > 0)`). Returns `false`
/// when a readable cookie DB exists on disk yet holds 0 rows for the flushed probe cookie (denied
/// macOS Keychain or locked Linux keyring).
pub fn probe_disk_cookies(cache_dir: &Path, scratch: &SnapshotScratch) -> bool {
    let candidates = [
        cache_dir.join("Network").join("Cookies"),
        cache_dir.join("Cookies"),
    ];
    let mut readable = 0usize;
    let mut found = false;
    for file in candidates {
        if !file.is_file() {
            continue;
        }
        let count = snapshot_database(&file, scratch, |conn| {
            conn.query_row(
                "SELECT count(*) FROM cookies WHERE host_key = ?1 AND name = ?2",
                [PROBE_COOKIE_HOST, PROBE_COOKIE_NAME],
                |row| row.get::<_, i64>(0),
            )
        });
        if let Ok(n) = count {
            readable += 1;
            if n > 0 {
                found = true;
            }
        }
    }
    readable == 0 || found
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).ok();
    }
    Ok(())
}

fn remove_dir_retry(dir: &Path) {
    if fs::remove_dir_all(dir).is_err() {
        std::thread::sleep(Duration::from_millis(200));
        fs::remove_dir_all(dir).ok();
    }
}
