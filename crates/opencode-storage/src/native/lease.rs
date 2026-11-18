//! The profile's lease: one process at a time maps the draft journal for writing and opens the
//! database. The app's instance claim doesn't cover it everywhere: on macOS `rune::single_instance`
//! admits every launch (Launch Services keeps one copy of an app, but `open -n`, a launch from a
//! shell, or the updater's relaunch after its 10 s wait still start a second process), and the
//! journal is mapped before SQLite's lock is taken.
//!
//! The lease is an OS lock on `<db>-boot`, a file of its own beside the database (so it never
//! disturbs SQLite's byte-range locks on the database): `flock` on macOS, and on Windows
//! `LockFileEx` on its first byte through a handle that shares everything, so a backup, an
//! indexer or a viewer that opens the file can't take the lease from the app. It's taken before
//! anything maps or writes the file and let go once the storage has closed (`release`, after the
//! writer's last write and its repage). A process that dies lets go with it: the system closes its
//! handle. A stale file never blocks: the lease is the lock, not the file. On a network volume
//! `flock` may be local to each machine, so two machines sharing one home can both hold it. Linux
//! keeps no journal, so it takes no lease (SQLite's own lock guards the file there).
//!
//! Who else writes the data folder, and whether they ask `holds`: the database and its journal
//! (only after the lease); `last-exit.bin` and `skip-restore.bin` (the app's `last_exit`: only in
//! the copy that holds the profile); the updater's downloads and staging (only started in that
//! copy); `glitch/` reports, the logs, `sounds/` and `shell-integration/` (the same bytes from
//! either copy, or appends), which don't ask.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

static HELD: Mutex<Option<File>> = Mutex::new(None);

/// Whether this process holds the profile: undecided until the storage's open decides it.
static HOLDS: AtomicU8 = AtomicU8::new(UNDECIDED);
const UNDECIDED: u8 = 0;
const HOLDS_IT: u8 = 1;
const NOT_HELD: u8 = 2;

pub(super) fn path(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push("-boot");
    PathBuf::from(p)
}

/// One try at the lease.
pub(super) enum Outcome {
    /// This process holds it now.
    Held,
    /// Another process holds it (another copy of the app, or one still closing its storage).
    Busy,
    /// The volume can't lock (a network home, a file system without locks): the run goes on as
    /// without a lease, as the builds before it did.
    Unsupported(io::Error),
    /// The file can't be opened or made (a read-only volume, a full disk): the database can't be
    /// written either.
    Unavailable(io::Error),
}

/// Tries once, without waiting. Linux: always held, with nothing held.
pub(super) fn try_take(db: &Path) -> Outcome {
    if !super::journal::SUPPORTED {
        return Outcome::Held;
    }
    if HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some() {
        return Outcome::Held;
    }
    let p = path(db);
    let file = match open(&p) {
        Ok(f) => f,
        // Another program opened it sharing nothing (none of the app's own opens do): a moment.
        Err(e) if cfg!(windows) && e.raw_os_error() == Some(32) => return Outcome::Busy,
        Err(e) => return Outcome::Unavailable(e),
    };
    match lock(&file) {
        Ok(()) => {
            *HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file);
            Outcome::Held
        }
        Err(e) if is_busy(&e) => Outcome::Busy,
        Err(e) => Outcome::Unsupported(e),
    }
}

#[cfg(not(windows))]
fn open(path: &Path) -> io::Result<File> {
    let mut opt = OpenOptions::new();
    opt.read(true).write(true).create(true).truncate(false);
    with_folder(path, || opt.open(path))
}

/// Windows: made if missing, then opened for reading only, sharing everything: a program that
/// opened it sharing only reading (a backup agent, a viewer) refuses writers, not readers, and
/// `LockFileEx` needs no more than read access.
#[cfg(windows)]
fn open(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};
    let all = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    if !path.exists() {
        let mut make = OpenOptions::new();
        make.write(true).create(true).truncate(false).share_mode(all);
        match with_folder(path, || make.open(path)) {
            Ok(_) => {}
            // Another program has it open sharing only reading: it exists, so it's readable.
            Err(e) if e.raw_os_error() == Some(32) => {}
            Err(e) => return Err(e),
        }
    }
    OpenOptions::new().read(true).share_mode(all).open(path)
}

