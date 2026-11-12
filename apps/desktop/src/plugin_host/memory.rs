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
    procwatch::alloc::sites::contribute("luau-vms", || VMS.lock().map(|v| v.iter().map(|(id, bytes)| format!("vm {bytes} {id}\n")).collect()).unwrap_or_default());
    procwatch::alloc::sites::contribute("sqlite", || {
        let (used, high) = opencode_storage::sqlite_memory();
        format!("sqlite {used} {high}\n")
    });
}
