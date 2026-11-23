//! Windows: the exe's own pages, asked for before the launch faults on them one by one.
//!
//! A launch runs code and reads constants scattered over the exe's 160 MB. When those pages are
//! in memory each costs a soft fault; when they aren't (the first start of a new build, or
//! memory pressure has pushed them out of the standby list) each is a fault that waits on the
//! disk, one small read at a time, and the first frame comes five times later
//! (`tools\launch-bench.ts --fresh-exe`, 23 Nov: 207,000 us against 37,000 us, 88,000 us of it
//! CPU). [`ask`], before the launch reads anything else, starts a thread that asks Windows for all of them
//! at once (`PrefetchVirtualMemory`: a few large reads into the standby list, nothing added to
//! the working set), while the launch goes on:
//!
//! - the pages the earlier launches of this very exe used, from `<cache>/launch-pages.bin`;
//! - with no list for this exe yet (its first start), its code and constants in address order.
//!
//! It asks only when the pages are away: it reads one byte of sixteen of them first, and a page
//! in memory answers in a few microseconds where one on the disk takes fifty or more. With the
//! exe in memory (most launches) nothing is asked for: the call would take the locks the
//! launch's own page faults need.
//!
//! Once the launch has settled the same thread reads which of the exe's pages the process
//! holds (`QueryWorkingSetEx`) and, when the list lacks enough of them, writes the two joined.
//! The list names the exe by its link time stamp and image size, so a new build never uses an
//! old one's.
//!
//! `OPENCODE_RUNE_IMAGE_PAGES=off` skips all of it; `=all` asks for the whole code and
//! constants and `=listed` for a list's pages, both without looking first (a bench's arms).
// Reading this process's own image headers and working set is Win32; each block says what it relies on.
#![allow(unsafe_code)]

use std::time::{Duration, Instant};

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PrefetchVirtualMemory, WIN32_MEMORY_RANGE_ENTRY};
use windows_sys::Win32::System::ProcessStatus::{PSAPI_WORKING_SET_EX_INFORMATION, QueryWorkingSetEx};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};

const PAGE: usize = 4096;
/// The list file's first four bytes.
const MAGIC: u32 = u32::from_le_bytes(*b"RLP1");
/// Runs of pages at most this far apart are asked for as one. A later launch holds 6,500 pages
/// (26 MB) in 1,800 runs; joined at 2 they are 1,000 runs and 30 MB, at 16, 285 runs and 49 MB.
const JOIN: u32 = 2;
/// How many of the wanted pages are read to learn whether they are in memory, how long a read
/// of one that isn't takes at least, and how many such reads say the exe is away.
const SAMPLES: u32 = 16;
const AWAY: Duration = Duration::from_micros(25);
const AWAY_SAMPLES: u32 = 2;
/// How long after the first input the launch counts as settled.
const SETTLED: Duration = Duration::from_millis(750);
/// How often the thread looks for the first input, and for how long at most.
const LOOK: Duration = Duration::from_millis(250);
const LOOKS: u32 = 240;
/// The list is rewritten when the launch used this many pages it doesn't name (1 MB).
const WORTH_WRITING: u32 = 256;

/// The exe as loaded: where, which build, and the sections a launch reads (`.text`, `.rdata`)
/// as (first page, pages) from its base.
struct Image {
    base: usize,
    stamp: u32,
    size: u32,
    sections: Vec<(u32, u32)>,
}

/// Runs of pages, as (first page from the image's base, pages), in address order.
type Runs = Vec<(u32, u32)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A list's pages, else the whole code and constants, when they aren't in memory.
    Auto,
    All,
    Listed,
}

/// First thing in `main`: starts the thread that asks for the launch's pages.
pub fn ask() {
    let mode = match std::env::var_os("OPENCODE_RUNE_IMAGE_PAGES").as_deref().and_then(|v| v.to_str()) {
        Some("off") => return,
        Some("all") => Mode::All,
        Some("listed") => Mode::Listed,
        _ => Mode::Auto,
    };
    std::thread::Builder::new().name("image-pages".into()).spawn(move || run(mode)).ok();
}

