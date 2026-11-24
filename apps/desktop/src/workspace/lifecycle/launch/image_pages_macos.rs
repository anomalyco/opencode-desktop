//! macOS: the exe's own pages, asked for in large reads when the file is not in memory, instead
//! of one small read per page fault as the launch runs into them.
//!
//! A launch faults on about 2,400 places spread over the whole exe (`cpuof`'s page-ins, a CI
//! build of 116 MiB whose file was pushed out of the file cache: after a restart, a long pause or
//! memory pressure), each a read the faulting thread waits for; by the time the window shows,
//! every page of the file has been read that way, a few pages at a time. With the file in memory
//! the same launch takes 164. [`ask`], first thing in `main`, starts a thread that looks which of
//! the code and constants are in memory (`mincore`: no page is touched) and, when an eighth or
//! more is not, asks the system for those runs at once (`madvise(MADV_WILLNEED)`: it reads ahead
//! and does not wait). The launch then finds its pages in memory or on their way.
//!
//! With the exe in memory (most launches) nothing is asked for. `OPENCODE_RUNE_IMAGE_PAGES=off`
//! skips all of it (a bench's arm).
// The process's own Mach-O header and mapping; each block says what it relies on.
#![allow(unsafe_code)]

use std::ffi::c_void;

unsafe extern "C" {
    /// dyld's: the Mach-O header of image `index` as mapped; image 0 is the main exe.
    fn _dyld_get_image_header(index: u32) -> *const u8;
}

const LC_SEGMENT_64: u32 = 0x19;
/// `mincore`'s bit for a page that is in memory.
const IN_CORE: u8 = 1;
/// Runs of pages that are away, at most this many held pages apart, are asked for as one.
const JOIN: usize = 16;

/// First thing in `main`: starts the thread that asks for the exe's pages.
pub fn ask() {
    if std::env::var_os("OPENCODE_RUNE_IMAGE_PAGES").is_some_and(|v| v == "off") {
        return;
    }
    std::thread::Builder::new().name("image-pages".into()).spawn(run).ok();
}

fn run() {
    let Some((start, len)) = text() else { return };
    // SAFETY: a plain query.
    let page = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(16384).max(4096);
    let mut held = vec![0u8; len.div_ceil(page)];
    // SAFETY: `start..start + len` is this process's mapped `__TEXT` segment (`text`), page
    // aligned by the loader; `mincore` writes one byte a page into `held` and touches no page.
    if unsafe { libc::mincore(start as *const c_void, len, held.as_mut_ptr().cast()) } != 0 {
        return;
    }
    let away = held.iter().filter(|state| **state & IN_CORE == 0).count();
    if away * 8 < held.len() {
        return;
    }
    let mut at = 0;
    while at < held.len() {
        if held[at] & IN_CORE != 0 {
            at += 1;
            continue;
        }
        // The run goes on across gaps of up to JOIN pages that are in memory.
        let mut end = at + 1;
        let mut gap = 0;
        for next in at + 1..held.len() {
            if held[next] & IN_CORE == 0 {
                end = next + 1;
                gap = 0;
                continue;
            }
            gap += 1;
            if gap > JOIN {
                break;
            }
        }
        let bytes = ((end - at) * page).min(len - at * page);
        // SAFETY: a range inside the mapped segment; advice only, the mapping is unchanged.
        unsafe { libc::madvise((start + at * page) as *mut c_void, bytes, libc::MADV_WILLNEED) };
        at = end;
    }
}

/// Where the main exe's `__TEXT` segment (header, code, constants, unwind tables) is mapped,
/// and its length.
fn text() -> Option<(usize, usize)> {
    // SAFETY: image 0 is the main exe; dyld keeps its header mapped for the life of the process.
    let header = unsafe { _dyld_get_image_header(0) };
    if header.is_null() {
        return None;
    }
    // SAFETY: a 64-bit Mach-O header is 32 bytes: the count of load commands at 16, their
    // bytes at 20, the commands after it. Each command starts with its kind and its size.
    let word = |at: usize| unsafe { header.add(at).cast::<u32>().read_unaligned() };
    let (count, size) = (word(16), word(20) as usize);
    let mut at = 32;
    for _ in 0..count {
        if at + 8 > 32 + size {
            return None;
        }
        let (kind, length) = (word(at), word(at + 4) as usize);
        // SAFETY: an LC_SEGMENT_64 command is 72 bytes: the segment's name at 8 (16 bytes), its
        // size in memory at 32.
        if kind == LC_SEGMENT_64 && unsafe { std::slice::from_raw_parts(header.add(at + 8), 7) } == b"__TEXT\0" {
            // SAFETY: as above.
            let len = unsafe { header.add(at + 32).cast::<u64>().read_unaligned() };
            return Some((header as usize, usize::try_from(len).ok()?));
        }
        at += length.max(8);
    }
    None
}
