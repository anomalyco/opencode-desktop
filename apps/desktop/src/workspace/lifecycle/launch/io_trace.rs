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
//! captures each call's return addresses, so a run with it is for the inventory, never for timing.
//!
//! **Nothing in a hook takes a lock, allocates, or calls the loader or the symbol engine**, on any
//! thread: a hook can run while another thread holds the loader lock (a DLL loading, a COM thread
//! starting), and a backtrace or a mutex there stopped the app at quit (the GPU thread's driver
//! loads, then the taskbar's and toasts' threads). Each call goes into a slot of a ring made at
//! [`start`], claimed with one atomic add, its path copied in and its frames taken raw
//! (`RtlCaptureStackBackTrace`); [`take`] resolves the frames to `file:line` (dbghelp) after the run.

#![allow(unsafe_code)]

use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use rune::time::Instant;

/// One call the app made.
pub struct Call {
    pub at: Instant,
    pub took_ns: u64,
    pub tid: u32,
    pub op: &'static str,
    pub path: String,
    pub ok: bool,
}

/// Calls a run keeps; later ones are counted in [`stats`] and dropped.
const SLOTS: usize = 16_384;
/// UTF-16 units of a path kept per call.
const PATH: usize = 300;
/// Return addresses kept per call.
const FRAMES: usize = 24;

/// A call as a hook writes it: plain data, no heap.
#[derive(Clone, Copy)]
struct Entry {
    at: Instant,
    took_ns: u64,
    tid: u32,
    op: &'static str,
    ok: bool,
    path_len: u16,
    path: [u16; PATH],
    frames_len: u8,
    frames: [usize; FRAMES],
}

struct Ring {
    slots: Box<[UnsafeCell<MaybeUninit<Entry>>]>,
    ready: Box<[AtomicBool]>,
    next: AtomicUsize,
}

// SAFETY: each slot is written by the one thread that claimed its index (`next`), and read only
// after its `ready` flag is published (Release/Acquire).
unsafe impl Sync for Ring {}

static RING: OnceLock<Ring> = OnceLock::new();
/// The hooks' own cost: calls timed and nanoseconds spent recording them (beyond the real call).
static COST_CALLS: AtomicU64 = AtomicU64::new(0);
static COST_NS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Inside a wrapper: calls the recorder itself makes go straight through.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// Starts recording when `--launch-trace-io` is given. First thing in `main`, before any other
/// thread runs: the ring is allocated here, never in a hook.
pub fn start() {
    if !crate::flag("--launch-trace-io") {
        return;
    }
    let _ = RING.set(Ring {
        slots: (0..SLOTS).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect(),
        ready: (0..SLOTS).map(|_| AtomicBool::new(false)).collect(),
        next: AtomicUsize::new(0),
    });
    // SAFETY: patches this module's own import table before any other thread runs.
    unsafe {
        hook_imports();
        watch_dll_loads();
    }
}

/// Calls dropped because the ring was full, and the hooks' mean cost per call in ns.
pub fn stats() -> (usize, u64) {
    let dropped = RING.get().map_or(0, |r| r.next.load(Ordering::Relaxed).saturating_sub(SLOTS));
    let calls = COST_CALLS.load(Ordering::Relaxed);
    (dropped, COST_NS.load(Ordering::Relaxed).checked_div(calls).unwrap_or(0))
}

/// Every call so far, oldest first, each with the app frames that made it (resolved now).
pub fn take() -> Vec<(Call, Vec<String>)> {
    let Some(ring) = RING.get() else { return Vec::new() };
    BUSY.with(|b| b.set(true));
    let n = ring.next.load(Ordering::Acquire).min(SLOTS);
    let entries: Vec<Entry> = (0..n)
        .filter(|&i| ring.ready[i].load(Ordering::Acquire))
        // SAFETY: a ready slot was written whole before its flag was published.
        .map(|i| unsafe { (*ring.slots[i].get()).assume_init() })
        .collect();
    let symbols = Symbols::new();
    let out = entries
        .iter()
        .map(|e| {
            let call = Call { at: e.at, took_ns: e.took_ns, tid: e.tid, op: e.op, path: String::from_utf16_lossy(&e.path[..e.path_len as usize]), ok: e.ok };
            (call, symbols.sites(&e.frames[..e.frames_len as usize]))
        })
        .collect();
    BUSY.with(|b| b.set(false));
    out
}

