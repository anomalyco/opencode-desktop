//! OS-specific fatal exception hooks and allocation-free raw file I/O for `last_exit`.
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use super::imp::{PATHS, TERMINAL_RECORDED, TENTATIVE_PANIC, record_fault};
use super::{
    FLAG_OS_EXCEPTION, RECORD_BYTES, SKIP_BYTES, SKIP_MAGIC, TAG_FATAL, TAG_OUT_OF_MEMORY,
    TAG_STACK_OVERFLOW,
};

pub(super) struct EncodedPaths {
    pub(super) record: PathBuf,
    pub(super) skip: PathBuf,
    #[cfg(windows)]
    record_wide: Vec<u16>,
    #[cfg(windows)]
    skip_wide: Vec<u16>,
    #[cfg(unix)]
    record_c: std::ffi::CString,
    #[cfg(unix)]
    skip_c: std::ffi::CString,
}

impl EncodedPaths {
    pub(super) fn new(record: PathBuf, skip: PathBuf) -> Self {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            Self {
                record_wide: record.as_os_str().encode_wide().chain([0]).collect(),
                skip_wide: skip.as_os_str().encode_wide().chain([0]).collect(),
                record,
                skip,
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            let c = |p: &Path| {
                std::ffi::CString::new(p.as_os_str().as_bytes())
                    .unwrap_or_else(|_| std::ffi::CString::new("").unwrap())
            };
            Self { record_c: c(&record), skip_c: c(&skip), record, skip }
        }
    }
}

#[cfg(windows)]
pub(super) fn raw_pid() -> u32 {
    // SAFETY: `GetCurrentProcessId` takes no parameters and has no preconditions.
    unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() }
}

#[cfg(unix)]
pub(super) fn raw_pid() -> u32 {
    // SAFETY: `getpid` takes no parameters and is always safe to call.
    unsafe { libc::getpid() as u32 }
}

#[cfg(windows)]
pub(super) fn reserve_main_stack_guarantee() {
    let mut bytes = 16 * 1024u32;
    // SAFETY: `&raw mut bytes` points to a valid, aligned `u32` owned by this stack frame.
    unsafe {
        let _ = windows_sys::Win32::System::Threading::SetThreadStackGuarantee(&raw mut bytes);
    }
}

#[cfg(unix)]
static mut ALT_STACK: [u8; 65_536] = [0; 65_536];

#[cfg(all(unix, not(target_os = "macos")))]
static MAIN_STACK_BOUNDS: [std::sync::atomic::AtomicUsize; 2] = [
    std::sync::atomic::AtomicUsize::new(0),
    std::sync::atomic::AtomicUsize::new(0),
];

#[cfg(unix)]
pub(super) fn reserve_main_stack_guarantee() {
    // SAFETY: called once on the main thread from `init_hooks`; `cur` and `attr` are
    // zero-initialisable C POD structs, `ALT_STACK` is `'static` storage never touched by Rust
    // code (used only by the kernel as this thread's alternate signal stack), and `attr` is
    // destroyed whenever `pthread_getattr_np` succeeds.
    unsafe {
        let mut cur: libc::stack_t = core::mem::zeroed();
        if libc::sigaltstack(core::ptr::null(), &raw mut cur) != 0
            || (cur.ss_flags & libc::SS_DISABLE) != 0
            || cur.ss_size < 32_768
        {
            let ss = libc::stack_t {
                ss_sp: (&raw mut ALT_STACK).cast(),
                ss_flags: 0,
                ss_size: 65_536,
            };
            let _ = libc::sigaltstack(&raw const ss, core::ptr::null_mut());
        }
        #[cfg(target_os = "linux")]
        {
            let mut attr: libc::pthread_attr_t = core::mem::zeroed();
            if libc::pthread_getattr_np(libc::pthread_self(), &raw mut attr) == 0 {
                let mut addr: *mut core::ffi::c_void = core::ptr::null_mut();
                let mut size: usize = 0;
                if libc::pthread_attr_getstack(&raw const attr, &raw mut addr, &raw mut size) == 0
                    && !addr.is_null()
                    && size > 0
                {
                    MAIN_STACK_BOUNDS[0].store(addr as usize, Ordering::Release);
                    MAIN_STACK_BOUNDS[1].store((addr as usize).saturating_add(size), Ordering::Release);
                }
                let _ = libc::pthread_attr_destroy(&raw mut attr);
            }
        }
    }
}

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