fn with_folder(path: &Path, open: impl Fn() -> io::Result<File>) -> io::Result<File> {
    match open() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path.parent().ok_or(io::ErrorKind::NotFound)?)?;
            open()
        }
        other => other,
    }
}

#[cfg(unix)]
fn lock(f: &File) -> io::Result<()> {
    f.try_lock().map_err(io::Error::from)
}

#[cfg(unix)]
fn is_busy(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::WouldBlock
}

/// An exclusive lock on the file's first byte, failing at once when another handle has it. The
/// system lets go of it when the handle closes or the process ends.
#[cfg(windows)]
fn lock(f: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx};
    use windows_sys::Win32::System::IO::OVERLAPPED;
    // SAFETY: an all-zero OVERLAPPED is offset 0 with no event; the handle is the open file's,
    // alive for the call.
    #[allow(unsafe_code)]
    let ok = unsafe {
        let mut at: OVERLAPPED = std::mem::zeroed();
        LockFileEx(f.as_raw_handle(), LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &mut at)
    };
    if ok != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// ERROR_LOCK_VIOLATION: another handle holds the byte.
#[cfg(windows)]
fn is_busy(e: &io::Error) -> bool {
    e.raw_os_error() == Some(33)
}

#[cfg(not(any(unix, windows)))]
fn lock(_: &File) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(not(any(unix, windows)))]
fn is_busy(_: &io::Error) -> bool {
    false
}

/// What an open refused for another copy gives as its reason (`BootReport::error`): the app says
/// it in its own words.
pub const IN_USE: &str = "another copy of OpenCode has this profile open";

/// Lets go of the lease once the storage has closed: nothing of this process writes the file after.
pub(super) fn release() {
    HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
}

/// The open decided: this process holds the profile (it took the lease, or the platform or the
/// volume has none), or another copy does.
pub(super) fn decide(holds: bool) {
    HOLDS.store(if holds { HOLDS_IT } else { NOT_HELD }, Ordering::Release);
}

/// Whether this process holds the profile: none until the storage's open has decided.
pub fn holds() -> Option<bool> {
    match HOLDS.load(Ordering::Acquire) {
        HOLDS_IT => Some(true),
        NOT_HELD => Some(false),
        _ => None,
    }
}

/// Whether another process holds SQLite's lock on the database: a copy of a build from before
/// the lease, which never takes it but keeps SQLite's lock for its whole life (`locking_mode =
/// EXCLUSIVE`). Asked without waiting, through the storage's kept descriptor (`dbfile`), so it
/// drops no lock of this process's. macOS only: on Windows the instance claim keeps such a copy
/// out until its storage has closed.
#[cfg(target_os = "macos")]
pub(super) fn sqlite_held_elsewhere(db: &Path) -> bool {
    use std::os::fd::AsRawFd;
    // SQLite's unix VFS lock bytes: PENDING (0x40000000), RESERVED, then the 510-byte SHARED range.
    const PENDING: libc::off_t = 0x4000_0000;
    let Ok(f) = super::dbfile::get(db) else { return false };
    let mut l = libc::flock { l_start: PENDING, l_len: 512, l_pid: 0, l_type: libc::F_WRLCK as libc::c_short, l_whence: libc::SEEK_SET as libc::c_short };
    // SAFETY: F_GETLK fills `l` and takes no lock; the descriptor is open for the process's life.
    #[allow(unsafe_code)]
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETLK, &mut l) };
    r == 0 && l.l_type != libc::F_UNLCK as libc::c_short
}

#[cfg(not(target_os = "macos"))]
pub(super) fn sqlite_held_elsewhere(_: &Path) -> bool {
    false
}