/// What a call names, read from the wrapped API's own arguments when the slot is written.
#[derive(Clone, Copy)]
enum Name {
    Wide(*const u16),
    Rename(*const u16, *const u16),
    Unicode(*const c_void),
    Ansi(*const u8),
    Socket(*const u8, i32),
}

/// Writes a call into the next slot: one atomic add, copies, and a raw stack capture. No lock,
/// no allocation, no loader or symbol call, so it may run while any thread holds the loader lock.
fn record(op: &'static str, name: Name, started: Instant, ok: bool) {
    let took_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let Some(ring) = RING.get() else { return };
    let i = ring.next.fetch_add(1, Ordering::Relaxed);
    if i >= SLOTS {
        return;
    }
    // SAFETY: no arguments; it reads the calling thread's id.
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let mut e = Entry { at: started, took_ns, tid, op, ok, path_len: 0, path: [0; PATH], frames_len: 0, frames: [0; FRAMES] };
    // SAFETY: each pointer is the wrapped API's own argument, of the kind its variant says.
    e.path_len = unsafe { copy_name(name, &mut e.path) } as u16;
    // SAFETY: an array of FRAMES pointers this frame owns. Skips this function and the wrapper.
    e.frames_len = unsafe { windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace(2, FRAMES as u32, e.frames.as_mut_ptr().cast(), std::ptr::null_mut()) } as u8;
    // SAFETY: slot `i` is this thread's alone (claimed above) and published once written.
    unsafe { (*ring.slots[i].get()).write(e) };
    ring.ready[i].store(true, Ordering::Release);
}

/// Runs `f` as a recorded call unless this thread is already recording, and keeps the
/// recording's own cost.
fn traced<R>(f: impl FnOnce() -> R, done: impl FnOnce(&R, Instant)) -> R {
    if BUSY.with(Cell::get) {
        return f();
    }
    BUSY.with(|b| b.set(true));
    let started = Instant::now();
    let r = f();
    let returned = Instant::now();
    done(&r, started);
    COST_NS.fetch_add(u64::try_from(returned.elapsed().as_nanos()).unwrap_or(0), Ordering::Relaxed);
    COST_CALLS.fetch_add(1, Ordering::Relaxed);
    BUSY.with(|b| b.set(false));
    r
}

/// Copies what `name` points at into `out` (UTF-16, cut at its length); returns the units written.
unsafe fn copy_name(name: Name, out: &mut [u16; PATH]) -> usize {
    let mut n = 0;
    let mut put = |u: u16| {
        if n < PATH {
            out[n] = u;
            n += 1;
        }
    };
    // SAFETY: the caller's pointers, as `record` documents.
    unsafe {
        match name {
            Name::Wide(p) => wide_into(p, &mut put),
            Name::Rename(from, to) => {
                wide_into(from, &mut put);
                " -> ".encode_utf16().for_each(&mut put);
                wide_into(to, &mut put);
            }
            Name::Unicode(s) => {
                if !s.is_null() {
                    // A UNICODE_STRING: u16 Length (bytes), u16 MaximumLength, pointer Buffer.
                    let len = *(s as *const u16) as usize / 2;
                    let buf = *((s as *const u8).add(8) as *const *const u16);
                    if !buf.is_null() {
                        (0..len).for_each(|i| put(*buf.add(i)));
                    }
                }
            }
            Name::Ansi(p) => {
                if !p.is_null() {
                    (0..).map(|i| *p.add(i)).take_while(|&b| b != 0).for_each(|b| put(u16::from(b)));
                }
            }
            Name::Socket(addr, len) => socket_into(addr, len, &mut put),
        }
    }
    n
}

/// A NUL-terminated UTF-16 string, unit by unit.
unsafe fn wide_into(p: *const u16, put: &mut impl FnMut(u16)) {
    if p.is_null() {
        return;
    }
    // SAFETY: the caller passes a NUL-terminated string, as every wrapped API requires.
    unsafe { (0..).map(|i| *p.add(i)).take_while(|&u| u != 0).for_each(put) }
}

