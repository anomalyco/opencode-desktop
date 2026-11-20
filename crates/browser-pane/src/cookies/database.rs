//! Private SQLite database snapshot helper for reading live browser cookie databases without
//! acquiring locks or mutating the source browser's files (`profile/database.ts`), plus the
//! startup persistence disk probe (`profile/probe.ts`).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::{ImportFailure, PROBE_COOKIE_HOST, PROBE_COOKIE_NAME};

const PREFIX: &str = "opencode-browser-import-";
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
    let base_name = file
        .file_name()
        .ok_or(ImportFailure::Read)?;
    let copy = dir.join(base_name);
    fs::copy(file, &copy).map_err(|e| io_failure(&e))?;

    for suffix in ["-wal", "-shm", "-journal"] {
        let mut src = file.as_os_str().to_os_string();
        src.push(suffix);
        let mut dst = copy.as_os_str().to_os_string();
        dst.push(suffix);
        fs::copy(PathBuf::from(src), PathBuf::from(dst)).ok();
    }

    // Open read-write on our private copy so SQLite can replay any copied `-wal` frames cleanly.
    let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .or_else(|_| Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY))
        .map_err(|_| ImportFailure::Read)?;
    let result = read(&conn).map_err(|_| ImportFailure::Read);
    drop(conn);
    result
}

/// Sweeps any stale snapshot directories left behind by a crashed process.
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
        if is_ours && !live.contains(&path) {
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
