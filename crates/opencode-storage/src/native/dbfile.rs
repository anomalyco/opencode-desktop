//! The storage's one descriptor on the database file outside SQLite, opened once and never
//! closed while the process runs.
//!
//! On macOS and Linux SQLite's locks are POSIX advisory locks, and a process loses every lock it
//! holds on a file the moment any of its descriptors on that file closes. SQLite takes care of
//! its own descriptors; one the storage opened and dropped while the writer's connection holds
//! the file (`locking_mode = EXCLUSIVE`) would silently hand the lock to any other process. So the
//! draft journal's mapping, its page reads and the reservation's sync all go through this one,
//! which is never closed (on Windows, where locks belong to a handle, it changes nothing).
//!
//! The boot record's read before the first frame (`boot_record::read_raw`) keeps its own short
//! handle: it closes before SQLite first opens the file, when no lock exists yet.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static FILE: OnceLock<(PathBuf, File)> = OnceLock::new();

/// The descriptor on the database, opened (read and write) at `path` the first time it's asked
/// for. This process has one database: later calls get the same descriptor whatever spelling of
/// its path they have (SQLite reports it absolute).
pub(crate) fn get(path: &Path) -> io::Result<&'static File> {
    if let Some((_, file)) = FILE.get() {
        return Ok(file);
    }
    let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    // Its first callers (the journal's reservation and attach) both run on the open's thread, so
    // no second first open can race this one and be dropped.
    let _ = FILE.set((path.to_path_buf(), file));
    FILE.get().map(|(_, f)| f).ok_or_else(|| io::Error::other("the database's descriptor"))
}
