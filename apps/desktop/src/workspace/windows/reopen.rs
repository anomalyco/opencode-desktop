//! A window closed while others stay, kept so it can open again: Desktop forgets it, tabs,
//! panes and drafts (`registry.closed`). Its rows and frame are copied into one value of the
//! lazy namespace `rune.closedWindows` (out of the boot record) and its drafts into documents of
//! their own (`rune.closedWindows:<id>:draft:<tab>`) before the window's own rows go; Undo on the
//! toast another window shows, or Reopen closed window, writes them back and opens it again where
//! it was (its frame, fitted to the displays as a launch fits it, `placement::fit`).
//!
//! What's kept follows the closed draft's rule (`tabs::DRAFT_DAYS`): 7 days from the close, its
//! images with it, since a draft document's `{ blob: { id } }` is what the blob sweep counts. At
//! most the [`KEEP`] newest windows, holding at most [`BYTES`] of images between them; the oldest
//! goes first when a close passes either, and every launch drops those older than 7 days.

use std::sync::Mutex;

use serde_json::{Map, Value, json};

use super::store::Frame;
use super::{OPEN, Window, open};
use crate::App;

const NAMESPACE: &str = "rune.closedWindows";
const KEEP: usize = 10;
/// The images all kept windows' drafts may hold together.
const BYTES: u64 = 256 << 20;
const DAY_MS: i64 = 24 * 3600 * 1000;

/// Windows whose rows came back on the storage thread, with where they were, for the window
/// that asked to open them.
static BACK: Mutex<Vec<(String, Option<Frame>)>> = Mutex::new(Vec::new());

/// The document a kept window's draft for tab `key` waits in.
fn kept_draft(id: &str, key: &str) -> String {
    format!("{NAMESPACE}:{id}:draft:{key}")
}

/// The bytes of the images a draft document names (`draft_doc::encode`'s `size`).
fn image_bytes(doc: &str) -> u64 {
    let Ok(v) = serde_json::from_str::<Value>(doc) else { return 0 };
    v["files"].as_array().into_iter().flatten().filter(|f| f["blob"].is_object()).filter_map(|f| f["size"].as_u64()).sum()
}

impl App {
    /// This window is closing for good while others stay: what it held, and `frame`, is kept to
    /// reopen, and the newest other window offers it back. False when it held nothing worth
    /// keeping.
    pub(super) fn keep_closed_window(&self, frame: Option<Frame>) -> bool {
        let id = self.window.id.clone();
        if id.is_empty() || self.tabs.is_empty() {
            return false;
        }
        let rows: Map<String, Value> = crate::storage::window_rows(&id).into_iter().map(|(k, v)| (k, Value::String(v.to_string()))).collect();
        let drafts = self.draft_documents();
        let bytes: u64 = drafts.values().map(|d| image_bytes(d)).sum();
        let keys: Vec<String> = drafts.keys().cloned().collect();
        let now = *self.now_ms;
        let snapshot = json!({ "closedAt": now, "tabs": self.tabs.len(), "rows": rows, "drafts": keys, "bytes": bytes, "frame": frame.map(|f| f.to_json()) });
        crate::storage::with_storage(move |s| {
            for (k, d) in drafts {
                s.documents().set(&kept_draft(&id, &k), Some(d));
            }
            let store = s.clone();
            s.state().load(&[NAMESPACE], Some(Box::new(move || {
                store.state().set_json(NAMESPACE, &id, &snapshot);
                let mut index: Vec<String> = store.state().json(NAMESPACE, "index").unwrap_or_default();
                index.retain(|x| *x != id);
                index.insert(0, id);
                trim(&store, index, now);
                store.commit_soon();
            })));
        });
        let front = OPEN.with(|o| o.borrow().iter().filter(|w| w.id != self.window.id && w.life.saved()).max_by_key(|w| w.focused).map(|w| w.app.clone()));
        if let Some(app) = front {
            let id = self.window.id.clone();
            app.update_if(move |s, _| {
                crate::undo::offer(s, opencode_i18n::t!("rune.undo.windowClosed"), crate::undo::Undoable::Window { id });
                true
            });
        }
        true
    }
}

/// Keeps `index` (newest first) within the rule: 7 days, [`KEEP`] windows, [`BYTES`] of images
/// (the newest always stays); what falls out goes, its drafts' documents with it.
fn trim(store: &opencode_storage::Storage, index: Vec<String>, now_ms: i64) {
    let (mut kept, mut bytes) = (Vec::new(), 0u64);
    for id in index {
        let snapshot: Option<Value> = store.state().json(NAMESPACE, &id);
        let fresh = snapshot.as_ref().and_then(|s| s["closedAt"].as_i64()).is_some_and(|at| now_ms - at <= crate::tabs::DRAFT_DAYS * DAY_MS);
        let size = snapshot.as_ref().and_then(|s| s["bytes"].as_u64()).unwrap_or(0);
        if fresh && kept.len() < KEEP && (kept.is_empty() || bytes + size <= BYTES) {
            bytes += size;
            kept.push(id);
            continue;
        }
        for key in snapshot.iter().flat_map(|s| s["drafts"].as_array().cloned().unwrap_or_default()) {
            if let Some(key) = key.as_str() {
                store.documents().set(&kept_draft(&id, key), None);
            }
        }
        store.state().delete(NAMESPACE, &id);
    }
    store.state().set_json(NAMESPACE, "index", &kept);
}

