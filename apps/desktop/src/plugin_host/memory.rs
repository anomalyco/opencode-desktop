//! The app process's memory for `Diagnostics:Memory`: what the OS counts and what the app's
//! counting allocator holds.

use serde_json::{Value, json};

/// The report as JSON: `footprint`, `footprintPeak`, `resident` and `heap`.
pub fn report() -> String {
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

/// What each plugin's Luau VMs hold, as of the last poll, for a measuring build's heap dumps.
#[cfg(feature = "alloc-sites")]
static VMS: std::sync::Mutex<Vec<(String, usize)>> = std::sync::Mutex::new(Vec::new());

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
#[cfg(feature = "alloc-sites")]
pub fn register_reports() {
    opencode_storage::count_sqlite_memory();
    procwatch::alloc::sites::contribute("luau-vms", || VMS.lock().map(|v| v.iter().map(|(id, bytes)| format!("vm {bytes} {id}\n")).collect()).unwrap_or_default());
    procwatch::alloc::sites::contribute("sqlite", || {
        let (used, high) = opencode_storage::sqlite_memory();
        format!("sqlite {used} {high}\n")
    });
    procwatch::alloc::sites::contribute("mimalloc-pages", mimalloc_pages);
}

/// mimalloc's pages by block size: `size <block> <pages> <committed> <live bytes>`, the slack
/// (committed less live) largest first, then `total` and the same for abandoned pages (a thread
/// that ended; seen only with `MIMALLOC_VISIT_ABANDONED=1`). Read from the dump thread while
/// others allocate, so a measure, not an exact count.
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
        let (area, by) = unsafe { (&*area, &mut *arg.cast::<std::collections::BTreeMap<usize, [usize; 3]>>()) };
        if block.is_null() {
            let e = by.entry(area.block_size).or_default();
            e[0] += 1;
            e[1] += area.committed;
            e[2] += area.used * area.block_size;
        }
        true
    }
    let mut out = String::new();
    for (name, abandoned) in [("live", false), ("abandoned", true)] {
        let mut by = std::collections::BTreeMap::<usize, [usize; 3]>::new();
        let arg = (&raw mut by).cast();
        // SAFETY: plain calls into the linked mimalloc; the visitor writes only `by`.
        unsafe { if abandoned { mi_heap_visit_abandoned_blocks(mi_heap_main(), false, visit, arg); } else { mi_heap_visit_blocks(mi_heap_main(), false, visit, arg); } }
        let mut rows: Vec<_> = by.into_iter().collect();
        rows.sort_by_key(|(_, [_, c, u])| std::cmp::Reverse(c.saturating_sub(*u)));
        let total = rows.iter().fold([0usize; 3], |t, (_, r)| [t[0] + r[0], t[1] + r[1], t[2] + r[2]]);
        out.push_str(&format!("{name} total {} {} {}\n", total[0], total[1], total[2]));
        for (size, [pages, committed, used]) in rows.iter().take(30) {
            out.push_str(&format!("{name} size {size} {pages} {committed} {used}\n"));
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
    out
}
