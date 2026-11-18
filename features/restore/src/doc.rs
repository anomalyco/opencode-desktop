//! The window's rows, read once as the window opens, in Desktop's mixed-server shape. A window
//! from before that shape was migrated once, as the database took the files in
//! (`opencode_storage::native::import`, MS-1's `migration::window`).

use opencode_kit::servers::TabKey;
use opencode_kit::servers::tabs::WindowDoc;
use opencode_storage::Lookup;
use serde_json::{Map, Value};

use crate::RestoreHost;

/// What window `host` saved, in Desktop's mixed-server shape (`WindowDoc`): every row of its
/// namespace, and the shown draft if it is already in memory (before storage is ready, only the
/// shown one; [`window_drafts`] brings the rest).
pub fn load_window_raw(host: &dyn RestoreHost) -> Value {
    let window_id = host.window_id();
    if window_id.is_empty() {
        return Value::Object(Map::new());
    }
    let mut doc: Map<String, Value> = host
        .storage_window_rows(window_id)
        .into_iter()
        .filter_map(|(k, v)| Some((k, serde_json::from_str(&v).ok()?)))
        .collect();
    let mut drafts = Map::new();
    if let Some(key) = doc.get("tabs.recent").and_then(|r| r["key"].as_str()).and_then(TabKey::parse)
        && let Lookup::Ready(Some(text)) = host.storage_doc(&opencode_storage::draft_document(window_id, &key.encode()))
        && let Ok(v) = serde_json::from_str(&text)
    {
        drafts.insert(key.encode(), v);
    }
    doc.insert("drafts".into(), Value::Object(drafts));
    Value::Object(doc)
}

/// Window `host`'s saved tabs, page, shown draft and panes (its closed tabs and other drafts come
/// in once the storage is ready, [`crate::hydrate_restore`]).
pub fn read_window(host: &dyn RestoreHost) -> WindowDoc {
    WindowDoc::from_json(&load_window_raw(host))
}

/// Decodes a [`WindowDoc`] directly from a raw window-namespace JSON object.
pub fn read_window_json(raw: &Value) -> WindowDoc {
    WindowDoc::from_json(raw)
}

/// Window `host`'s drafts the storage holds: `(tab key, draft)` for each in memory, and the
/// document keys still on disk to load via [`RestoreHost::storage_load_docs`].
pub fn window_drafts(host: &dyn RestoreHost) -> (Vec<(TabKey, Value)>, Vec<String>) {
    let prefix = format!("{}:draft:", opencode_storage::window_namespace(host.window_id()));
    let (mut ready, mut pending) = (Vec::new(), Vec::new());
    for doc in host.storage_doc_keys(&prefix) {
        let Some(key) = doc.strip_prefix(&prefix).and_then(TabKey::parse) else { continue };
        match host.storage_doc(&doc) {
            Lookup::Ready(Some(text)) => ready.extend(serde_json::from_str(&text).ok().map(|v| (key, v))),
            Lookup::Ready(None) => {}
            Lookup::Pending => pending.push(doc),
        }
    }
    (ready, pending)
}

/// Sets `key` of window `host`'s namespace (`Null` removes it). An unchanged value writes nothing.
pub fn save_window_key(host: &mut dyn RestoreHost, key: &str, value: &Value) {
    let window_id = host.window_id();
    if window_id.is_empty() {
        return;
    }
    let ns = opencode_storage::window_namespace(window_id);
    match value {
        Value::Null => host.storage_delete(&ns, key),
        v => host.storage_set_json(&ns, key, v),
    }
}

/// Sets (`Some`) or removes (`None`) tab `key`'s draft in window `host`.
pub fn save_window_draft(host: &mut dyn RestoreHost, key: &TabKey, draft: Option<String>) {
    let window_id = host.window_id();
    if !window_id.is_empty() {
        let doc = opencode_storage::draft_document(window_id, &key.encode());
        host.storage_set_doc(&doc, draft);
    }
}
