//! `--launch-trace-io`: every file open, attribute read, directory scan, delete and rename, every
//! process start, socket connect and name lookup the app's own code makes, and every DLL the
//! process loads, with the thread, the time (ns since the process was created), how long the call
//! took, and the first frames of the app's or Rune's code that made it (file:line, from the
//! exe's line tables). The list goes into the `--launch-trace` line as `io`, for the boot and quit
//! inventories (`harness\perf\launch-bench.ts inventory`).
//!
//! It records by pointing this exe's import table at wrappers, so it sees what the app and every
//! crate linked into it call, not what system DLLs do on their own (DirectWrite, Direct3D and the
//! GPU driver open their files through their own imports; their DLL loads still show). Recording
//! takes a backtrace per call, so a run with it is for the inventory, never for timing.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use rune::time::Instant;

/// One call the app made.
pub struct Call {
    pub at: Instant,
    pub took_ns: u64,
    pub tid: u32,
    pub op: &'static str,
    pub path: String,
    pub ok: bool,
    trace: Option<std::backtrace::Backtrace>,
}

static CALLS: Mutex<Vec<Call>> = Mutex::new(Vec::new());

thread_local! {
    /// Inside a wrapper: calls the recorder itself makes go straight through.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// Starts recording when `--launch-trace-io` is given. First thing in `main`.
pub fn start() {
    if !crate::flag("--launch-trace-io") {
        return;
    }
    // SAFETY: patches this module's own import table before any other thread runs.
    unsafe {
        hook_imports();
        watch_dll_loads();
    }
}

/// Every call so far, oldest first, each with the app frames that made it (resolved now).
pub fn take() -> Vec<(Call, Vec<String>)> {
    let calls = std::mem::take(&mut *CALLS.lock().unwrap_or_else(|e| e.into_inner()));
    BUSY.with(|b| b.set(true));
    let out = calls.into_iter().map(|mut c| {
        let sites = c.trace.take().map(|t| sites(&t)).unwrap_or_default();
        (c, sites)
    }).collect();
    BUSY.with(|b| b.set(false));
    out
}

/// The innermost frames in the app's or Rune's code (at most four), as `path:line`.
fn sites(trace: &std::backtrace::Backtrace) -> Vec<String> {
    let text = trace.to_string();
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(at) = line.trim().strip_prefix("at ") else { continue };
        let ours = (at.contains("apps\\desktop\\src") || at.contains("\\crates\\") || at.contains("anomalyco\\rune") || at.contains("rune-") && at.contains("\\checkouts\\"))
            && !at.contains("io_trace.rs");
        if ours {
            let short = at.rsplit_once("\\src\\").map_or(at.to_owned(), |(head, tail)| format!("{}/src/{}", head.rsplit('\\').next().unwrap_or(""), tail.replace('\\', "/")));
            out.push(short);
            if out.len() == 4 {
                break;
            }
        }
    }
    out
}

fn record(op: &'static str, path: String, started: Instant, ok: bool) {
    let took_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let trace = Some(std::backtrace::Backtrace::force_capture());
    // SAFETY: no arguments; it reads the calling thread's id.
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    CALLS.lock().unwrap_or_else(|e| e.into_inner()).push(Call { at: started, took_ns, tid, op, path, ok, trace });
}

/// Runs `f` as a recorded call unless this thread is already recording.
fn traced<R>(f: impl FnOnce() -> R, done: impl FnOnce(&R, Instant)) -> R {
    if BUSY.with(Cell::get) {
        return f();
    }
    BUSY.with(|b| b.set(true));
    let started = Instant::now();
    let r = f();
    done(&r, started);
    BUSY.with(|b| b.set(false));
    r
}

/// A NUL-terminated UTF-16 string.
unsafe fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the caller passes a NUL-terminated string, as every wrapped API requires.
    unsafe {
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

/// A UNICODE_STRING (`Length` in bytes, `Buffer` at offset 8).
unsafe fn unicode(s: *const c_void) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: a UNICODE_STRING's layout: u16 Length, u16 MaximumLength, pointer Buffer.
    unsafe {
        let len = *(s as *const u16) as usize / 2;
        let buf = *((s as *const u8).add(8) as *const *const u16);
        if buf.is_null() { String::new() } else { String::from_utf16_lossy(std::slice::from_raw_parts(buf, len)) }
    }
}

