//! What each window keeps across launches, in the rows Desktop keeps it in
//! (`packages/desktop/src/main/windows`, `storage/state.ts`): the open windows' ids (`windowIds`
//! in `opencode.settings`), each window's tabs, page, closed tabs and panes (one row per key of
//! `opencode.window.<id>.dat`), each draft its own document, and the window's frame
//! (`window_state`). All of it lives in the app's database (`crate::storage`).

use serde_json::Value;

use super::Window;

pub use opencode_storage::WindowFrame as Frame;

/// The window ids saved at the last quit, in the order they opened; none when no list was
/// ever saved (a fresh profile).
pub fn saved_ids() -> Option<Vec<String>> {
    crate::storage::window_ids()
}

pub fn save_ids(ids: &[String]) {
    crate::storage::save_window_ids(ids);
}

/// A new window's id (Desktop's `randomUUID`).
pub fn new_id() -> String {
    let id = opencode_kit::client::session_id();
    id.strip_prefix("ses_").unwrap_or(&id).to_owned()
}

/// Window `w`'s namespace (`opencode.window.<id>.dat`).
fn namespace(w: &Window) -> String {
    opencode_storage::window_namespace(&w.id)
}

/// Sets `key` of window `w`'s namespace (`Null` removes it). An unchanged value writes nothing.
pub fn save(w: &Window, key: &str, value: &Value) {
    if w.id.is_empty() {
        return;
    }
    match value {
        Value::Null => crate::storage::delete(&namespace(w), key),
        v => crate::storage::set_json(&namespace(w), key, v),
    }
}

/// Window `id` closed while others stay open: its rows go (Desktop's `registry.closed`).
pub fn forget(id: &str) {
    crate::storage::forget_window(id);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_frame(id: &str) -> Option<Frame> {
    crate::storage::frame(id)
}

/// The key of a window's layout reads in its namespace.
const LAYOUT: &str = "rune.layout";

/// What window `id`'s view last read from layout (`rune::App::layout_seed`): its next launch's
/// first view is given these rects, so the first frame builds the view once, not twice.
/// `OPENCODE_RUNE_LAYOUT_SEED=off` gives it none (a bench's arm).
pub fn load_layout(id: &str) -> Option<rune::LayoutSeed> {
    if id.is_empty() || rune::env::var("OPENCODE_RUNE_LAYOUT_SEED").is_some_and(|v| v == "off") {
        return None;
    }
    let v = crate::storage::json(&opencode_storage::window_namespace(id), LAYOUT)?;
    let floats = |v: &Value| v.as_array()?.iter().map(|n| n.as_f64().map(|n| n as f32)).collect::<Option<Vec<f32>>>();
    let &[w, h, scale, zoom] = floats(v.get("view")?)?.as_slice() else { return None };
    let rects = v
        .get("rects")?
        .as_object()?
        .iter()
        .map(|(key, rect)| {
            if rect.is_null() {
                return Some((key.clone(), None));
            }
            let &[x, y, w, h] = floats(rect)?.as_slice() else { return None };
            Some((key.clone(), Some([x, y, w, h])))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(rune::LayoutSeed { viewport: [w, h], scale, zoom, rects })
}

/// Rune hands them over as the window rests, closes or quits, when they changed.
pub fn save_layout(id: &str, seed: &rune::LayoutSeed) {
    if id.is_empty() {
        return;
    }
    let rects: serde_json::Map<String, Value> = seed.rects.iter().map(|(key, rect)| (key.clone(), rect.map_or(Value::Null, |r| serde_json::json!(r)))).collect();
    let value = serde_json::json!({ "view": [seed.viewport[0], seed.viewport[1], seed.scale, seed.zoom], "rects": rects });
    crate::storage::set_json(&opencode_storage::window_namespace(id), LAYOUT, &value);
}

/// Written behind, as the window settles after a move and as it closes.
pub fn save_frame(id: &str, frame: Frame) {
    crate::storage::set_frame(id, frame);
}

#[cfg(target_os = "linux")]
const APPEARANCE: &str = "rune.appearance";

/// The desktop's light or dark and reduced motion as the last run saw them
/// (`rune::last_appearance`). Linux asks the desktop portal for them over D-Bus, and a window's
/// first frame draws with these instead of waiting for its answer (a portal that never
/// answered held the first frame for 100 ms).
#[cfg(target_os = "linux")]
pub fn load_appearance() -> Option<rune::Appearance> {
    let v = crate::storage::json("opencode.global.dat", APPEARANCE)?;
    Some(rune::Appearance { dark: v.get("dark")?.as_bool(), reduced_motion: v.get("reducedMotion")?.as_bool()? })
}

/// Rune hands over each answer of the portal's that isn't what it last had, on its own thread.
#[cfg(target_os = "linux")]
pub fn save_appearance(seen: rune::Appearance) {
    crate::storage::set_json("opencode.global.dat", APPEARANCE, &serde_json::json!({ "dark": seen.dark, "reducedMotion": seen.reduced_motion }));
}
