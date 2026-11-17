//! OS-specific fatal exception hooks and allocation-free raw file I/O for `last_exit`.
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::Ordering;

use super::imp::PATHS;
#[cfg(windows)]
use super::imp::{TERMINAL_RECORDED, TENTATIVE_PANIC, record_fault};
use super::{RECORD_BYTES, SKIP_BYTES, SKIP_MAGIC};
#[cfg(windows)]
use super::{FLAG_OS_EXCEPTION, TAG_FATAL, TAG_OUT_OF_MEMORY, TAG_STACK_OVERFLOW};

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
    unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() }
}

#[cfg(unix)]
pub(super) fn raw_pid() -> u32 {
    unsafe { libc::getpid() as u32 }
}

#[cfg(windows)]
pub(super) fn reserve_main_stack_guarantee() {
    let mut bytes = 16 * 1024u32;
    unsafe {
        let _ = windows_sys::Win32::System::Threading::SetThreadStackGuarantee(&raw mut bytes);
    }
}

#[cfg(unix)]
pub(super) fn reserve_main_stack_guarantee() {}

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
    unsafe {
        let prev = SetUnhandledExceptionFilter(Some(unhandled_filter));
        if let Some(f) = prev {
            PREV_FILTER.store(f as *mut core::ffi::c_void, Ordering::Release);
        }
        let _ = signal(SIGABRT, on_sigabrt as *const () as usize);
    }
}

#[cfg(unix)]
pub(super) fn install_os_hooks() {}

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
        let rec = unsafe { (*info).record };
        if !rec.is_null() {
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
        let f: TopLevelFilter = unsafe { core::mem::transmute(prev) };
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
    let fd = unsafe { libc::open(paths.record_c.as_ptr(), libc::O_RDWR | libc::O_CREAT, 0o600) };
    fd as isize
}

#[cfg(windows)]
pub(super) fn read_raw(fd: isize, buf: &mut [u8; RECORD_BYTES]) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{FILE_BEGIN, ReadFile, SetFilePointerEx};
    let handle = fd as windows_sys::Win32::Foundation::HANDLE;
    let mut got = 0u32;
    unsafe {
        SetFilePointerEx(handle, 0, core::ptr::null_mut(), FILE_BEGIN) != 0
            && ReadFile(handle, buf.as_mut_ptr(), RECORD_BYTES as u32, &raw mut got, core::ptr::null_mut()) != 0
            && got as usize == RECORD_BYTES
    }
}

#[cfg(unix)]
pub(super) fn read_raw(fd: isize, buf: &mut [u8; RECORD_BYTES]) -> bool {
    let got = unsafe { libc::pread(fd as libc::c_int, buf.as_mut_ptr().cast(), RECORD_BYTES, 0) };
    got == RECORD_BYTES as isize
}

#[cfg(windows)]
pub(super) fn close_raw(fd: isize) {
    unsafe {
        let _ = windows_sys::Win32::Foundation::CloseHandle(fd as windows_sys::Win32::Foundation::HANDLE);
    }
}

#[cfg(unix)]
pub(super) fn close_raw(fd: isize) {
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
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::RaiseException(
            0xC000_0005,
            1,
            0,
            core::ptr::null(),
        );
    }
}
