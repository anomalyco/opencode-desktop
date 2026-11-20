//! The app process's memory for `Diagnostics:Memory`: what the OS counts and what the app's
//! counting allocator holds.

use serde_json::{Value, json};

/// The report as JSON: `footprint`, `footprintPeak`, `resident` and `heap`.
pub fn report() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let usage = procwatch::usage();
        let heap = procwatch::alloc::heap().map_or(Value::Null, |h| {
            let tags: serde_json::Map<String, Value> = h.tags.iter().map(|(name, bytes)| ((*name).to_owned(), json!(bytes))).collect();
            json!({ "live": h.live, "peak": h.peak, "allocations": h.allocations, "largest": h.largest, "tags": tags })
        });
        json!({
            "footprint": usage.footprint,
            "footprintPeak": usage.footprint_peak,
            "resident": usage.resident,
            "heap": heap,
        })
        .to_string()
    }
    #[cfg(target_arch = "wasm32")]
    {
        json!({
            "footprint": Value::Null,
            "footprintPeak": Value::Null,
            "resident": Value::Null,
            "heap": Value::Null,
        })
        .to_string()
    }
}

/// What each plugin's Luau VMs hold, as of the last poll, for a measuring build's heap dumps.
#[cfg(feature = "alloc-sites")]
static VMS: std::sync::Mutex<Vec<(String, usize)>> = std::sync::Mutex::new(Vec::new());

/// `App::rest`: every running VM (window halves and app halves) collects its garbage in full.
pub fn collect_garbage(app: &crate::App) {
    for p in app.plugins.list.iter() { if let Some(s) = p.script() { s.collect_garbage(); } }
    super::app_half::each(|a| { a.script.collect_garbage(); None::<()> });
}

/// Notes each running VM's Luau heap (window halves and app halves), for [`register_reports`].
#[cfg(feature = "alloc-sites")]
pub fn publish_vms(app: &crate::App) {
    let mut vms: Vec<(String, usize)> = app.plugins.list.iter().filter_map(|p| p.script().map(|s| (p.half.id.to_string(), s.memory()))).collect();
    vms.extend(super::app_half::each(|a| Some((format!("{}@app", a.half.id), a.script.memory()))));
    if let Ok(mut v) = VMS.lock() {
        *v = vms;
    }
}

/// A measuring build's heap dumps (procwatch `alloc-sites`) also list each plugin VM's Luau heap
/// and SQLite's own heap, which the allocator sites can't name by plugin or see at all.
#[cfg(all(feature = "alloc-sites", not(target_arch = "wasm32")))]
pub fn register_reports() {
    opencode_storage::count_sqlite_memory();
    procwatch::alloc::sites::contribute("luau-vms", || VMS.lock().map(|v| v.iter().map(|(id, bytes)| format!("vm {bytes} {id}\n")).collect()).unwrap_or_default());
    procwatch::alloc::sites::contribute("sqlite", || {
        let (used, high) = opencode_storage::sqlite_memory();
        format!("sqlite {used} {high}\n")
    });
    procwatch::alloc::sites::contribute("mimalloc-pages", mimalloc_pages);
}

/// mimalloc's pages by block size: `size <block> <pages> <committed> <live bytes> <OS-committed>`, the slack
/// (committed less live) largest first, then `total` and the same for abandoned pages (a thread
/// that ended; seen only with `MIMALLOC_VISIT_ABANDONED=1`). Read from the dump thread while
/// others allocate, so a measure, not an exact count.
#[cfg(all(feature = "alloc-sites", not(target_arch = "wasm32")))]
#[allow(unsafe_code)]
fn mimalloc_pages() -> String {
    #[repr(C)]
    struct Area { blocks: *mut std::ffi::c_void, reserved: usize, committed: usize, used: usize, block_size: usize, full_block_size: usize, reserved1: *mut std::ffi::c_void }
    type Visit = unsafe extern "C" fn(*const std::ffi::c_void, *const Area, *mut std::ffi::c_void, usize, *mut std::ffi::c_void) -> bool;
    unsafe extern "C" {
        fn mi_heap_main() -> *mut std::ffi::c_void;
        fn mi_heap_visit_blocks(heap: *mut std::ffi::c_void, visit_blocks: bool, visitor: Visit, arg: *mut std::ffi::c_void) -> bool;
        fn mi_heap_visit_abandoned_blocks(heap: *mut std::ffi::c_void, visit_blocks: bool, visitor: Visit, arg: *mut std::ffi::c_void) -> bool;
    }
    unsafe extern "C" fn visit(_: *const std::ffi::c_void, area: *const Area, block: *mut std::ffi::c_void, _: usize, arg: *mut std::ffi::c_void) -> bool {
        // SAFETY: mimalloc passes a live area, and `arg` is the map below, for this call only.
        let (area, by) = unsafe { (&*area, &mut *arg.cast::<std::collections::BTreeMap<usize, [usize; 4]>>()) };
        if block.is_null() {
            let e = by.entry(area.block_size).or_default();
            e[0] += 1;
            e[1] += area.committed;
            e[2] += area.used * area.block_size;
            e[3] += os_committed(area.blocks as usize, area.reserved);
        }
        true
    }
    let mut out = String::new();
    for (name, abandoned) in [("live", false), ("abandoned", true)] {
        let mut by = std::collections::BTreeMap::<usize, [usize; 4]>::new();
        let arg = (&raw mut by).cast();
        // SAFETY: plain calls into the linked mimalloc; the visitor writes only `by`.
        unsafe { if abandoned { mi_heap_visit_abandoned_blocks(mi_heap_main(), false, visit, arg); } else { mi_heap_visit_blocks(mi_heap_main(), false, visit, arg); } }
        let mut rows: Vec<_> = by.into_iter().collect();
        rows.sort_by_key(|(_, [_, _, u, os])| std::cmp::Reverse(os.saturating_sub(*u)));
        let total = rows.iter().fold([0usize; 4], |t, (_, r)| [t[0] + r[0], t[1] + r[1], t[2] + r[2], t[3] + r[3]]);
        out.push_str(&format!("{name} total {} {} {} {}\n", total[0], total[1], total[2], total[3]));
        for (size, [pages, committed, used, os]) in rows.iter().take(30) {
            out.push_str(&format!("{name} size {size} {pages} {committed} {used} {os}\n"));
        }
    }
    unsafe extern "C" {
        fn mi_stats_print_out(out: unsafe extern "C" fn(*const std::ffi::c_char, *mut std::ffi::c_void), arg: *mut std::ffi::c_void);
    }
    unsafe extern "C" fn line(msg: *const std::ffi::c_char, arg: *mut std::ffi::c_void) {
        // SAFETY: mimalloc passes a NUL-terminated message, and `arg` is the string below.
        let (msg, out) = unsafe { (std::ffi::CStr::from_ptr(msg), &mut *arg.cast::<String>()) };
        for l in msg.to_string_lossy().lines() { out.push_str("stats "); out.push_str(l); out.push('\n'); }
    }
    // SAFETY: a plain call into the linked mimalloc; the callback writes only `out`.
    unsafe { mi_stats_print_out(line, (&raw mut out).cast()) };
    out.push_str(&arena_slices());
    out
}