const INVALID: isize = -1;

macro_rules! original {
    ($slot:ident, $ty:ty) => {
        // SAFETY: the slot holds the import's address before it was patched, of this type.
        unsafe { std::mem::transmute::<usize, $ty>($slot.load(Ordering::Relaxed)) }
    };
}

static CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, *const c_void, u32, u32, *mut c_void) -> isize;
unsafe extern "system" fn create_file_w(name: *const u16, access: u32, share: u32, sa: *const c_void, disposition: u32, flags: u32, template: *mut c_void) -> isize {
    let real = original!(CREATE_FILE_W, CreateFileW);
    // SAFETY: the arguments go to the real CreateFileW unchanged.
    traced(|| unsafe { real(name, access, share, sa, disposition, flags, template) }, |&h, t| {
        let op = if disposition == 1 || disposition == 2 || disposition == 4 { "file.create" } else { "file.open" };
        // SAFETY: `name` is CreateFileW's NUL-terminated path.
        record(op, unsafe { wide(name) }, t, h != INVALID);
    })
}

static GET_FILE_ATTRIBUTES_EX_W: AtomicUsize = AtomicUsize::new(0);
type GetFileAttributesExW = unsafe extern "system" fn(*const u16, i32, *mut c_void) -> i32;
unsafe extern "system" fn get_file_attributes_ex_w(name: *const u16, level: i32, info: *mut c_void) -> i32 {
    let real = original!(GET_FILE_ATTRIBUTES_EX_W, GetFileAttributesExW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, level, info) }, |&ok, t| record("file.stat", unsafe { wide(name) }, t, ok != 0))
}

static GET_FILE_ATTRIBUTES_W: AtomicUsize = AtomicUsize::new(0);
type GetFileAttributesW = unsafe extern "system" fn(*const u16) -> u32;
unsafe extern "system" fn get_file_attributes_w(name: *const u16) -> u32 {
    let real = original!(GET_FILE_ATTRIBUTES_W, GetFileAttributesW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name) }, |&a, t| record("file.stat", unsafe { wide(name) }, t, a != u32::MAX))
}

static FIND_FIRST_FILE_EX_W: AtomicUsize = AtomicUsize::new(0);
type FindFirstFileExW = unsafe extern "system" fn(*const u16, i32, *mut c_void, i32, *const c_void, u32) -> isize;
unsafe extern "system" fn find_first_file_ex_w(name: *const u16, level: i32, data: *mut c_void, search: i32, filter: *const c_void, flags: u32) -> isize {
    let real = original!(FIND_FIRST_FILE_EX_W, FindFirstFileExW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, level, data, search, filter, flags) }, |&h, t| record("dir.scan", unsafe { wide(name) }, t, h != INVALID))
}

static FIND_FIRST_FILE_W: AtomicUsize = AtomicUsize::new(0);
type FindFirstFileW = unsafe extern "system" fn(*const u16, *mut c_void) -> isize;
unsafe extern "system" fn find_first_file_w(name: *const u16, data: *mut c_void) -> isize {
    let real = original!(FIND_FIRST_FILE_W, FindFirstFileW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, data) }, |&h, t| record("dir.scan", unsafe { wide(name) }, t, h != INVALID))
}

static DELETE_FILE_W: AtomicUsize = AtomicUsize::new(0);
type DeleteFileW = unsafe extern "system" fn(*const u16) -> i32;
unsafe extern "system" fn delete_file_w(name: *const u16) -> i32 {
    let real = original!(DELETE_FILE_W, DeleteFileW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name) }, |&ok, t| record("file.delete", unsafe { wide(name) }, t, ok != 0))
}

