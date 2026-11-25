//! Where a fatal exception happened, said by the process that had it (Windows).
//!
//! `last-exit.bin` records that a session ended with an exception, and its code. This adds the
//! place: the faulting instruction and the frames above it as `library+offset`, written to
//! `<logs>/last-fault.txt` (the newest fault only) and to stderr before the process ends. It needs
//! no debugger and no report from Windows: a launch from a script inherits an error mode in which
//! Windows writes none, and a fault in a launch's first half second comes before the profile's
//! own record can be written. A headless run writes it too.
//!
//! It runs inside the unhandled-exception filter of a process that may be short of stack or heap
//! and may hold any lock: one faulting thread only, fixed buffers, no allocation, no lock of the
//! app's, and no call that takes the loader's lock. A library is found with `RtlPcToFileHeader`
//! (the table exception dispatch read to get here) and named by the file its image is mapped from
//! (`NtQueryVirtualMemory`), of which only the last path component is written: never a folder,
//! so never a user's name. An offset is turned into a function with the build's debug file.
#![allow(unsafe_code)]

use core::cell::UnsafeCell;
use core::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, WriteFile,
};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{RtlCaptureStackBackTrace, RtlPcToFileHeader};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryVirtualMemory(
        process: HANDLE,
        base: *const c_void,
        class: u32,
        information: *mut c_void,
        length: usize,
        returned: *mut usize,
    ) -> i32;
}

/// MEMORY_INFORMATION_CLASS `MemoryMappedFilenameInformation`: a `UNICODE_STRING`, then its text.
const MAPPED_FILE_NAME: u32 = 2;

/// `EXCEPTION_RECORD` on 64-bit Windows.
#[repr(C)]
struct Record {
    code: u32,
    _flags: u32,
    _chained: *const Record,
    address: *const c_void,
    parameters: u32,
    information: [usize; 15],
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    _maximum: u16,
    buffer: *const u16,
}

const TEXT: usize = 4096;
/// `RtlCaptureStackBackTrace` takes fewer than 63 frames on the oldest Windows it runs on.
const FRAMES: usize = 62;
/// Frames written, from the faulting one outwards.
const SHOWN: usize = 24;
/// The mapped file's name with its device path: 1,120 bytes, aligned for the `UNICODE_STRING`.
const NAME_WORDS: usize = 140;

struct Scratch {
    text: [u8; TEXT],
    frames: [*mut c_void; FRAMES],
    name: [u64; NAME_WORDS],
}

struct Shared(UnsafeCell<Scratch>);
// SAFETY: only the one thread that wins `TAKEN` touches the scratch, for the rest of the process.
unsafe impl Sync for Shared {}

static SCRATCH: Shared =
    Shared(UnsafeCell::new(Scratch { text: [0; TEXT], frames: [core::ptr::null_mut(); FRAMES], name: [0; NAME_WORDS] }));
static TAKEN: AtomicBool = AtomicBool::new(false);
/// `last-fault.txt` in the log folder, and that folder with the folders above it (outermost
/// first), as NUL-terminated UTF-16, made when the hooks are armed.
static PATHS: OnceLock<(Vec<u16>, Vec<Vec<u16>>)> = OnceLock::new();

/// Names the file a fault is written to. No file I/O: the folder is made, if it must be, by the fault.
pub(super) fn arm(logs: Option<PathBuf>) {
    use std::os::windows::ffi::OsStrExt as _;
    let Some(logs) = logs else { return };
    let wide = |path: &Path| path.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    // A launch's first moments, or a headless run: the log folder may not be there yet, nor the
    // one above it. The drive itself is left out.
    let mut folders: Vec<Vec<u16>> = logs.ancestors().filter(|folder| folder.parent().is_some()).map(wide).collect();
    folders.reverse();
    PATHS.set((wide(&logs.join("last-fault.txt")), folders)).ok();
}

/// Text into a fixed buffer; what does not fit is dropped.
struct Out<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Out<'_> {
    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if let Some(slot) = self.buf.get_mut(self.len) {
                *slot = b;
                self.len += 1;
            }
        }
    }

    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    /// `v` in upper-case hex, at least `digits` long.
    fn hex(&mut self, v: u64, digits: usize) {
        let mut started = false;
        for i in (0..16usize).rev() {
            let nibble = ((v >> (i * 4)) & 0xF) as usize;
            if nibble != 0 || started || i < digits {
                started = true;
                self.bytes(&[b"0123456789ABCDEF"[nibble]]);
            }
        }
    }

    fn dec(&mut self, v: u64) {
        let mut digits = [0u8; 20];
        let mut left = v;
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (left % 10) as u8;
            left /= 10;
            if left == 0 {
                break;
            }
        }
        self.bytes(&digits[at..]);
    }
}