#[cfg(windows)]
pub(super) fn install_os_hooks() {
    const SIGABRT: i32 = 22;
    // SAFETY: `unhandled_filter` and `on_sigabrt` are static functions with matching Win32/C ABIs installed once at startup.
    unsafe {
        let prev = SetUnhandledExceptionFilter(Some(unhandled_filter));
        if let Some(f) = prev {
            PREV_FILTER.store(f as *mut core::ffi::c_void, Ordering::Release);
        }
        let _ = signal(SIGABRT, on_sigabrt as *const () as usize);
    }
}

#[cfg(unix)]
const FATAL_SIGS: [libc::c_int; 6] = [
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGABRT,
    libc::SIGILL,
    libc::SIGFPE,
    libc::SIGTRAP,
];

#[cfg(unix)]
pub(super) fn install_os_hooks() {
    // SAFETY: `act` is a zero-initialisable `sigaction` struct initialized with `on_fatal_signal`
    // (whose `extern "C"` signature matches `SA_SIGINFO` and which uses only lock-free atomics,
    // raw POSIX syscalls, and `_exit`); `sigemptyset` and `sigaction` receive valid stack pointers.
    unsafe {
        for &sig in &FATAL_SIGS {
            let mut act: libc::sigaction = core::mem::zeroed();
            act.sa_sigaction = on_fatal_signal as *const () as libc::sighandler_t;
            libc::sigemptyset(&raw mut act.sa_mask);
            act.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            let _ = libc::sigaction(sig, &raw const act, core::ptr::null_mut());
        }
    }
}

