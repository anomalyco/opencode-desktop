//! The app process's memory for plugins (`TEXT_MEMORY`): what the OS counts and what the app's
//! counting allocator holds, which every plugin library allocates on too.

use serde_json::{Value, json};

/// The report as JSON (see `opencode_plugin::abi::TEXT_MEMORY`).
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