/// `127.0.0.1:4096` or `[::1]:4096`, written without allocating.
unsafe fn socket_into(addr: *const u8, len: i32, put: &mut impl FnMut(u16)) {
    if addr.is_null() || len < 8 {
        return;
    }
    let num = |v: u32, put: &mut dyn FnMut(u16)| {
        let mut digits = [0u16; 10];
        let (mut k, mut v) = (0, v);
        loop {
            digits[k] = b'0' as u16 + (v % 10) as u16;
            k += 1;
            v /= 10;
            if v == 0 { break; }
        }
        (0..k).rev().for_each(|i| put(digits[i]));
    };
    // SAFETY: the caller's SOCKADDR is `len` bytes.
    unsafe {
        let family = *(addr as *const u16);
        let port = u16::from_be_bytes([*addr.add(2), *addr.add(3)]);
        match family {
            2 => {
                for k in 0..4 {
                    if k > 0 { put(u16::from(b'.')); }
                    num(u32::from(*addr.add(4 + k)), put);
                }
            }
            23 if len >= 24 => {
                put(u16::from(b'['));
                for k in 0..8 {
                    if k > 0 { put(u16::from(b':')); }
                    let group = u16::from_be_bytes([*addr.add(8 + 2 * k), *addr.add(9 + 2 * k)]);
                    for shift in [12, 8, 4, 0] {
                        let d = ((group >> shift) & 0xf) as u8;
                        put(u16::from(if d < 10 { b'0' + d } else { b'a' + d - 10 }));
                    }
                }
                put(u16::from(b']'));
            }
            _ => return,
        }
        put(u16::from(b':'));
        num(u32::from(port), put);
    }
}

/// dbghelp for this process, opened in [`take`] after the run and closed when dropped.
struct Symbols {
    process: windows_sys::Win32::Foundation::HANDLE,
    ok: bool,
}

impl Symbols {
    fn new() -> Symbols {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Diagnostics::Debug::{SYMOPT_DEFERRED_LOADS, SYMOPT_LOAD_LINES, SymInitializeW, SymSetOptions};
        // SAFETY: this process's pseudo-handle; dbghelp finds its modules and their PDBs itself.
        unsafe {
            let process = windows_sys::Win32::System::Threading::GetCurrentProcess();
            SymSetOptions(SYMOPT_LOAD_LINES | SYMOPT_DEFERRED_LOADS);
            // The exe's own folder (a copied exe with its PDB beside it), then the path the linker
            // wrote. A process std's backtrace already set up answers false and still resolves.
            let dir: Vec<u16> = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.as_os_str().encode_wide().chain([0]).collect())).unwrap_or_else(|| vec![0]);
            let ok = SymInitializeW(process, dir.as_ptr(), 1) != 0 || std::io::Error::last_os_error().raw_os_error() == Some(87);
            Symbols { process, ok }
        }
    }

    /// The innermost frames in the app's or Rune's code (at most four), as `crate/src/file.rs:line`.
    fn sites(&self, frames: &[usize]) -> Vec<String> {
        use windows_sys::Win32::System::Diagnostics::Debug::{IMAGEHLP_LINEW64, SymGetLineFromAddrW64};
        if !self.ok {
            return Vec::new();
        }
        let mut out = Vec::new();
        for &pc in frames {
            // SAFETY: an IMAGEHLP_LINEW64 with its size set, filled by dbghelp for address `pc - 1`
            // (inside the call, not after it); its FileName lives until the next dbghelp call.
            let (file, line) = unsafe {
                let mut info: IMAGEHLP_LINEW64 = std::mem::zeroed();
                info.SizeOfStruct = std::mem::size_of::<IMAGEHLP_LINEW64>() as u32;
                let mut displacement = 0u32;
                if SymGetLineFromAddrW64(self.process, (pc as u64).saturating_sub(1), &mut displacement, &mut info) == 0 || info.FileName.is_null() {
                    continue;
                }
                let len = (0..).take_while(|&i| *info.FileName.add(i) != 0).count();
                (String::from_utf16_lossy(std::slice::from_raw_parts(info.FileName, len)), info.LineNumber)
            };
            let ours = (file.contains(r"apps\desktop\src") || file.contains(r"\crates\") || file.contains(r"anomalyco\rune") || file.contains("rune-") && file.contains(r"\checkouts\"))
                && !file.contains("io_trace.rs");
            if ours {
                let short = file.rsplit_once(r"\src\").map_or(file.clone(), |(head, tail)| format!("{}/src/{}", head.rsplit('\\').next().unwrap_or(""), tail.replace('\\', "/")));
                out.push(format!("{short}:{line}"));
                if out.len() == 4 {
                    break;
                }
            }
        }
        out
    }
}

impl Drop for Symbols {
    fn drop(&mut self) {
        // SAFETY: the handle `new` initialised.
        unsafe { windows_sys::Win32::System::Diagnostics::Debug::SymCleanup(self.process) };
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
        record(op, Name::Wide(name), t, h != INVALID);
    })
}

static GET_FILE_ATTRIBUTES_EX_W: AtomicUsize = AtomicUsize::new(0);
type GetFileAttributesExW = unsafe extern "system" fn(*const u16, i32, *mut c_void) -> i32;
unsafe extern "system" fn get_file_attributes_ex_w(name: *const u16, level: i32, info: *mut c_void) -> i32 {
    let real = original!(GET_FILE_ATTRIBUTES_EX_W, GetFileAttributesExW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, level, info) }, |&ok, t| record("file.stat", Name::Wide(name), t, ok != 0))
}

