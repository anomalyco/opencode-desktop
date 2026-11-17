//! Fixed 64-byte session exit record (`<data>/last-exit.bin`).
//!
//! Opened off the UI thread after the first frame presents (`presented`), so nothing runs before
//! Frame 1 and no background thread or timer is kept at rest. A normal quit writes `Clean` ahead of
//! `storage::close()` (`mark_clean`). Fatal hooks in later commits overwrite those same 64 bytes in
//! place from a stack buffer (`WriteFile` / `pwrite`) without allocating. A launch that finds
//! `Running` knows only that the previous process vanished from outside (killed or power lost) and
//! never guesses why.
//!
//! Explicit lifecycle machine matching the process-termination split in Official's
//! `packages/desktop/src/main/windows/recovery-machine.ts` (`RecoveryIncident` `terminated`).
#![cfg_attr(not(target_arch = "wasm32"), allow(unsafe_code))]

const MAGIC: u32 = 0x4558_5431; // "EXT1"
const VERSION: u16 = 1;
pub const RECORD_BYTES: usize = 64;

const TAG_CLEAN: u8 = 0;
const TAG_RUNNING: u8 = 1;
const TAG_OUT_OF_MEMORY: u8 = 2;
const TAG_STACK_OVERFLOW: u8 = 3;
const TAG_FATAL: u8 = 4;

/// Why the previous process ended, recorded only when that process observed the cause itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Reason {
    OutOfMemory,
    StackOverflow,
    Fatal,
}

/// What `<data>/last-exit.bin` held from the previous session when this one opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Previous {
    /// First launch, unreadable record, or the previous session quit cleanly (`tag = 0`).
    Clean,
    /// Previous session left `tag = 1` (`Running`): external kill or power loss; never guessed.
    Interrupted { pid: u32, seq: u64 },
    /// Previous session wrote a verified fatal reason before exiting.
    Ended {
        reason: Reason,
        flags: u8,
        code: u32,
        pid: u32,
        requested_bytes: u64,
        avail_commit_bytes: u64,
        seq: u64,
    },
}