static MOVE_FILE_EX_W: AtomicUsize = AtomicUsize::new(0);
type MoveFileExW = unsafe extern "system" fn(*const u16, *const u16, u32) -> i32;
unsafe extern "system" fn move_file_ex_w(from: *const u16, to: *const u16, flags: u32) -> i32 {
    let real = original!(MOVE_FILE_EX_W, MoveFileExW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(from, to, flags) }, |&ok, t| record("file.rename", unsafe { format!("{} -> {}", wide(from), wide(to)) }, t, ok != 0))
}

static CREATE_DIRECTORY_W: AtomicUsize = AtomicUsize::new(0);
type CreateDirectoryW = unsafe extern "system" fn(*const u16, *const c_void) -> i32;
unsafe extern "system" fn create_directory_w(name: *const u16, sa: *const c_void) -> i32 {
    let real = original!(CREATE_DIRECTORY_W, CreateDirectoryW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, sa) }, |&ok, t| record("dir.create", unsafe { wide(name) }, t, ok != 0))
}

static READ_FILE: AtomicUsize = AtomicUsize::new(0);
type ReadFile = unsafe extern "system" fn(isize, *mut c_void, u32, *mut u32, *mut c_void) -> i32;
unsafe extern "system" fn read_file(h: isize, buf: *mut c_void, len: u32, read: *mut u32, ov: *mut c_void) -> i32 {
    let real = original!(READ_FILE, ReadFile);
    if BUSY.with(Cell::get) {
        // SAFETY: unchanged arguments.
        return unsafe { real(h, buf, len, read, ov) };
    }
    // Reads go into the totals of the call that opened the handle, not a row each.
    // SAFETY: unchanged arguments.
    let ok = unsafe { real(h, buf, len, read, ov) };
    // SAFETY: `read` is ReadFile's out count when given.
    let n = if read.is_null() { 0 } else { unsafe { *read } };
    READ_BYTES.fetch_add(n as usize, Ordering::Relaxed);
    READ_CALLS.fetch_add(1, Ordering::Relaxed);
    ok
}

/// ReadFile totals over the app's own calls.
pub static READ_BYTES: AtomicUsize = AtomicUsize::new(0);
pub static READ_CALLS: AtomicUsize = AtomicUsize::new(0);

static CREATE_PROCESS_W: AtomicUsize = AtomicUsize::new(0);
type CreateProcessW = unsafe extern "system" fn(*const u16, *mut u16, *const c_void, *const c_void, i32, u32, *const c_void, *const u16, *const c_void, *mut c_void) -> i32;
unsafe extern "system" fn create_process_w(app: *const u16, cmd: *mut u16, pa: *const c_void, ta: *const c_void, inherit: i32, flags: u32, env: *const c_void, dir: *const u16, si: *const c_void, pi: *mut c_void) -> i32 {
    let real = original!(CREATE_PROCESS_W, CreateProcessW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) }, |&ok, t| {
        // SAFETY: the command line is NUL-terminated when given.
        let line = unsafe { if cmd.is_null() { wide(app) } else { wide(cmd) } };
        record("process.spawn", line, t, ok != 0)
    })
}

static NT_CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
type NtCreateFile = unsafe extern "system" fn(*mut isize, u32, *const c_void, *mut c_void, *const i64, u32, u32, u32, u32, *const c_void, u32) -> i32;
unsafe extern "system" fn nt_create_file(h: *mut isize, access: u32, oa: *const c_void, io: *mut c_void, size: *const i64, attrs: u32, share: u32, disposition: u32, options: u32, ea: *const c_void, ea_len: u32) -> i32 {
    let real = original!(NT_CREATE_FILE, NtCreateFile);
    // SAFETY: unchanged arguments; OBJECT_ATTRIBUTES holds ObjectName at offset 16.
    traced(|| unsafe { real(h, access, oa, io, size, attrs, share, disposition, options, ea, ea_len) }, |&status, t| {
        let name = if oa.is_null() { String::new() } else { unsafe { unicode(*((oa as *const u8).add(16) as *const *const c_void)) } };
        record("file.open", name, t, status >= 0)
    })
}