/// Bytes the OS has committed in `[start, start + len)`.
#[cfg(all(feature = "alloc-sites", target_os = "windows"))]
#[allow(unsafe_code)]
fn os_committed(start: usize, len: usize) -> usize {
    #[repr(C)]
    struct Info { base: usize, alloc_base: usize, alloc_protect: u32, partition: u16, size: usize, state: u32, protect: u32, kind: u32 }
    unsafe extern "system" {
        fn VirtualQuery(addr: usize, info: *mut Info, len: usize) -> usize;
    }
    let (mut at, end, mut sum) = (start, start + len, 0);
    while at < end {
        let mut info = Info { base: 0, alloc_base: 0, alloc_protect: 0, partition: 0, size: 0, state: 0, protect: 0, kind: 0 };
        // SAFETY: VirtualQuery only reads the address space and writes `info`.
        if unsafe { VirtualQuery(at, &mut info, std::mem::size_of::<Info>()) } == 0 || info.size == 0 { break; }
        let stop = (info.base + info.size).min(end);
        if info.state == 0x1000 { sum += stop - at; }
        at = stop;
    }
    sum
}

#[cfg(all(feature = "alloc-sites", not(target_os = "windows"), not(target_arch = "wasm32")))]
fn os_committed(_start: usize, _len: usize) -> usize {
    0
}

/// mimalloc's arenas slice by slice (`mi_arenas_print`, one character per 64 KiB slice), counted
/// by kind: `arena-slices <kind> <slices>`, then the map itself as `arena-map` lines. Kinds: p/f/s
/// pages in use (P/F/S not abandoned), i arena info, m metadata, ~ free and purgeable (committed,
/// purge scheduled), _ free and committed, . free and reserved only.
#[cfg(all(feature = "alloc-sites", not(target_arch = "wasm32")))]
#[allow(unsafe_code)]
fn arena_slices() -> String {
    unsafe extern "C" {
        fn mi_register_output(out: Option<unsafe extern "C" fn(*const std::ffi::c_char, *mut std::ffi::c_void)>, arg: *mut std::ffi::c_void);
        fn mi_arenas_print();
    }
    unsafe extern "C" fn take(msg: *const std::ffi::c_char, arg: *mut std::ffi::c_void) {
        // SAFETY: mimalloc passes a NUL-terminated message, and `arg` is the string below.
        let (msg, out) = unsafe { (std::ffi::CStr::from_ptr(msg), &mut *arg.cast::<String>()) };
        out.push_str(&msg.to_string_lossy());
    }
    let mut raw = String::new();
    // SAFETY: plain calls into the linked mimalloc; the output goes to `raw` until it's put back.
    unsafe {
        mi_register_output(Some(take), (&raw mut raw).cast());
        mi_arenas_print();
        mi_register_output(None, std::ptr::null_mut());
    }
    // Drop the colour escapes (ESC [ ... m).
    let mut plain = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\x1B' { for c in chars.by_ref() { if c == 'm' { break; } } continue; }
        plain.push(c);
    }
    let mut counts = std::collections::BTreeMap::<char, usize>::new();
    let mut out = String::new();
    for line in plain.lines() {
        let body = line.trim_start();
        // Chunk rows start with the chunk's index; their slice characters follow it.
        if body.split_whitespace().next().is_some_and(|w| w.chars().all(|c| c.is_ascii_digit())) {
            for c in body.chars().skip_while(|c| c.is_ascii_digit() || *c == ' ') {
                if "pfsPFSim~_.".contains(c) { *counts.entry(c).or_default() += 1; }
            }
        }
        out.push_str("arena-map ");
        out.push_str(line);
        out.push('\n');
    }
    let mut head: String = counts.iter().map(|(k, n)| format!("arena-slices {k} {n}\n")).collect();
    head.push_str(&out);
    head
}