static GET_FILE_ATTRIBUTES_W: AtomicUsize = AtomicUsize::new(0);
type GetFileAttributesW = unsafe extern "system" fn(*const u16) -> u32;
unsafe extern "system" fn get_file_attributes_w(name: *const u16) -> u32 {
    let real = original!(GET_FILE_ATTRIBUTES_W, GetFileAttributesW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name) }, |&a, t| record("file.stat", Name::Wide(name), t, a != u32::MAX))
}

static FIND_FIRST_FILE_EX_W: AtomicUsize = AtomicUsize::new(0);
type FindFirstFileExW = unsafe extern "system" fn(*const u16, i32, *mut c_void, i32, *const c_void, u32) -> isize;
unsafe extern "system" fn find_first_file_ex_w(name: *const u16, level: i32, data: *mut c_void, search: i32, filter: *const c_void, flags: u32) -> isize {
    let real = original!(FIND_FIRST_FILE_EX_W, FindFirstFileExW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, level, data, search, filter, flags) }, |&h, t| record("dir.scan", Name::Wide(name), t, h != INVALID))
}

static FIND_FIRST_FILE_W: AtomicUsize = AtomicUsize::new(0);
type FindFirstFileW = unsafe extern "system" fn(*const u16, *mut c_void) -> isize;
unsafe extern "system" fn find_first_file_w(name: *const u16, data: *mut c_void) -> isize {
    let real = original!(FIND_FIRST_FILE_W, FindFirstFileW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, data) }, |&h, t| record("dir.scan", Name::Wide(name), t, h != INVALID))
}

static DELETE_FILE_W: AtomicUsize = AtomicUsize::new(0);
type DeleteFileW = unsafe extern "system" fn(*const u16) -> i32;
unsafe extern "system" fn delete_file_w(name: *const u16) -> i32 {
    let real = original!(DELETE_FILE_W, DeleteFileW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name) }, |&ok, t| record("file.delete", Name::Wide(name), t, ok != 0))
}

static MOVE_FILE_EX_W: AtomicUsize = AtomicUsize::new(0);
type MoveFileExW = unsafe extern "system" fn(*const u16, *const u16, u32) -> i32;
unsafe extern "system" fn move_file_ex_w(from: *const u16, to: *const u16, flags: u32) -> i32 {
    let real = original!(MOVE_FILE_EX_W, MoveFileExW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(from, to, flags) }, |&ok, t| record("file.rename", Name::Rename(from, to), t, ok != 0))
}

