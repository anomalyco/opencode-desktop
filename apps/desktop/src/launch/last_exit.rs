//! Fixed 64-byte session exit record (`<data>/last-exit.bin`) and out-of-memory hooks.
//!
//! Opened off the UI thread after the first frame presents (`presented`), so nothing runs before
//! Frame 1 and no background thread or timer is kept at rest. A normal quit writes `Clean` ahead of
//! `storage::close()` (`mark_clean`). When an allocation returns null (`OomGuard`) or Windows
//! dispatches an unhandled out-of-memory / commit-short stack-growth exception
//! (`SetUnhandledExceptionFilter`, zero first-chance overhead), the failing thread overwrites those
//! 64 bytes in place from a stack buffer (`WriteFile` / `pwrite`) without allocating. A launch that
//! finds `Running` knows only that the previous process vanished from outside and never guesses.
//!
//! Explicit lifecycle machine matching the process-termination split in Official's
//! `packages/desktop/src/main/windows/recovery-machine.ts` (`RecoveryIncident` `terminated`).
#![cfg_attr(not(target_arch = "wasm32"), allow(unsafe_code))]
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

const MAGIC: u32 = 0x4558_5431; // "EXT1"
const VERSION: u16 = 1;
pub const RECORD_BYTES: usize = 64;

const TAG_CLEAN: u8 = 0;
const TAG_RUNNING: u8 = 1;
const TAG_OUT_OF_MEMORY: u8 = 2;
const TAG_STACK_OVERFLOW: u8 = 3;
const TAG_FATAL: u8 = 4;