fn run(mode: Mode) {
    let Some(image) = image() else { return };
    let path = crate::dirs::cache().map(|dir| dir.join("launch-pages.bin"));
    let listed = path.as_deref().and_then(|p| std::fs::read(p).ok()).and_then(|bytes| decode(&bytes, &image));
    let wanted = match (&listed, mode) {
        (Some(runs), Mode::Auto | Mode::Listed) => Some(runs.clone()),
        (None, Mode::Listed) => None,
        _ => Some(image.sections.clone()),
    };
    if let Some(runs) = &wanted {
        if mode != Mode::Auto || away(&image, runs) {
            prefetch(&image, runs);
            rune::marks::mark("image.pages.asked");
        } else {
            rune::marks::mark("image.pages.present");
        }
    }
    // The rest waits for the launch and must not compete with it.
    // SAFETY: the pseudo-handle of this thread.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    for _ in 0..LOOKS {
        if rune::marks::marks().iter().any(|(name, _)| *name == "input.ready") {
            break;
        }
        std::thread::sleep(LOOK);
    }
    std::thread::sleep(SETTLED);
    let Some(path) = path else { return };
    let used = held(&image);
    let known = listed.unwrap_or_default();
    let new: u32 = used.iter().map(|&(first, pages)| (first..first + pages).filter(|page| !covers(&known, *page)).count() as u32).sum();
    if new < WORTH_WRITING {
        return;
    }
    let all = join(known.into_iter().chain(used).collect(), JOIN);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    // Written beside and renamed: a launch that reads it meanwhile gets the old list or the new.
    let partial = path.with_extension("bin.new");
    if std::fs::write(&partial, encode(&all, &image)).is_ok() {
        std::fs::rename(&partial, &path).ok();
    }
    rune::marks::mark("image.pages.listed");
}

/// This process's exe, from its headers in memory.
fn image() -> Option<Image> {
    // SAFETY: a null name asks for the exe's own module, mapped for the life of the process; its
    // first page holds the DOS and PE headers and the section table (the loader read them from
    // there), read unaligned and inside that page.
    unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as usize;
        if base == 0 {
            return None;
        }
        let u16_at = |at: usize| std::ptr::read_unaligned((base + at) as *const u16);
        let u32_at = |at: usize| std::ptr::read_unaligned((base + at) as *const u32);
        let pe = u32_at(0x3c) as usize;
        if pe + 24 + 60 > PAGE || u32_at(pe) != u32::from_le_bytes(*b"PE\0\0") {
            return None;
        }
        let (count, optional) = (u16_at(pe + 6) as usize, u16_at(pe + 20) as usize);
        let table = pe + 24 + optional;
        if table + count * 40 > PAGE {
            return None;
        }
        let sections = (0..count)
            .map(|i| table + i * 40)
            .filter(|&at| {
                let name = std::slice::from_raw_parts((base + at) as *const u8, 8);
                name.starts_with(b".text\0") || name.starts_with(b".rdata\0")
            })
            .map(|at| (u32_at(at + 12) / PAGE as u32, u32_at(at + 8).div_ceil(PAGE as u32)))
            .collect();
        Some(Image { base, stamp: u32_at(pe + 8), size: u32_at(pe + 24 + 56), sections })
    }
}

/// Whether `runs`' pages wait on the disk: [`SAMPLES`] of them, evenly spread, read one byte each.
fn away(image: &Image, runs: &[(u32, u32)]) -> bool {
    let total: u64 = runs.iter().map(|&(_, pages)| u64::from(pages)).sum();
    let mut slow = 0;
    for i in 0..u64::from(SAMPLES) {
        let mut nth = total * (2 * i + 1) / (2 * u64::from(SAMPLES));
        let Some(page) = runs.iter().find_map(|&(first, pages)| {
            if nth < u64::from(pages) {
                return Some(u64::from(first) + nth);
            }
            nth -= u64::from(pages);
            None
        }) else {
            continue;
        };
        if (page + 1) * PAGE as u64 > u64::from(image.size) {
            continue;
        }
        let started = Instant::now();
        // SAFETY: a page inside this process's own image (checked against its size above), in
        // its code or constants, which stay mapped and readable for the life of the process.
        std::hint::black_box(unsafe { std::ptr::read_volatile((image.base + page as usize * PAGE) as *const u8) });
        if started.elapsed() >= AWAY {
            slow += 1;
        }
    }
    slow >= AWAY_SAMPLES
}