#[cfg(target_os = "macos")]
fn is_stack_overflow_addr(addr: usize) -> bool {
    if addr < 4096 {
        return false;
    }
    // SAFETY: `pthread_self` returns the calling thread's live `pthread_t`; on Darwin
    // `pthread_get_stackaddr_np` and `pthread_get_stacksize_np` read fields of that thread's
    // descriptor with no locks or allocation.
    unsafe {
        let th = libc::pthread_self();
        let high = libc::pthread_get_stackaddr_np(th) as usize;
        let size = libc::pthread_get_stacksize_np(th);
        if high == 0 || size == 0 {
            return false;
        }
        let low = high.saturating_sub(size);
        addr >= low.saturating_sub(65_536) && addr <= low.saturating_add(32_768)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_stack_overflow_addr(addr: usize) -> bool {
    if addr < 4096 {
        return false;
    }
    let low = MAIN_STACK_BOUNDS[0].load(Ordering::Acquire);
    let high = MAIN_STACK_BOUNDS[1].load(Ordering::Acquire);
    low > 0 && high > low && addr >= low.saturating_sub(65_536) && addr <= low.saturating_add(32_768)
}

#[cfg(unix)]
fn siginfo_fault_addr(info: *mut libc::siginfo_t) -> usize {
    if info.is_null() {
        return 0;
    }
    #[cfg(target_os = "linux")]
    // SAFETY: private helper called only from `on_fatal_signal` (`SA_SIGINFO`) with the kernel's
    // `siginfo_t` pointer, checked non-null above; `si_addr` reads a POD pointer field.
    unsafe {
        (*info).si_addr() as usize
    }
    #[cfg(not(target_os = "linux"))]
    // SAFETY: private helper called only from `on_fatal_signal` (`SA_SIGINFO`) with the kernel's
    // `siginfo_t` pointer, checked non-null above.
    unsafe {
        (*info).si_addr as usize
    }
}

#[cfg(unix)]
extern "C" fn on_fatal_signal(
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    _ucontext: *mut core::ffi::c_void,
) {
    if sig == libc::SIGABRT && TENTATIVE_PANIC.load(Ordering::Acquire) {
        TERMINAL_RECORDED.store(true, Ordering::Release);
    } else {
        let addr = siginfo_fault_addr(info);
        let (short, avail) = commit_short_and_avail();
        let tag = if (sig == libc::SIGSEGV || sig == libc::SIGBUS) && is_stack_overflow_addr(addr) {
            if short { TAG_OUT_OF_MEMORY } else { TAG_STACK_OVERFLOW }
        } else if sig == libc::SIGABRT && short {
            TAG_OUT_OF_MEMORY
        } else {
            TAG_FATAL
        };
        record_fault(tag, FLAG_OS_EXCEPTION, sig as u32, 0, avail, true);
    }
    // End at once via `_exit` rather than re-raising into CrashReporter or unwinding on an exhausted stack.
    // SAFETY: `_exit` is async-signal-safe and immediately terminates the process with no memory preconditions.
    unsafe {
        libc::_exit(128 + sig);
    }
}

#[cfg(windows)]
extern "C" fn on_sigabrt(_sig: i32) {
    if TENTATIVE_PANIC.load(Ordering::Acquire) {
        TERMINAL_RECORDED.store(true, Ordering::Release);
        return;
    }
    let (short, avail) = commit_short_and_avail();
    let tag = if short { TAG_OUT_OF_MEMORY } else { TAG_FATAL };
    record_fault(tag, FLAG_OS_EXCEPTION, 0xC000_0409, 0, avail, true);
}

#[cfg(windows)]
unsafe extern "system" fn unhandled_filter(info: *const ExceptionPointers) -> i32 {
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const EXCEPTION_EXECUTE_HANDLER: i32 = 1;
    if !info.is_null() {
        // SAFETY: Windows SEH passes a valid `EXCEPTION_POINTERS` (checked non-null above) whose first field is `ExceptionRecord`.
        let rec = unsafe { (*info).record };
        if !rec.is_null() {
            // SAFETY: `rec` is a non-null `EXCEPTION_RECORD` whose first field is `DWORD ExceptionCode`.
            let code = unsafe { (*rec).code };
            let (short, avail) = commit_short_and_avail();
            let tag = match code {
                0xC000_0017 | 0xC000_012D | 0xE000_0008 => TAG_OUT_OF_MEMORY,
                0xC000_00FD if short => TAG_OUT_OF_MEMORY,
                0xC000_00FD => TAG_STACK_OVERFLOW,
                _ => TAG_FATAL,
            };
            record_fault(tag, FLAG_OS_EXCEPTION, code, 0, avail, true);
            let prev = PREV_FILTER.load(Ordering::Acquire);
            if prev.is_null() {
                // End at once with `code` rather than unwinding/detaching DLLs or spawning WerFault on an exhausted stack.
                // SAFETY: `GetCurrentProcess()` returns the valid current-process pseudo-handle; terminating with `code` has no memory preconditions.
                unsafe {
                    let _ = windows_sys::Win32::System::Threading::TerminateProcess(
                        windows_sys::Win32::System::Threading::GetCurrentProcess(),
                        code,
                    );
                }
                return EXCEPTION_EXECUTE_HANDLER;
            }
        }
    }
    let prev = PREV_FILTER.load(Ordering::Acquire);
    if !prev.is_null() {
        // SAFETY: `PREV_FILTER` only stores a non-null `TopLevelFilter` previously returned by `SetUnhandledExceptionFilter`.
        let f: TopLevelFilter = unsafe { core::mem::transmute(prev) };
        // SAFETY: `f` is the prior top-level exception filter and `info` is the SEH pointer passed to this handler.
        return unsafe { f(info) };
    }
    EXCEPTION_CONTINUE_SEARCH
}

#[cfg(windows)]
pub(super) fn commit_short_and_avail() -> (bool, u64) {
    use windows_sys::Win32::System::JobObjects::{
        JOB_OBJECT_LIMIT_JOB_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, QueryInformationJobObject,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut avail = u64::MAX;
    let mut short = false;
    // SAFETY: `MEMORYSTATUSEX` and `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` are zero-initialisable Win32 POD structs passed with their exact sizes; a null job handle queries the current process's job.
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
pub(super) fn commit_short_and_avail() -> (bool, u64) {
    (false, u64::MAX)
}

#[cfg(windows)]
pub(super) fn open_file(path: &Path) -> std::io::Result<std::fs::File> {
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
pub(super) fn open_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().read(true).write(true).create(true).open(path)
}

#[cfg(windows)]
pub(super) fn into_raw_fd(file: std::fs::File) -> isize {
    use std::os::windows::io::IntoRawHandle as _;
    file.into_raw_handle() as isize
}

#[cfg(unix)]
pub(super) fn into_raw_fd(file: std::fs::File) -> isize {
    use std::os::fd::IntoRawFd as _;
    file.into_raw_fd() as isize
}

#[cfg(windows)]
pub(super) fn open_raw_emergency() -> isize {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_ALWAYS,
    };
    let Some(paths) = PATHS.get() else { return -1 };
    // SAFETY: `paths.record_wide` is a NUL-terminated UTF-16 path stored in the `'static` `PATHS` lock.
    let h = unsafe {
        CreateFileW(
            paths.record_wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            core::ptr::null(),
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            core::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() { -1 } else { h as isize }
}

#[cfg(unix)]
pub(super) fn open_raw_emergency() -> isize {
    let Some(paths) = PATHS.get() else { return -1 };
    // SAFETY: `paths.record_c` is a `'static` NUL-terminated `CString` in `PATHS`; `O_RDWR | O_CREAT`
    // passes a valid `c_int` mode (`0o600`), and `open` is async-signal-safe.
    let fd = unsafe { libc::open(paths.record_c.as_ptr(), libc::O_RDWR | libc::O_CREAT, 0o600) };
    fd as isize
}

#[cfg(windows)]
pub(super) fn read_raw(fd: isize, buf: &mut [u8; RECORD_BYTES]) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};
    let handle = fd as windows_sys::Win32::Foundation::HANDLE;
    let mut got = 0u32;
    // SAFETY: `handle` is an open file handle from `open_raw_emergency`, `buf` is writable for `RECORD_BYTES`, and `got` is a valid aligned `u32`.
    unsafe {
        SetFilePointerEx(handle, 0, core::ptr::null_mut(), FILE_BEGIN) != 0
            && ReadFile(handle, buf.as_mut_ptr(), RECORD_BYTES as u32, &raw mut got, core::ptr::null_mut()) != 0
            && got as usize == RECORD_BYTES
    }
}

#[cfg(unix)]
pub(super) fn read_raw(fd: isize, buf: &mut [u8; RECORD_BYTES]) -> bool {
    // SAFETY: `fd` is an open file descriptor from `open_raw_emergency`, `buf` is valid writable
    // storage for `RECORD_BYTES` bytes, and `pread` is async-signal-safe.
    let got = unsafe { libc::pread(fd as libc::c_int, buf.as_mut_ptr().cast(), RECORD_BYTES, 0) };
    got == RECORD_BYTES as isize
}

#[cfg(windows)]
pub(super) fn close_raw(fd: isize) {
    // SAFETY: called only on a temporary handle returned by `open_raw_emergency`, closing it once.
    unsafe {
        let _ = windows_sys::Win32::Foundation::CloseHandle(fd as windows_sys::Win32::Foundation::HANDLE);
    }
}

#[cfg(unix)]
pub(super) fn close_raw(fd: isize) {
    // SAFETY: called once on a temporary file descriptor returned by `open_raw_emergency`; `close`
    // is async-signal-safe.
    unsafe {
        let _ = libc::close(fd as libc::c_int);
    }
}

#[cfg(windows)]
pub(super) fn write_skip_sentinel(tag: u8, session_hash: u64) {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, WriteFile,
    };
    let Some(paths) = PATHS.get() else { return };
    let mut buf = [0u8; SKIP_BYTES];
    buf[0..4].copy_from_slice(&SKIP_MAGIC.to_le_bytes());
    buf[4] = tag;
    buf[8..16].copy_from_slice(&session_hash.to_le_bytes());
    let mut written = 0u32;
    // SAFETY: `paths.skip_wide` is a `'static` NUL-terminated UTF-16 path; `buf` holds `SKIP_BYTES` initialized bytes; `h` is validated before writing and closed once.
    unsafe {
        let h = CreateFileW(
            paths.skip_wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            core::ptr::null(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            core::ptr::null_mut(),
        );
        if h != INVALID_HANDLE_VALUE && !h.is_null() {
            let _ = WriteFile(h, buf.as_ptr(), SKIP_BYTES as u32, &raw mut written, core::ptr::null_mut());
            let _ = CloseHandle(h);
        }
    }
}

#[cfg(unix)]
pub(super) fn write_skip_sentinel(tag: u8, session_hash: u64) {
    let Some(paths) = PATHS.get() else { return };
    let mut buf = [0u8; SKIP_BYTES];
    buf[0..4].copy_from_slice(&SKIP_MAGIC.to_le_bytes());
    buf[4] = tag;
    buf[8..16].copy_from_slice(&session_hash.to_le_bytes());
    // SAFETY: `paths.skip_c` is a `'static` NUL-terminated `CString` in `PATHS`, `buf` holds
    // `SKIP_BYTES` initialized bytes, `fd` is checked `>= 0` before writing and closed once, and
    // `open`/`write`/`close` are async-signal-safe.
    unsafe {
        let fd = libc::open(paths.skip_c.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o600);
        if fd >= 0 {
            let _ = libc::write(fd, buf.as_ptr().cast(), SKIP_BYTES);
            let _ = libc::close(fd);
        }
    }
}

#[cfg(windows)]
pub(super) fn write_raw(fd: isize, buf: &[u8; RECORD_BYTES]) {
    use windows_sys::Win32::Storage::FileSystem::{FILE_BEGIN, SetFilePointerEx, WriteFile};
    if fd == -1 || fd == 0 {
        return;
    }
    let handle = fd as windows_sys::Win32::Foundation::HANDLE;
    let mut written = 0u32;
    // SAFETY: `handle` is an open file handle owned by `RECORD_FD` or the current emergency call; `buf` holds `RECORD_BYTES` initialized bytes and `written` is valid.
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
    // SAFETY: `fd` is checked `>= 0` above and is an open descriptor owned by `RECORD_FD` or the
    // current emergency call; `buf` holds `RECORD_BYTES` initialized bytes and `pwrite` is
    // async-signal-safe.
    unsafe {
        let _ = libc::pwrite(fd as libc::c_int, buf.as_ptr().cast(), RECORD_BYTES, 0);
    }
}

#[cfg(debug_assertions)]
pub(super) fn wait_drill_go(go: &Path) {
    #[cfg(windows)]
    let probe = {
        use std::os::windows::ffi::OsStrExt as _;
        go.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>()
    };
    loop {
        // SAFETY: `probe` is a valid NUL-terminated UTF-16 path alive for this entire loop.
        #[cfg(windows)]
        let ready = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFileAttributesW(probe.as_ptr()) != u32::MAX
        };
        #[cfg(not(windows))]
        let ready = go.exists();
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

#[cfg(debug_assertions)]
pub(super) fn raise_access_violation() {
    // SAFETY: debug-only crash drill invokes Win32 `RaiseException` with 0 arguments, null array, and `EXCEPTION_NONCONTINUABLE` to exercise `unhandled_filter` and terminate the process.
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::RaiseException(
            0xC000_0005,
            1,
            0,
            core::ptr::null(),
        );
    }
    // SAFETY: this debug-only drill faults on purpose. It maps a fresh `PROT_NONE` region that
    // nothing else refers to and reads from the middle of it, so the kernel delivers `SIGSEGV`
    // with a real fault address for `on_fatal_signal` to record. A raised signal would carry none.
    #[cfg(unix)]
    unsafe {
        #[cfg(target_os = "macos")]
        const ANON: libc::c_int = libc::MAP_ANON;
        #[cfg(not(target_os = "macos"))]
        const ANON: libc::c_int = libc::MAP_ANONYMOUS;
        let page = libc::mmap(
            core::ptr::null_mut(),
            262_144,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | ANON,
            -1,
            0,
        );
        if page != libc::MAP_FAILED {
            let _ = core::ptr::read_volatile(page.cast::<u8>().add(131_072));
        } else {
            libc::abort();
        }
    }
}
