//! Linux: the exe's own pages, asked for before the launch faults on them one by one (what
//! `image_pages.rs` does on Windows, with Linux's calls).
//!
//! When the exe's pages are not in the page cache (the first start after an install or an
//! update, or memory pressure has dropped them) each page fault of the launch waits on the
//! disk. [`ask`], first thing in `main`, starts a thread that asks the kernel for them at once
//! while the launch goes on:
//!
//! - the pages the earlier launches of this very exe used, from `<cache>/launch-pages.bin`;
//! - with no list for this exe yet (its first start), its code and constants in address order.
//!
//! It asks only when the pages are away, and learns that as Windows does: it reads one byte of
//! sixteen of them and times each read (a page in the cache answers in a microsecond or two, one
//! on the disk in fifty or more). `mincore` and `cachestat` would say it without a read, but they
//! answer "in memory" for a file the user may not write, which is every installed exe.
//!
//! The asking is `madvise(MADV_WILLNEED)`. One call reads no more than the device's read-ahead
//! window or its largest read, whichever is larger (`force_page_cache_ra`): of this exe's
//! 188 MiB one call over all of it brought in 8 MiB on a device with an 8 MiB window, and a
//! device with the default window would give 128 KiB. So a run is asked for in pieces of
//! [`ASK`] bytes, which brought in all of it in 43 ms.
//!
//! Windows' second step, mapping the listed pages into the process ahead of the launch's own
//! threads, is not here: on Linux (`MADV_POPULATE_READ`) it made a launch with the exe in memory
//! 1,100 us slower to its first frame, twice in two blocks of nine launches, and one without it
//! no faster or slower than with nothing at all. A fault on a page in the cache maps its
//! neighbours too, so there is little to save.
//!
//! Once the launch has settled the same thread reads which of the exe's pages the process has
//! mapped (`/proc/self/pagemap`) and, when the list lacks enough of them, writes the two joined.
//! The list names the exe by its file's size and time, so a new build never uses an old one's.
//!
//! `OPENCODE_RUNE_IMAGE_PAGES=off` skips all of it; `=all` asks for the whole code and constants
//! and `=listed` for a list's pages, both without looking first (a bench's arms).
// Reading this process's own program headers and advising the kernel about its own mapping is
// libc; each block says what it relies on.
#![allow(unsafe_code)]

use std::os::unix::fs::{FileExt, MetadataExt};
use std::time::{Duration, Instant};

/// The list file's first four bytes.
const MAGIC: u32 = u32::from_le_bytes(*b"RLP1");
/// Runs of pages at most this far apart are listed as one.
const JOIN: u32 = 2;
/// Runs at most this far apart are asked for as one.
const ASK_JOIN: u32 = 16;
/// One `madvise(MADV_WILLNEED)` call's bytes: the kernel reads no more per call than the larger
/// of the device's largest read and its read-ahead window, and 128 KiB is the least either is.
const ASK: usize = 128 * 1024;
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
/// The list is rewritten when the launch used this many pages it doesn't name.
const WORTH_WRITING: u32 = 256;

/// The exe as loaded: where, the machine's page size, which build, how many pages it spans, and
/// the parts a launch reads (the segments it may not write: code and constants) as
/// (first page, pages) from its base.
struct Image {
    base: usize,
    page: usize,
    stamp: u32,
    size: u32,
    pages: u32,
    sections: Runs,
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

    // The rest waits for the launch and must not compete with it: on Linux a thread has a
    // niceness of its own.
    // SAFETY: this thread's own id; a refusal leaves the thread as it was.
    unsafe { libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10) };
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