static CONNECT: AtomicUsize = AtomicUsize::new(0);
type Connect = unsafe extern "system" fn(usize, *const u8, i32) -> i32;
unsafe extern "system" fn connect(s: usize, addr: *const u8, len: i32) -> i32 {
    let real = original!(CONNECT, Connect);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(s, addr, len) }, |&r, t| {
        // SAFETY: a SOCKADDR of `len` bytes: family, then port and address.
        let text = unsafe { sockaddr(addr, len) };
        // A non-blocking connect returns SOCKET_ERROR (WSAEWOULDBLOCK) while it proceeds.
        record("net.connect", text, t, r == 0 || r == -1)
    })
}

/// `127.0.0.1:4096` or `[::1]:4096`.
unsafe fn sockaddr(addr: *const u8, len: i32) -> String {
    if addr.is_null() || len < 8 {
        return String::new();
    }
    // SAFETY: the caller's SOCKADDR is `len` bytes.
    unsafe {
        let family = *(addr as *const u16);
        let port = u16::from_be_bytes([*addr.add(2), *addr.add(3)]);
        match family {
            2 => format!("{}.{}.{}.{}:{port}", *addr.add(4), *addr.add(5), *addr.add(6), *addr.add(7)),
            23 if len >= 24 => {
                let ip: [u8; 16] = std::ptr::read_unaligned(addr.add(8) as *const [u8; 16]);
                format!("[{}]:{port}", std::net::Ipv6Addr::from(ip))
            }
            _ => format!("family {family}"),
        }
    }
}

static GET_ADDR_INFO_EX_W: AtomicUsize = AtomicUsize::new(0);
type GetAddrInfoExW = unsafe extern "system" fn(*const u16, *const u16, u32, *const c_void, *const c_void, *mut *mut c_void, *const c_void, *const c_void, *const c_void, *mut isize) -> i32;
unsafe extern "system" fn get_addr_info_ex_w(name: *const u16, service: *const u16, ns: u32, id: *const c_void, hints: *const c_void, result: *mut *mut c_void, timeout: *const c_void, ov: *const c_void, done: *const c_void, handle: *mut isize) -> i32 {
    let real = original!(GET_ADDR_INFO_EX_W, GetAddrInfoExW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, service, ns, id, hints, result, timeout, ov, done, handle) }, |&r, t| record("net.resolve", unsafe { wide(name) }, t, r == 0))
}

static GET_ADDR_INFO: AtomicUsize = AtomicUsize::new(0);
type GetAddrInfo = unsafe extern "system" fn(*const u8, *const u8, *const c_void, *mut *mut c_void) -> i32;
unsafe extern "system" fn get_addr_info(name: *const u8, service: *const u8, hints: *const c_void, result: *mut *mut c_void) -> i32 {
    let real = original!(GET_ADDR_INFO, GetAddrInfo);
    // SAFETY: unchanged arguments; `name` is a NUL-terminated ANSI string when given.
    traced(|| unsafe { real(name, service, hints, result) }, |&r, t| {
        let host = if name.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(name.cast()).to_string_lossy().into_owned() } };
        record("net.resolve", host, t, r == 0)
    })
}

/// The imports wrapped: (name, slot for the original, wrapper).
fn wrappers() -> [(&'static str, &'static AtomicUsize, usize); 14] {
    [
        ("CreateFileW", &CREATE_FILE_W, create_file_w as *const () as usize),
        ("GetFileAttributesExW", &GET_FILE_ATTRIBUTES_EX_W, get_file_attributes_ex_w as *const () as usize),
        ("GetFileAttributesW", &GET_FILE_ATTRIBUTES_W, get_file_attributes_w as *const () as usize),
        ("FindFirstFileExW", &FIND_FIRST_FILE_EX_W, find_first_file_ex_w as *const () as usize),
        ("FindFirstFileW", &FIND_FIRST_FILE_W, find_first_file_w as *const () as usize),
        ("DeleteFileW", &DELETE_FILE_W, delete_file_w as *const () as usize),
        ("MoveFileExW", &MOVE_FILE_EX_W, move_file_ex_w as *const () as usize),
        ("CreateDirectoryW", &CREATE_DIRECTORY_W, create_directory_w as *const () as usize),
        ("ReadFile", &READ_FILE, read_file as *const () as usize),
        ("CreateProcessW", &CREATE_PROCESS_W, create_process_w as *const () as usize),
        ("NtCreateFile", &NT_CREATE_FILE, nt_create_file as *const () as usize),
        ("connect", &CONNECT, connect as *const () as usize),
        ("GetAddrInfoExW", &GET_ADDR_INFO_EX_W, get_addr_info_ex_w as *const () as usize),
        ("getaddrinfo", &GET_ADDR_INFO, get_addr_info as *const () as usize),
    ]
}