/// Asks Windows to bring `runs` into memory.
fn prefetch(image: &Image, runs: &[(u32, u32)]) {
    // `OPENCODE_RUNE_IMAGE_PAGES_JOIN`: the gap for this ask, for timing one gap against another.
    let gap = std::env::var("OPENCODE_RUNE_IMAGE_PAGES_JOIN").ok().and_then(|v| v.parse().ok()).unwrap_or(JOIN);
    let mut ranges: Vec<WIN32_MEMORY_RANGE_ENTRY> = join(runs.to_vec(), gap)
        .into_iter()
        .filter(|&(first, pages)| (first as usize + pages as usize) * PAGE <= image.size as usize)
        .map(|(first, pages)| WIN32_MEMORY_RANGE_ENTRY { VirtualAddress: (image.base + first as usize * PAGE) as *mut _, NumberOfBytes: pages as usize * PAGE })
        .collect();
    if ranges.is_empty() {
        return;
    }
    // SAFETY: every range lies inside this process's own image (checked against its size above),
    // and the array outlives the call. A failure (an OS without the call's support for images)
    // leaves the launch as it was.
    unsafe { PrefetchVirtualMemory(GetCurrentProcess(), ranges.len(), ranges.as_mut_ptr(), 0) };
}

/// The pages of the image's code and constants this process holds now.
fn held(image: &Image) -> Runs {
    let mut runs = Runs::new();
    for &(first, pages) in &image.sections {
        let mut info: Vec<PSAPI_WORKING_SET_EX_INFORMATION> = (0..pages as usize)
            .map(|i| PSAPI_WORKING_SET_EX_INFORMATION {
                VirtualAddress: (image.base + (first as usize + i) * PAGE) as *mut _,
                // SAFETY: a plain-data union of integers, for which zero is a value.
                VirtualAttributes: unsafe { std::mem::zeroed() },
            })
            .collect();
        // SAFETY: an array this frame owns, with its size in bytes; the call fills each entry's attributes.
        let ok = unsafe { QueryWorkingSetEx(GetCurrentProcess(), info.as_mut_ptr().cast(), (info.len() * std::mem::size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32) };
        if ok == 0 {
            continue;
        }
        for (i, entry) in info.iter().enumerate() {
            // SAFETY: the union's integer view; bit 0 says the page is in the working set.
            if unsafe { entry.VirtualAttributes.Flags } & 1 == 0 {
                continue;
            }
            let page = first + i as u32;
            match runs.last_mut() {
                Some((start, len)) if *start + *len == page => *len += 1,
                _ => runs.push((page, 1)),
            }
        }
    }
    runs
}

fn covers(runs: &[(u32, u32)], page: u32) -> bool {
    let i = runs.partition_point(|&(first, _)| first <= page);
    i > 0 && page < runs[i - 1].0 + runs[i - 1].1
}

/// `runs` sorted, with neighbours and near neighbours ([`JOIN`]) made one.
fn join(mut runs: Runs, gap: u32) -> Runs {
    runs.sort_unstable();
    let mut out = Runs::with_capacity(runs.len());
    for (first, pages) in runs {
        match out.last_mut() {
            Some((start, len)) if first <= *start + *len + gap => *len = (*len).max(first + pages - *start),
            _ => out.push((first, pages)),
        }
    }
    out
}

fn encode(runs: &[(u32, u32)], image: &Image) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + runs.len() * 8);
    for word in [MAGIC, image.stamp, image.size, runs.len() as u32] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    for &(first, pages) in runs {
        out.extend_from_slice(&first.to_le_bytes());
        out.extend_from_slice(&pages.to_le_bytes());
    }
    out
}

/// The list in `bytes`, when it is this exe's and whole.
fn decode(bytes: &[u8], image: &Image) -> Option<Runs> {
    let word = |i: usize| bytes.get(i * 4..i * 4 + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if word(0)? != MAGIC || word(1)? != image.stamp || word(2)? != image.size {
        return None;
    }
    let count = word(3)? as usize;
    if bytes.len() != 16 + count * 8 {
        return None;
    }
    let pages = image.size / PAGE as u32;
    (0..count)
        .map(|i| Some((word(4 + i * 2)?, word(5 + i * 2)?)))
        .map(|run| run.filter(|&(first, len)| len > 0 && first.checked_add(len).is_some_and(|end| end <= pages)))
        .collect()
}
