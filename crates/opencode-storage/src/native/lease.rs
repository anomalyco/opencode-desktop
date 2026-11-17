//! The profile's lease: one process at a time maps the draft journal for writing and opens the
//! database. The app's instance claim doesn't cover it everywhere: on macOS `rune::single_instance`
//! admits every launch (Launch Services keeps one copy of an app, but `open -n`, a launch from a
//! shell or a launch while the running copy exits still start a second process), and the journal
//! is mapped before SQLite's lock is taken. So a second process on the profile runs in memory and
//! never writes the file.
//!
//! The lease is an exclusive hold on `<db>-boot` (a file of its own, so it never disturbs SQLite's
//! byte-range locks on the database): `flock` on macOS, a handle that shares only reading on
//! Windows. It's taken after the instance claim and before the journal is mapped, and kept until
//! the storage has closed (`release`, after the writer's last write and its repage). A process
//! that dies lets go with it: the system closes its handle. Linux keeps no journal, so it takes
//! no lease (SQLite's own lock guards the file there).

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a launch waits for the lease: a grace for an owner already on its way out (its
/// storage closing), not a wait for a running one.
const GRACE: Duration = Duration::from_millis(12);

static HELD: Mutex<Option<File>> = Mutex::new(None);

pub(super) fn path(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push("-boot");
    PathBuf::from(p)
}

enum State {
    Trying,
    Waiting { until: Instant },
    Held(File),
    Refused(io::Error),
}

enum Event {
    Acquired(File),
    Busy,
    Failed(io::Error),
    Tick,
}

/// What the caller does next: try the file again, sleep a millisecond first, or stop.
enum Effect {
    Try,
    Sleep,
    Done,
}

fn step(s: &mut State, event: Event) -> Effect {
    match (std::mem::replace(s, State::Trying), event) {
        (State::Trying | State::Waiting { .. }, Event::Acquired(f)) => {
            *s = State::Held(f);
            Effect::Done
        }
        (State::Trying, Event::Busy) => {
            *s = State::Waiting { until: Instant::now() + GRACE };
            Effect::Sleep
        }
        (State::Waiting { until }, Event::Busy) if Instant::now() < until => {
            *s = State::Waiting { until };
            Effect::Sleep
        }
        (State::Waiting { .. }, Event::Busy) => {
            *s = State::Refused(io::Error::new(io::ErrorKind::WouldBlock, "another process of the app has this profile open"));
            Effect::Done
        }
        (State::Waiting { until }, Event::Tick) => {
            *s = State::Waiting { until };
            Effect::Try
        }
        (_, Event::Failed(e)) => {
            *s = State::Refused(e);
            Effect::Done
        }
        (other, _) => {
            *s = other;
            Effect::Done
        }
    }
}

fn open(path: &Path) -> io::Result<File> {
    let mut opt = OpenOptions::new();
    opt.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
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
    #[cfg(unix)]
    f.try_lock().map_err(io::Error::from)?;
    Ok(f)
}

/// Takes the profile's lease for this process, or says why not (another process of the app holds
/// it). Linux: always granted, with nothing held.
pub(super) fn acquire(db: &Path) -> io::Result<()> {
    if !super::journal::SUPPORTED {
        return Ok(());
    }
    let p = path(db);
    let mut s = State::Trying;
    let mut effect = Effect::Try;
    loop {
        let event = match effect {
            Effect::Try => match open(&p) {
                Ok(f) => Event::Acquired(f),
                // ERROR_SHARING_VIOLATION: another process has it open for writing.
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || cfg!(windows) && e.raw_os_error() == Some(32) => Event::Busy,
                Err(e) => Event::Failed(e),
            },
            Effect::Sleep => {
                std::thread::sleep(Duration::from_millis(1));
                Event::Tick
            }
            Effect::Done => {
                return match s {
                    State::Held(f) => {
                        *HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(f);
                        Ok(())
                    }
                    State::Refused(e) => Err(e),
                    State::Trying | State::Waiting { .. } => Err(io::Error::other("the profile lease ended in no state")),
                };
            }
        };
        effect = step(&mut s, event);
    }
}

/// Lets go of the lease once the storage has closed: nothing of this process writes the file after.
pub(super) fn release() {
    HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
}