/// Points this module's import table entries for [`wrappers`] at the wrappers.
unsafe fn hook_imports() {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    // SAFETY: the main module's PE image, read by its documented layout (PE32+): e_lfanew at 0x3C,
    // the import directory at optional header + 120 (NT + 24 + 120), descriptors of 20 bytes.
    unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as *const u8;
        let nt = base.add(*(base.add(0x3c) as *const u32) as usize);
        let import_rva = *(nt.add(24 + 120) as *const u32) as usize;
        if import_rva == 0 {
            return;
        }
        let mut desc = base.add(import_rva);
        let list = wrappers();
        loop {
            let names_rva = *(desc as *const u32) as usize;
            let iat_rva = *(desc.add(16) as *const u32) as usize;
            if iat_rva == 0 {
                break;
            }
            let names = base.add(if names_rva != 0 { names_rva } else { iat_rva }) as *const u64;
            let iat = base.add(iat_rva) as *mut usize;
            let mut i = 0;
            loop {
                let entry = *names.add(i);
                if entry == 0 {
                    break;
                }
                if entry & (1 << 63) == 0 {
                    let name = std::ffi::CStr::from_ptr(base.add(entry as usize + 2).cast());
                    if let Some((_, slot, wrapper)) = list.iter().find(|(n, ..)| n.as_bytes() == name.to_bytes()) {
                        let at = iat.add(i);
                        let mut old = 0u32;
                        if VirtualProtect(at.cast(), std::mem::size_of::<usize>(), PAGE_READWRITE, &mut old) != 0 {
                            slot.store(*at, Ordering::Relaxed);
                            *at = *wrapper;
                            VirtualProtect(at.cast(), std::mem::size_of::<usize>(), old, &mut old);
                        }
                    }
                }
                i += 1;
            }
            desc = desc.add(20);
        }
    }
}

/// Records every DLL the process loads (`LdrRegisterDllNotification`), whoever loads it.
unsafe fn watch_dll_loads() {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type Register = unsafe extern "system" fn(u32, unsafe extern "system" fn(u32, *const c_void, *mut c_void), *mut c_void, *mut *mut c_void) -> i32;
    unsafe extern "system" fn loaded(reason: u32, data: *const c_void, _: *mut c_void) {
        if reason != 1 || BUSY.with(Cell::get) {
            return;
        }
        BUSY.with(|b| b.set(true));
        // SAFETY: LDR_DLL_LOADED_NOTIFICATION_DATA: Flags, then FullDllName (PCUNICODE_STRING) at 8.
        let name = unsafe { unicode(*((data as *const u8).add(8) as *const *const c_void)) };
        // The loader lock is held: no backtrace here, which could load more.
        // SAFETY: no arguments.
        let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        CALLS.lock().unwrap_or_else(|e| e.into_inner()).push(Call { at: Instant::now(), took_ns: 0, tid, op: "dll.load", path: name, ok: true, trace: None });
        BUSY.with(|b| b.set(false));
    }
    // SAFETY: ntdll's documented export, looked up by name and called with its signature.
    unsafe {
        let ntdll = GetModuleHandleW(windows_sys::core::w!("ntdll.dll"));
        let Some(register) = GetProcAddress(ntdll, c"LdrRegisterDllNotification".as_ptr().cast()) else { return };
        let register: Register = std::mem::transmute(register);
        let mut cookie = std::ptr::null_mut();
        register(0, loaded, std::ptr::null_mut(), &mut cookie);
    }
}
