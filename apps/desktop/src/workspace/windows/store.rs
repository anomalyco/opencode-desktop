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

/// Written behind, as the window settles after a move and as it closes.
pub fn save_frame(id: &str, frame: Frame) {
    crate::storage::set_frame(id, frame);
}