pub const FLAG_HEAP_ALLOC: u8 = 1;
pub const FLAG_OS_EXCEPTION: u8 = 2;

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
pub fn mark_clean() {}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use core::alloc::{GlobalAlloc, Layout};
    use std::io::Read as _;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};

    use super::{
        Effect, Event, FLAG_HEAP_ALLOC, Previous, RECORD_BYTES, State, TAG_CLEAN,
        TAG_OUT_OF_MEMORY, TAG_RUNNING, decode_record, encode_record, step,
    };

    static STATE: Mutex<State> = Mutex::new(State::Unopened);
    /// Pre-opened raw descriptor (`HANDLE` on Windows, `RawFd` on Unix), `-1` until opened.
    static RECORD_FD: AtomicIsize = AtomicIsize::new(-1);
    static SEQ: AtomicU64 = AtomicU64::new(0);
    static FAULT_RECORDED: AtomicBool = AtomicBool::new(false);

    /// Wraps the global allocator (`Counting<OomGuard<MiMalloc>>`): forwards straight to `A` and
    /// records `OutOfMemory` from a stack buffer if `A` ever returns null, before Rust's
    /// `handle_alloc_error` aborts (`__fastfail` / `SIGABRT`).
    pub struct OomGuard<A>(pub A);

    // SAFETY: delegates every operation unchanged to `A`; `on_alloc_oom` never touches the heap.
    unsafe impl<A: GlobalAlloc> GlobalAlloc for OomGuard<A> {
        #[inline(always)]
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { self.0.alloc(layout) };
            if p.is_null() {
                on_alloc_oom(layout.size());
            }
            p
        }

        #[inline(always)]
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { self.0.alloc_zeroed(layout) };
            if p.is_null() {
                on_alloc_oom(layout.size());
            }
            p
        }

        #[inline(always)]
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { self.0.dealloc(ptr, layout) };
        }

        #[inline(always)]
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let p = unsafe { self.0.realloc(ptr, layout, new_size) };
            if p.is_null() {
                on_alloc_oom(new_size);
            }
            p
        }
    }

    #[cold]
    #[inline(never)]
    fn on_alloc_oom(requested: usize) {
        #[cfg(windows)]
        const CODE: u32 = 0xC000_0409; // STATUS_STACK_BUFFER_OVERRUN (`__fastfail` from `abort`)
        #[cfg(unix)]
        const CODE: u32 = libc::SIGABRT as u32;
        let (_, avail) = commit_short_and_avail();
        record_fault(TAG_OUT_OF_MEMORY, FLAG_HEAP_ALLOC, CODE, requested as u64, avail);
    }

    pub(super) fn record_fault(tag: u8, flags: u8, code: u32, requested: u64, avail: u64) {
        if FAULT_RECORDED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_err() {
            return;
        }
        let fd = RECORD_FD.load(Ordering::Acquire);
        if fd == -1 || fd == 0 {
            return;
        }
        let seq = SEQ.load(Ordering::Relaxed);
        let buf = encode_record(tag, flags, code, raw_pid(), requested, avail, seq);
        write_raw(fd, &buf);
    }

    pub fn record_path() -> Option<PathBuf> {
        Some(crate::dirs::data()?.join("last-exit.bin"))
    }

    pub fn presented() {
        static STARTED: std::sync::Once = std::sync::Once::new();
        if crate::headless() {
            return;
        }
        STARTED.call_once(|| {
            reserve_main_stack_guarantee();
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
            RECORD_FD.store(fd, Ordering::Release);
            install_os_hooks();
            let buf = encode_record(TAG_RUNNING, 0, 0, raw_pid(), 0, u64::MAX, seq);
            write_raw(fd, &buf);
            #[cfg(debug_assertions)]
            maybe_start_drill();
        }
    }

    /// Writes `Clean` (`tag = 0`) ahead of `storage::close()`, unless a fatal hook already ran.
    pub fn mark_clean() {
        if FAULT_RECORDED.load(Ordering::Acquire) {
            return;
        }
        let effect = {
            let mut guard = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            step(&mut guard, Event::CleanQuit)
        };
        if let Some(Effect::WriteClean { seq }) = effect {
            let fd = RECORD_FD.load(Ordering::Acquire);
            if fd != -1 && !FAULT_RECORDED.load(Ordering::Acquire) {
                let buf = encode_record(TAG_CLEAN, 0, 0, raw_pid(), 0, u64::MAX, seq);
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
    fn raw_pid() -> u32 {
        // SAFETY: reads the TEB/process ID with no allocation or lock.
        unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() }
    }

    #[cfg(unix)]
    fn raw_pid() -> u32 {
        // SAFETY: async-signal-safe syscall wrapper.
        unsafe { libc::getpid() as u32 }
    }

    #[cfg(windows)]
    fn reserve_main_stack_guarantee() {
        let mut bytes = 16 * 1024u32;
        // SAFETY: pre-commits 16 KiB of guard stack on the UI thread for `unhandled_filter`.
        unsafe {
            let _ = windows_sys::Win32::System::Threading::SetThreadStackGuarantee(&raw mut bytes);
        }
    }

    #[cfg(unix)]
    fn reserve_main_stack_guarantee() {}

    #[cfg(windows)]
    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
    }

    #[cfg(windows)]
    #[repr(C)]
    struct ExceptionPointers {
        record: *const ExceptionRecord,
        context: *const core::ffi::c_void,
    }

    #[cfg(windows)]
    type TopLevelFilter = unsafe extern "system" fn(*const ExceptionPointers) -> i32;

    #[cfg(windows)]
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetUnhandledExceptionFilter(filter: Option<TopLevelFilter>) -> Option<TopLevelFilter>;
    }

    #[cfg(windows)]
    unsafe extern "C" {
        fn signal(sig: i32, func: usize) -> usize;
    }

    #[cfg(windows)]
    static PREV_FILTER: std::sync::atomic::AtomicPtr<core::ffi::c_void> =
        std::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

    /// Uses `SetUnhandledExceptionFilter` (never a vectored handler, so zero cost on handled
    /// first-chance exceptions) plus UCRT `signal(SIGABRT)` (invoked by C/C++ `abort()` before
    /// `__fastfail` when a native library or GPU driver runs out of commit).
    #[cfg(windows)]
    fn install_os_hooks() {
        const SIGABRT: i32 = 22;
        // SAFETY: registers top-level unhandled SEH filter and C runtime abort signal handler.
        unsafe {
            let prev = SetUnhandledExceptionFilter(Some(unhandled_filter));
            if let Some(f) = prev {
                PREV_FILTER.store(f as *mut core::ffi::c_void, Ordering::Release);
            }
            let _ = signal(SIGABRT, on_sigabrt as *const () as usize);
        }
    }

    #[cfg(windows)]
    extern "C" fn on_sigabrt(_sig: i32) {
        use super::FLAG_OS_EXCEPTION;
        let (short, avail) = commit_short_and_avail();
        if short {
            record_fault(TAG_OUT_OF_MEMORY, FLAG_OS_EXCEPTION, 0xC000_0409, 0, avail);
        }
    }

    #[cfg(unix)]
    fn install_os_hooks() {}

    #[cfg(windows)]
    unsafe extern "system" fn unhandled_filter(info: *const ExceptionPointers) -> i32 {
        use super::FLAG_OS_EXCEPTION;
        const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
        if !info.is_null() {
            // SAFETY: Windows passes a valid `EXCEPTION_POINTERS` with a non-null `ExceptionRecord`.
            let rec = unsafe { (*info).record };
            if !rec.is_null() {
                let code = unsafe { (*rec).code };
                let (short, avail) = commit_short_and_avail();
                let is_oom = matches!(code, 0xC000_0017 | 0xC000_012D | 0xE000_0008)
                    || (code == 0xC000_00FD && short);
                if is_oom {
                    record_fault(TAG_OUT_OF_MEMORY, FLAG_OS_EXCEPTION, code, 0, avail);
                }
            }
        }
        let prev = PREV_FILTER.load(Ordering::Acquire);
        if !prev.is_null() {
            // SAFETY: `prev` is the function pointer previously returned by `SetUnhandledExceptionFilter`.
            let f: TopLevelFilter = unsafe { core::mem::transmute(prev) };
            return unsafe { f(info) };
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    /// Whether commit memory is short (`< 512 MiB` system-wide or `< 64 MiB` left in this
    /// process's job commit limit, matching `crates/browser-pane/src/transport.rs`), plus the
    /// tightest observed free commit in bytes (`u64::MAX` when unavailable). Stack-only, 0 allocs.
    #[cfg(windows)]
    fn commit_short_and_avail() -> (bool, u64) {
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_JOB_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, QueryInformationJobObject,
        };
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut avail = u64::MAX;
        let mut short = false;
        // SAFETY: stack structs initialized with their exact byte sizes.
        unsafe {
            let mut status: MEMORYSTATUSEX = core::mem::zeroed();
            status.dwLength = core::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&raw mut status) != 0 {
                avail = status.ullAvailPageFile;
                if status.ullAvailPageFile < (512 << 20) {
                    short = true;
                }
            }
            let mut job: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = core::mem::zeroed();
            let size = core::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            if QueryInformationJobObject(
                core::ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                (&raw mut job).cast(),
                size,
                core::ptr::null_mut(),
            ) != 0
                && (job.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY) != 0
            {
                let job_left = (job.JobMemoryLimit as u64).saturating_sub(job.PeakJobMemoryUsed as u64);
                avail = avail.min(job_left);
                if job.PeakJobMemoryUsed.saturating_add(64 << 20) >= job.JobMemoryLimit {
                    short = true;
                }
            }
        }
        (short, avail)
    }

    #[cfg(unix)]
    fn commit_short_and_avail() -> (bool, u64) {
        (false, u64::MAX)
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

    /// Proof-only crash drill inside a `CommitJob` on a scratch root: triple-gated by
    /// `#[cfg(debug_assertions)]`, `build_channel() == "local"`, and `dirs::rooted()`. Waits for
    /// `<RUNE_DIRS_ROOT>/drill.go` so the harness clamps only its own job object after Frame 1.
    #[cfg(debug_assertions)]
    fn maybe_start_drill() {
        if crate::updater::build_channel() != "local" || !crate::dirs::rooted() {
            return;
        }
        #[allow(clippy::disallowed_methods)]
        let Some(kind) = std::env::var("OPENCODE_RUNE_CRASH_DRILL").ok() else { return };
        #[allow(clippy::disallowed_methods)]
        let Some(root) = std::env::var_os("RUNE_DIRS_ROOT").map(PathBuf::from) else { return };
        let stack_bytes = if kind == "stack-commit" { 8 << 20 } else { 1 << 20 };
        let go = root.join("drill.go");
        #[cfg(windows)]
        let go_probe = {
            use std::os::windows::ffi::OsStrExt as _;
            go.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>()
        };
        let _ = std::thread::Builder::new().name("crash-drill".into()).stack_size(stack_bytes).spawn(move || {
            loop {
                #[cfg(windows)]
                // SAFETY: `go_probe` is null-terminated UTF-16; `GetFileAttributesW` does not allocate on the Rust heap.
                let ready = unsafe {
                    windows_sys::Win32::Storage::FileSystem::GetFileAttributesW(go_probe.as_ptr()) != u32::MAX
                };
                #[cfg(not(windows))]
                let ready = go.exists();
                if ready {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            match kind.as_str() {
                "heap" => {
                    let mut v = Vec::<u8>::new();
                    v.reserve_exact(16 << 20);
                    std::hint::black_box(&mut v);
                }
                "stack-commit" | "stack-overflow" => {
                    #[inline(never)]
                    fn grow(depth: usize) -> u8 {
                        let mut page = [depth as u8; 4096];
                        let inner = if depth == 0 { 0 } else { grow(depth - 1) };
                        page[depth & 4095] = page[depth & 4095].wrapping_add(inner);
                        std::hint::black_box(&mut page)[0]
                    }
                    std::hint::black_box(grow(4096));
                }
                _ => {}
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use imp::{OomGuard, mark_clean, presented};
