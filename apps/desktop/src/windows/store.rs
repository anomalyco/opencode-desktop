//! What each window keeps across launches, in the rows Desktop keeps it in
//! (`packages/desktop/src/main/windows`, `storage/state.ts`): the open windows' ids (`windowIds`
//! in `opencode.settings`), each window's tabs, page, closed tabs and panes (one row per key of
//! `opencode.window.<id>.dat`), each draft its own document, and the window's frame
//! (`window_state`). All of it lives in the app's database (`crate::storage`).

use opencode_kit::servers::TabKey;
use opencode_storage::Lookup;
use serde_json::{Map, Value};

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

/// The document window `w` keeps tab `key`'s draft in.
pub fn draft_key(w: &Window, key: &TabKey) -> String {
    opencode_storage::draft_document(&w.id, &key.encode())
}

/// What window `w` saved, in Desktop's mixed-server shape (`opencode_kit::servers::tabs::WindowDoc`):
/// every row of its namespace, and each draft it has in memory under `drafts` (before the
/// storage is ready, only the one it shows; [`drafts`] brings the rest).
pub fn load(w: &Window) -> Value {
    if w.id.is_empty() {
        return Value::Object(Map::new());
    }
    let mut doc: Map<String, Value> = crate::storage::window_rows(&w.id).into_iter().filter_map(|(k, v)| Some((k, serde_json::from_str(&v).ok()?))).collect();
    let mut drafts = Map::new();
    let shown = doc.get("tabs.recent").and_then(|r| r["key"].as_str()).and_then(TabKey::parse);
    for key in shown.into_iter() {
        if let Lookup::Ready(Some(text)) = crate::storage::doc(&draft_key(w, &key))
            && let Ok(v) = serde_json::from_str(&text)
        {
            drafts.insert(key.encode(), v);
        }
    }
    doc.insert("drafts".into(), Value::Object(drafts));
    Value::Object(doc)
}

/// Window `w`'s drafts the storage holds: `(tab key, draft)` for each in memory, and the
/// documents still on disk, to load ([`crate::storage::load_docs`]).
pub fn drafts(w: &Window) -> (Vec<(TabKey, Value)>, Vec<String>) {
    let prefix = format!("{}:draft:", namespace(w));
    let (mut ready, mut pending) = (Vec::new(), Vec::new());
    for doc in crate::storage::doc_keys(&prefix) {
        let Some(key) = doc.strip_prefix(&prefix).and_then(TabKey::parse) else { continue };
        match crate::storage::doc(&doc) {
            Lookup::Ready(Some(text)) => ready.extend(serde_json::from_str(&text).ok().map(|v| (key, v))),
            Lookup::Ready(None) => {}
            Lookup::Pending => pending.push(doc),
        }
    }
    (ready, pending)
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

/// Sets (`Some`) or removes tab `key`'s draft in window `w`.
pub fn save_draft(w: &Window, key: &TabKey, draft: Option<String>) {
    if !w.id.is_empty() {
        crate::storage::set_doc(&draft_key(w, key), draft);
    }
}

/// Window `id` closed while others stay open: its rows go (Desktop's `registry.closed`).
pub fn forget(id: &str) {
    #[cfg(not(target_arch="wasm32"))]
    crate::restore::undo::forget_closing(id);
    crate::storage::forget_window(id);
}

pub fn load_frame(id: &str) -> Option<Frame> {
    crate::storage::frame(id)
}

/// Written behind, as the window settles after a move and as it closes.
pub fn save_frame(id: &str, frame: Frame) {
    crate::storage::set_frame(id, frame);
}