/// This process's exe, from its program headers in memory.
fn image() -> Option<Image> {
    struct Found {
        base: usize,
        loads: Vec<(usize, usize, bool)>,
    }
    /// The first object the loader reports is the program itself.
    unsafe extern "C" fn first(info: *mut libc::dl_phdr_info, _size: libc::size_t, data: *mut libc::c_void) -> libc::c_int {
        // SAFETY: the loader passes a valid object description for the length of the call, with
        // `dlpi_phnum` program headers at `dlpi_phdr`; `data` is the `Found` this call was given.
        unsafe {
            let (info, found) = (&*info, &mut *data.cast::<Found>());
            found.base = info.dlpi_addr as usize;
            if !info.dlpi_phdr.is_null() {
                found.loads = std::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize)
                    .iter()
                    .filter(|header| header.p_type == libc::PT_LOAD)
                    .map(|header| (header.p_vaddr as usize, header.p_memsz as usize, header.p_flags & libc::PF_W != 0))
                    .collect();
            }
        }
        1
    }
    let mut found = Found { base: 0, loads: Vec::new() };
    // SAFETY: the callback only reads what the loader hands it and writes to `found`, which
    // outlives the call.
    unsafe { libc::dl_iterate_phdr(Some(first), (&raw mut found).cast()) };
    // SAFETY: no pointers; the answer is a number.
    let page = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok().filter(|page| page.is_power_of_two())?;
    let end = found.loads.iter().map(|&(at, len, _)| at + len).max()?;
    let sections = found
        .loads
        .iter()
        .filter(|&&(_, len, written)| !written && len > 0)
        .map(|&(at, len, _)| ((at / page) as u32, (at % page + len).div_ceil(page) as u32))
        .collect();
    let file = std::fs::metadata("/proc/self/exe").ok()?;
    Some(Image {
        base: found.base,
        page,
        stamp: (file.mtime() as u32) ^ (file.mtime_nsec() as u32).rotate_left(16),
        size: file.len() as u32,
        pages: u32::try_from(end.div_ceil(page)).ok()?,
        sections,
    })
}

/// Whether `runs`' pages wait on the disk: [`SAMPLES`] of them, evenly spread, read one byte each.
fn away(image: &Image, runs: &[(u32, u32)]) -> bool {
    let total: u64 = runs.iter().map(|&(_, pages)| u64::from(pages)).sum();
    let mut slow = 0;
    for i in 0..u64::from(SAMPLES) {
        let mut nth = total * (2 * i + 1) / (2 * u64::from(SAMPLES));
        let Some(page) = runs.iter().find_map(|&(first, pages)| {
            if nth < u64::from(pages) {
                return Some(first + nth as u32);
            }
            nth -= u64::from(pages);
            None
        }) else {
            continue;
        };
        if !readable(image, page) {
            continue;
        }
        let started = Instant::now();
        // SAFETY: a page of this process's own code or constants (`readable`), which stay mapped
        // and readable for the life of the process.
        std::hint::black_box(unsafe { std::ptr::read_volatile((image.base + page as usize * image.page) as *const u8) });
        if started.elapsed() >= AWAY {
            slow += 1;
        }
    }
    slow >= AWAY_SAMPLES
}

/// Whether `page` lies in the image's code or constants.
fn readable(image: &Image, page: u32) -> bool {
    image.sections.iter().any(|&(first, pages)| page >= first && page - first < pages)
}

/// `runs` cut down to the image's code and constants, with near neighbours joined at `gap`.
fn within(image: &Image, runs: &[(u32, u32)], gap: u32) -> Runs {
    join(runs.to_vec(), gap)
        .into_iter()
        .flat_map(|(first, pages)| {
            image.sections.iter().filter_map(move |&(section, len)| {
                let (from, to) = (first.max(section), (first + pages).min(section + len));
                (from < to).then(|| (from, to - from))
            })
        })
        .collect()
}

/// Asks the kernel to start reading `runs` into the page cache.
fn prefetch(image: &Image, runs: &[(u32, u32)]) {
    for (first, pages) in within(image, runs, ASK_JOIN) {
        let (mut at, end) = (image.base + first as usize * image.page, image.base + (first + pages) as usize * image.page);
        while at < end {
            let len = ASK.min(end - at);
            // SAFETY: a page-aligned range inside this process's own code or constants
            // (`within`); the call changes no memory and its failure leaves the launch as it was.
            unsafe { libc::madvise(at as *mut libc::c_void, len, libc::MADV_WILLNEED) };
            at += len;
        }
    }
}

/// The pages of the image's code and constants this process has mapped now.
fn held(image: &Image) -> Runs {
    let mut runs = Runs::new();
    let Ok(pagemap) = std::fs::File::open("/proc/self/pagemap") else { return runs };
    for &(first, pages) in &image.sections {
        // One 64-bit entry per page of the address space; bit 63 says the page is present.
        let mut entries = vec![0u8; pages as usize * 8];
        let at = ((image.base / image.page) as u64 + u64::from(first)) * 8;
        if pagemap.read_exact_at(&mut entries, at).is_err() {
            continue;
        }
        for (i, entry) in entries.chunks_exact(8).enumerate() {
            if entry[7] & 0x80 == 0 {
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

/// `runs` sorted, with neighbours and near neighbours (at most `gap` pages apart) made one.
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
    (0..count)
        .map(|i| Some((word(4 + i * 2)?, word(5 + i * 2)?)))
        .map(|run| run.filter(|&(first, len)| len > 0 && first.checked_add(len).is_some_and(|end| end <= image.pages)))
        .collect()
}
