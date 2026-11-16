//! Ownership of a writable journal outlives the UI instance claim. The lease is on the boot-copy
//! file, not on the SQLite file: Windows sharing and BSD flock do not disturb SQLite's byte locks.
//! Never unlink the lease name or replace its inode to publish a copy. Death closes the handle.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) enum Owner { WithoutJournal, Exclusive(File) }
impl Owner {
    pub(super) fn saved(&self) {
        if matches!(self, Self::Exclusive(_)) { super::journal::finish(true); }
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        // Also fences an early-return or unwinding writer: a process-global mapping must no
        // longer accept records by the time another process can obtain our file handle.
        if matches!(self, Self::Exclusive(_)) { super::journal::finish(false); }
    }
}
enum State { Trying, Waiting, Owned(Owner), Refused(io::Error) }
enum Event { Acquired(Owner), Busy, Retry, Deadline, Failed(io::Error) }
enum Effect { Try, Wait, Done }

fn step(s: &mut State, event: Event) -> Effect {
    match (&*s, event) {
        (State::Trying, Event::Acquired(f)) => { *s = State::Owned(f); Effect::Done }
        (State::Trying, Event::Busy) => { *s = State::Waiting; Effect::Wait }
        (State::Waiting, Event::Retry) => { *s = State::Trying; Effect::Try }
        (State::Waiting, Event::Deadline) => {
            *s = State::Refused(io::Error::new(io::ErrorKind::WouldBlock, "another process is still saving this profile")); Effect::Done
        }
        (State::Trying, Event::Failed(e)) => { *s = State::Refused(e); Effect::Done }
        _ => Effect::Done,
    }
}

fn open(path: &Path) -> io::Result<File> {
    let mut opt = OpenOptions::new();
    opt.read(true).write(true).create(true).truncate(false);
    // Sole writer/deleter; a stopped successor may still read a checksummed snapshot.
    #[cfg(windows)] {
        use std::os::windows::fs::OpenOptionsExt;
        opt.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
    }
    let f = match opt.open(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path.parent().ok_or(io::ErrorKind::NotFound)?)?;
            opt.open(path)?
        }
        other => other?,
    };
    #[cfg(unix)] f.try_lock().map_err(io::Error::from)?;
    Ok(f)
}

/// Only a grace period for a file owner on its way out, not a process wait. An occupied lease
/// becomes an explicit memory-only open instead of ever attaching a second writable mapping.
pub(super) fn acquire(db: &Path) -> io::Result<Owner> {
    if !super::journal::SUPPORTED { return Ok(Owner::WithoutJournal); }
    let p = super::bootcopy::path(db);
    let until = Instant::now() + Duration::from_millis(12);
    let mut s = State::Trying;
    let mut effect = Effect::Try;
    loop {
        let event = match effect {
            Effect::Try => match open(&p) {
                Ok(f) => Event::Acquired(Owner::Exclusive(f)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || cfg!(windows) && e.raw_os_error() == Some(32) => Event::Busy,
                Err(e) => Event::Failed(e),
            },
            Effect::Wait if Instant::now() >= until => Event::Deadline,
            Effect::Wait => { std::thread::sleep(Duration::from_millis(1)); Event::Retry }
            Effect::Done => return match s { State::Owned(f) => Ok(f), State::Refused(e) => Err(e), _ => Err(io::Error::other("invalid profile lease transition")) },
        };
        effect = step(&mut s, event);
    }
}