static CREATE_DIRECTORY_W: AtomicUsize = AtomicUsize::new(0);
type CreateDirectoryW = unsafe extern "system" fn(*const u16, *const c_void) -> i32;
unsafe extern "system" fn create_directory_w(name: *const u16, sa: *const c_void) -> i32 {
    let real = original!(CREATE_DIRECTORY_W, CreateDirectoryW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, sa) }, |&ok, t| record("dir.create", Name::Wide(name), t, ok != 0))
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
        record("process.spawn", Name::Wide(if cmd.is_null() { app } else { cmd }), t, ok != 0)
    })
}

static NT_CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
type NtCreateFile = unsafe extern "system" fn(*mut isize, u32, *const c_void, *mut c_void, *const i64, u32, u32, u32, u32, *const c_void, u32) -> i32;
unsafe extern "system" fn nt_create_file(h: *mut isize, access: u32, oa: *const c_void, io: *mut c_void, size: *const i64, attrs: u32, share: u32, disposition: u32, options: u32, ea: *const c_void, ea_len: u32) -> i32 {
    let real = original!(NT_CREATE_FILE, NtCreateFile);
    // SAFETY: unchanged arguments; OBJECT_ATTRIBUTES holds ObjectName at offset 16.
    traced(|| unsafe { real(h, access, oa, io, size, attrs, share, disposition, options, ea, ea_len) }, |&status, t| {
        // SAFETY: OBJECT_ATTRIBUTES holds ObjectName (a PUNICODE_STRING) at offset 16.
        let name = if oa.is_null() { std::ptr::null() } else { unsafe { *((oa as *const u8).add(16) as *const *const c_void) } };
        record("file.open", Name::Unicode(name), t, status >= 0)
    })
}

static CONNECT: AtomicUsize = AtomicUsize::new(0);
type Connect = unsafe extern "system" fn(usize, *const u8, i32) -> i32;
unsafe extern "system" fn connect(s: usize, addr: *const u8, len: i32) -> i32 {
    let real = original!(CONNECT, Connect);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(s, addr, len) }, |&r, t| {
        // A non-blocking connect returns SOCKET_ERROR (WSAEWOULDBLOCK) while it proceeds.
        record("net.connect", Name::Socket(addr, len), t, r == 0 || r == -1)
    })
}

static GET_ADDR_INFO_EX_W: AtomicUsize = AtomicUsize::new(0);
type GetAddrInfoExW = unsafe extern "system" fn(*const u16, *const u16, u32, *const c_void, *const c_void, *mut *mut c_void, *const c_void, *const c_void, *const c_void, *mut isize) -> i32;
unsafe extern "system" fn get_addr_info_ex_w(name: *const u16, service: *const u16, ns: u32, id: *const c_void, hints: *const c_void, result: *mut *mut c_void, timeout: *const c_void, ov: *const c_void, done: *const c_void, handle: *mut isize) -> i32 {
    let real = original!(GET_ADDR_INFO_EX_W, GetAddrInfoExW);
    // SAFETY: unchanged arguments.
    traced(|| unsafe { real(name, service, ns, id, hints, result, timeout, ov, done, handle) }, |&r, t| record("net.resolve", Name::Wide(name), t, r == 0))
}

static GET_ADDR_INFO: AtomicUsize = AtomicUsize::new(0);
type GetAddrInfo = unsafe extern "system" fn(*const u8, *const u8, *const c_void, *mut *mut c_void) -> i32;
unsafe extern "system" fn get_addr_info(name: *const u8, service: *const u8, hints: *const c_void, result: *mut *mut c_void) -> i32 {
    let real = original!(GET_ADDR_INFO, GetAddrInfo);
    // SAFETY: unchanged arguments; `name` is a NUL-terminated ANSI string when given.
    traced(|| unsafe { real(name, service, hints, result) }, |&r, t| {
        record("net.resolve", Name::Ansi(name), t, r == 0)
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
        // The loader lock is held here: `record` takes no lock and loads nothing.
        // SAFETY: LDR_DLL_LOADED_NOTIFICATION_DATA: Flags, then FullDllName (PCUNICODE_STRING) at 8.
        let name = unsafe { *((data as *const u8).add(8) as *const *const c_void) };
        record("dll.load", Name::Unicode(name), Instant::now(), true);
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