/// `library.dll+0xOFFSET` for an address inside a mapped image, else the bare address.
fn place(out: &mut Out<'_>, name: &mut [u64; NAME_WORDS], pc: *const c_void) {
    let mut base: *mut c_void = core::ptr::null_mut();
    // SAFETY: any address may be asked for; `base` is a valid out-pointer on this stack.
    unsafe { RtlPcToFileHeader(pc, &raw mut base) };
    if base.is_null() {
        out.str("0x");
        out.hex(pc as u64, 16);
        return;
    }
    let bytes = core::mem::size_of_val(name);
    let mut returned = 0usize;
    // SAFETY: `base` is the start of an image mapped in this process; `name` is writable for
    // `bytes` bytes and aligned for the `UNICODE_STRING` the call puts at its start.
    let status = unsafe {
        NtQueryVirtualMemory(GetCurrentProcess(), base, MAPPED_FILE_NAME, name.as_mut_ptr().cast(), bytes, &raw mut returned)
    };
    let mut named = false;
    if status >= 0 {
        // SAFETY: on success `name` begins with a `UNICODE_STRING`; `name` is aligned for it.
        let header = unsafe { &*name.as_ptr().cast::<UnicodeString>() };
        let start = name.as_ptr() as usize;
        let text = header.buffer as usize;
        let len = usize::from(header.length);
        if text >= start && text.checked_add(len).is_some_and(|end| end <= start + bytes) {
            // SAFETY: checked above: `len` bytes of UTF-16 that lie inside `name`.
            let chars = unsafe { core::slice::from_raw_parts(header.buffer, len / 2) };
            // The file's own name: nothing of the folder it is in.
            let tail = chars.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
            for &c in chars.iter().skip(tail) {
                out.bytes(&[if (0x20..0x7F).contains(&c) { c as u8 } else { b'?' }]);
                named = true;
            }
        }
    }
    if !named {
        out.str("?");
    }
    out.str("+0x");
    out.hex((pc as u64).wrapping_sub(base as u64), 1);
}

/// Writes where the exception of `record` (an `EXCEPTION_RECORD`) happened: once a process, by
/// the unhandled-exception filter, on the faulting thread.
///
/// # Safety
/// `record` is null or the `EXCEPTION_RECORD` Windows passed the filter.
pub(super) unsafe fn write(record: *const c_void) {
    if record.is_null() || TAKEN.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: this thread took `TAKEN`: nothing else reads or writes the scratch from now on.
    let Scratch { text, frames, name } = unsafe { &mut *SCRATCH.0.get() };
    // SAFETY: the caller's contract: a valid `EXCEPTION_RECORD`.
    let record = unsafe { &*record.cast::<Record>() };
    let mut out = Out { buf: &mut text[..], len: 0 };
    out.str("opencode-rune ");
    out.str(crate::updater::build_version());
    out.str(": fatal exception 0x");
    out.hex(u64::from(record.code), 8);
    // An access violation or an in-page error says what it did to which address.
    if matches!(record.code, 0xC000_0005 | 0xC000_0006) && record.parameters >= 2 {
        out.str(match record.information[0] {
            0 => " (read of 0x",
            1 => " (write of 0x",
            8 => " (execute of 0x",
            _ => " (access to 0x",
        });
        out.hex(record.information[1] as u64, 16);
        out.str(")");
    }
    out.str(" on thread ");
    // SAFETY: no parameters, no preconditions.
    out.dec(u64::from(unsafe { GetCurrentThreadId() }));
    out.str(" at ");
    place(&mut out, name, record.address);
    out.str("\n");
    // This thread's stack: the filter and exception dispatch on top, then the faulting frames.
    // Not after a stack overflow: the walk needs stack this thread no longer has.
    let captured = if record.code == 0xC000_00FD {
        0
    } else {
        // SAFETY: `frames` is writable for `FRAMES` pointers; the hash is not asked for.
        usize::from(unsafe { RtlCaptureStackBackTrace(0, FRAMES as u32, frames.as_mut_ptr(), core::ptr::null_mut()) })
    };
    let frames = &frames[..captured.min(FRAMES)];
    // From the faulting instruction on; all of it when the walk did not pass through that frame.
    let first = frames.iter().position(|&frame| frame.cast_const() == record.address).unwrap_or(0);
    for (i, &frame) in frames.iter().skip(first).take(SHOWN).enumerate() {
        out.str(if i < 10 { "opencode-rune:   #0" } else { "opencode-rune:   #" });
        out.dec(i as u64);
        out.str(" ");
        place(&mut out, name, frame);
        out.str("\n");
    }
    let len = out.len;
    emit(&text[..len]);
}

/// To `<logs>/last-fault.txt` (replacing an older one), then to stderr when the starter gave one.
fn emit(text: &[u8]) {
    let mut written = 0u32;
    if let Some((file, folders)) = PATHS.get() {
        let open = || {
            // SAFETY: `file` is a NUL-terminated UTF-16 path that lives as long as the process.
            unsafe {
                CreateFileW(
                    file.as_ptr(),
                    GENERIC_WRITE,
                    FILE_SHARE_READ,
                    core::ptr::null(),
                    CREATE_ALWAYS,
                    FILE_ATTRIBUTE_NORMAL,
                    core::ptr::null_mut(),
                )
            }
        };
        let mut handle = open();
        if handle == INVALID_HANDLE_VALUE {
            // The log folder is not there yet: it is made, with what is missing above it (a
            // folder that exists answers with an error, which is ignored).
            for folder in folders {
                // SAFETY: `folder` is a NUL-terminated UTF-16 path that lives as long as the process.
                unsafe { CreateDirectoryW(folder.as_ptr(), core::ptr::null()) };
            }
            handle = open();
        }
        if handle != INVALID_HANDLE_VALUE && !handle.is_null() {
            // SAFETY: `handle` is the file just opened for writing; `text` is readable for its
            // length; the handle is closed once.
            unsafe {
                WriteFile(handle, text.as_ptr(), text.len() as u32, &raw mut written, core::ptr::null_mut());
                CloseHandle(handle);
            }
        }
    }
    // SAFETY: asks for this process's own handle; a missing one is null or invalid and is skipped.
    let stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if !stderr.is_null() && stderr != INVALID_HANDLE_VALUE {
        // SAFETY: `stderr` is this process's error stream; `text` is readable for its length.
        unsafe { WriteFile(stderr, text.as_ptr(), text.len() as u32, &raw mut written, core::ptr::null_mut()) };
    }
}