/// Once a launch: drops kept windows closed more than 7 days ago.
pub fn expire(now_ms: i64) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        crate::storage::with_storage(move |s| {
            let store = s.clone();
            s.state().load(&[NAMESPACE], Some(Box::new(move || {
                let index: Vec<String> = store.state().json(NAMESPACE, "index").unwrap_or_default();
                if !index.is_empty() {
                    trim(&store, index, now_ms);
                }
            })));
        });
    });
}

/// Window `id`, closed and kept, opens again: its rows and drafts are written back on the
/// storage thread, then `s`'s next poll opens it where it was ([`poll`]).
pub fn reopen(s: &App, id: &str) {
    let (id, wake) = (id.to_owned(), (*s.waker).clone());
    crate::storage::with_storage(move |st| {
        let store = st.clone();
        st.state().load(&[NAMESPACE], Some(Box::new(move || {
            let Some(snapshot) = store.state().json::<Value>(NAMESPACE, &id) else { return };
            let keys: Vec<String> = snapshot["drafts"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_owned)).collect();
            let docs: Vec<String> = keys.iter().map(|k| kept_draft(&id, k)).collect();
            let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
            let again = store.clone();
            store.documents().load(&refs, Some(Box::new(move || {
                let store = again;
                let ns = opencode_storage::window_namespace(&id);
                for (k, v) in snapshot["rows"].as_object().into_iter().flatten() {
                    if let Some(v) = v.as_str() {
                        store.state().set(&ns, k, v.to_owned());
                    }
                }
                for key in &keys {
                    let kept = kept_draft(&id, key);
                    if let opencode_storage::Lookup::Ready(Some(doc)) = store.documents().get(&kept) {
                        store.documents().set(&opencode_storage::draft_document(&id, key), Some(doc.to_string()));
                    }
                    store.documents().set(&kept, None);
                }
                // A window kept by an earlier build held its drafts in the snapshot itself.
                for (k, v) in snapshot["drafts"].as_object().into_iter().flatten() {
                    store.documents().set(&opencode_storage::draft_document(&id, k), v.as_str().map(str::to_owned));
                }
                store.state().delete(NAMESPACE, &id);
                let mut index: Vec<String> = store.state().json(NAMESPACE, "index").unwrap_or_default();
                index.retain(|x| *x != id);
                store.state().set_json(NAMESPACE, "index", &index);
                store.commit_soon();
                let frame = Frame::from_json(&snapshot["frame"]);
                BACK.lock().unwrap_or_else(|e| e.into_inner()).push((id, frame));
                if let Some(w) = wake {
                    w();
                }
            })));
        })));
    });
}

/// Every poll: windows whose rows came back open, each where it was when it closed (or where
/// the system puts a new window, for one kept without a frame).
pub fn poll(s: &App) {
    let back = std::mem::take(&mut *BACK.lock().unwrap_or_else(|e| e.into_inner()));
    for (id, frame) in back {
        if OPEN.with(|o| o.borrow().iter().any(|w| w.id == id)) {
            continue;
        }
        let _ = open::open(s, Window::new(id, false), frame.and_then(open::refit), false);
    }
}

/// The newest window kept to reopen, for Reopen closed window: read on the storage thread.
pub fn reopen_newest(s: &App) {
    let wake = (*s.waker).clone();
    crate::storage::with_storage(move |st| {
        let store = st.clone();
        st.state().load(&[NAMESPACE], Some(Box::new(move || {
            let index: Vec<String> = store.state().json(NAMESPACE, "index").unwrap_or_default();
            if let Some(id) = index.first() {
                BACK_ASK.lock().unwrap_or_else(|e| e.into_inner()).push(id.clone());
                if let Some(w) = wake {
                    w();
                }
            }
        })));
    });
}

/// Ids [`reopen_newest`] found, to reopen on the next poll.
static BACK_ASK: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Every poll: asks for the newest kept window's rows when Reopen closed window found one.
pub fn poll_asked(s: &App) {
    let asked = std::mem::take(&mut *BACK_ASK.lock().unwrap_or_else(|e| e.into_inner()));
    for id in asked {
        reopen(s, &id);
    }
}