/// Session lifecycle of the exit record (`packages/desktop/src/main/windows/recovery-machine.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Before Frame 1 presents: no descriptor open, zero pre-frame I/O.
    Unopened,
    /// Descriptor open and initialized to `Running` (`seq`) for this session.
    Active { seq: u64, previous: Previous, taken: bool },
    /// Orderly quit wrote `Clean` ahead of `storage::close()`.
    Closed { seq: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Event {
    Opened { seq: u64, previous: Previous },
    TakePrevious,
    CleanQuit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    WriteRunning { seq: u64 },
    WriteClean { seq: u64 },
}

pub fn step(state: &mut State, event: Event) -> Option<Effect> {
    match (*state, event) {
        (State::Unopened, Event::Opened { seq, previous }) => {
            *state = State::Active { seq, previous, taken: false };
            Some(Effect::WriteRunning { seq })
        }
        (State::Unopened, Event::CleanQuit) => {
            *state = State::Closed { seq: 0 };
            None
        }
        (State::Active { seq, previous, .. }, Event::TakePrevious) => {
            *state = State::Active { seq, previous, taken: true };
            None
        }
        (State::Active { seq, .. }, Event::CleanQuit) => {
            *state = State::Closed { seq };
            Some(Effect::WriteClean { seq })
        }
        _ => None,
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

pub(crate) fn encode_record(
    tag: u8,
    flags: u8,
    code: u32,
    pid: u32,
    requested_bytes: u64,
    avail_commit_bytes: u64,
    seq: u64,
) -> [u8; RECORD_BYTES] {
    let mut buf = [0u8; RECORD_BYTES];
    buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    buf[4..6].copy_from_slice(&VERSION.to_le_bytes());
    buf[6] = tag;
    buf[7] = flags;
    buf[8..12].copy_from_slice(&code.to_le_bytes());
    buf[12..16].copy_from_slice(&pid.to_le_bytes());
    buf[16..24].copy_from_slice(&requested_bytes.to_le_bytes());
    buf[24..32].copy_from_slice(&avail_commit_bytes.to_le_bytes());
    buf[32..40].copy_from_slice(&seq.to_le_bytes());
    let sum = crc32(&buf[0..60]);
    buf[60..64].copy_from_slice(&sum.to_le_bytes());
    buf
}

fn decode_record(buf: &[u8; RECORD_BYTES]) -> Option<(Previous, u64)> {
    if u32::from_le_bytes(buf[0..4].try_into().ok()?) != MAGIC {
        return None;
    }
    if u16::from_le_bytes(buf[4..6].try_into().ok()?) != VERSION {
        return None;
    }
    if u32::from_le_bytes(buf[60..64].try_into().ok()?) != crc32(&buf[0..60]) {
        return None;
    }
    let tag = buf[6];
    let flags = buf[7];
    let code = u32::from_le_bytes(buf[8..12].try_into().ok()?);
    let pid = u32::from_le_bytes(buf[12..16].try_into().ok()?);
    let requested_bytes = u64::from_le_bytes(buf[16..24].try_into().ok()?);
    let avail_commit_bytes = u64::from_le_bytes(buf[24..32].try_into().ok()?);
    let seq = u64::from_le_bytes(buf[32..40].try_into().ok()?);
    let previous = match tag {
        TAG_CLEAN => Previous::Clean,
        TAG_RUNNING => Previous::Interrupted { pid, seq },
        TAG_OUT_OF_MEMORY => Previous::Ended { reason: Reason::OutOfMemory, flags, code, pid, requested_bytes, avail_commit_bytes, seq },
        TAG_STACK_OVERFLOW => Previous::Ended { reason: Reason::StackOverflow, flags, code, pid, requested_bytes, avail_commit_bytes, seq },
        TAG_FATAL => Previous::Ended { reason: Reason::Fatal, flags, code, pid, requested_bytes, avail_commit_bytes, seq },
        _ => return None,
    };
    Some((previous, seq))
}

#[cfg(target_arch = "wasm32")]
pub fn presented() {}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
pub fn mark_clean() {}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::io::Read as _;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};

    use super::{
        Effect, Event, Previous, RECORD_BYTES, State, TAG_CLEAN, TAG_RUNNING, decode_record,
        encode_record, step,
    };

    static STATE: Mutex<State> = Mutex::new(State::Unopened);
    /// Pre-opened raw descriptor (`HANDLE` on Windows, `RawFd` on Unix), `-1` until opened.
    pub(super) static RECORD_FD: AtomicIsize = AtomicIsize::new(-1);
    pub(super) static SEQ: AtomicU64 = AtomicU64::new(0);

    pub fn record_path() -> Option<PathBuf> {
        Some(crate::dirs::data()?.join("last-exit.bin"))
    }

    pub fn presented() {
        static STARTED: std::sync::Once = std::sync::Once::new();
        if crate::headless() {
            return;
        }
        STARTED.call_once(|| {
            let _ = rune::task::spawn("last-exit", open_once);
        });
    }

    fn open_once() {
        let Some(path) = record_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(mut file) = open_file(&path) else { return };
        let mut raw = [0u8; RECORD_BYTES];
        let read_all = file.read(&mut raw).ok() == Some(RECORD_BYTES);
        let (previous, prev_seq) = if read_all {
            decode_record(&raw).unwrap_or((Previous::Clean, 0))
        } else {
            (Previous::Clean, 0)
        };
        if previous != Previous::Clean {
            eprintln!("last-exit: previous session recorded {previous:?}");
        }
        let next_seq = prev_seq.wrapping_add(1);
        let effect = {
            let mut guard = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            step(&mut guard, Event::Opened { seq: next_seq, previous })
        };
        if let Some(Effect::WriteRunning { seq }) = effect {
            let fd = into_raw_fd(file);
            SEQ.store(seq, Ordering::Relaxed);
            let buf = encode_record(TAG_RUNNING, 0, 0, std::process::id(), 0, u64::MAX, seq);
            write_raw(fd, &buf);
            RECORD_FD.store(fd, Ordering::Release);
        }
    }

    /// Writes `Clean` (`tag = 0`) ahead of `storage::close()`.
    pub fn mark_clean() {
        let effect = {
            let mut guard = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            step(&mut guard, Event::CleanQuit)
        };
        if let Some(Effect::WriteClean { seq }) = effect {
            let fd = RECORD_FD.load(Ordering::Acquire);
            if fd != -1 {
                let buf = encode_record(TAG_CLEAN, 0, 0, std::process::id(), 0, u64::MAX, seq);
                write_raw(fd, &buf);
            }
        }
    }

    #[allow(dead_code)]
    pub fn take_previous() -> Option<Previous> {
        let mut guard = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let State::Active { previous, taken: false, .. } = *guard else { return None };
        let _ = step(&mut guard, Event::TakePrevious);
        Some(previous)
    }

    #[cfg(windows)]
    fn open_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)
    }

    #[cfg(unix)]
    fn open_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new().read(true).write(true).create(true).open(path)
    }

    #[cfg(windows)]
    fn into_raw_fd(file: std::fs::File) -> isize {
        use std::os::windows::io::IntoRawHandle as _;
        file.into_raw_handle() as isize
    }

    #[cfg(unix)]
    fn into_raw_fd(file: std::fs::File) -> isize {
        use std::os::fd::IntoRawFd as _;
        file.into_raw_fd() as isize
    }

    #[cfg(windows)]
    pub(super) fn write_raw(fd: isize, buf: &[u8; RECORD_BYTES]) {
        use windows_sys::Win32::Storage::FileSystem::{FILE_BEGIN, SetFilePointerEx, WriteFile};
        if fd == -1 || fd == 0 {
            return;
        }
        let handle = fd as windows_sys::Win32::Foundation::HANDLE;
        let mut written = 0u32;
        // SAFETY: `handle` is our open file descriptor kept for the process's life; `buf` lives on
        // the stack. A completed `WriteFile` lands in the OS file cache and survives process abort.
        unsafe {
            if SetFilePointerEx(handle, 0, core::ptr::null_mut(), FILE_BEGIN) != 0 {
                let _ = WriteFile(
                    handle,
                    buf.as_ptr(),
                    RECORD_BYTES as u32,
                    &raw mut written,
                    core::ptr::null_mut(),
                );
            }
        }
    }

    #[cfg(unix)]
    pub(super) fn write_raw(fd: isize, buf: &[u8; RECORD_BYTES]) {
        if fd < 0 {
            return;
        }
        // SAFETY: `fd` is our open descriptor; `pwrite` at offset 0 is async-signal-safe and does
        // not allocate.
        unsafe {
            let _ = libc::pwrite(fd as libc::c_int, buf.as_ptr().cast(), RECORD_BYTES, 0);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use imp::{mark_clean, presented};
