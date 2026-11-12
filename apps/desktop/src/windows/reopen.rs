//! A window closed while others stay, kept so it can open again: Desktop forgets it, tabs,
//! panes and drafts (`registry.closed`). Its rows and its drafts' documents are copied into one
//! value of the lazy namespace `rune.closedWindows` (out of the boot record) before the window's
//! own rows go, the ten newest kept; Undo on the toast another window shows, or Reopen closed
//! window, writes them back and opens it again. A draft's images are named by blob, which the
//! blob sweep keeps only 15 minutes after nothing else names them: a window reopened later comes
//! back with its words and tabs, its drafts' images gone.

use std::sync::Mutex;

use serde_json::{Map, Value, json};

use super::{OPEN, Window, open};
use crate::App;

const NAMESPACE: &str = "rune.closedWindows";
const KEEP: usize = 10;

/// Windows whose rows came back on the storage thread, for the window that asked to open them.
static BACK: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl App {
    /// This window is closing for good while others stay: what it held is kept to reopen, and
    /// the newest other window offers it back. False when it held nothing worth keeping.
    pub(super) fn keep_closed_window(&self) -> bool {
        let id = self.window.id.clone();
        if id.is_empty() || self.tabs.is_empty() {
            return false;
        }
        let rows: Map<String, Value> = crate::storage::window_rows(&id).into_iter().map(|(k, v)| (k, Value::String(v.to_string()))).collect();
        let drafts: Map<String, Value> = self.draft_documents().into_iter().map(|(k, v)| (k, Value::String(v))).collect();
        let snapshot = json!({ "closedAt": *self.now_ms, "tabs": self.tabs.len(), "rows": rows, "drafts": drafts });
        crate::storage::with_storage(move |s| {
            let store = s.clone();
            s.state().load(&[NAMESPACE], Some(Box::new(move || {
                store.state().set_json(NAMESPACE, &id, &snapshot);
                let mut index: Vec<String> = store.state().json(NAMESPACE, "index").unwrap_or_default();
                index.retain(|x| *x != id);
                index.insert(0, id);
                for gone in index.split_off(index.len().min(KEEP)) {
                    store.state().delete(NAMESPACE, &gone);
                }
                store.state().set_json(NAMESPACE, "index", &index);
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

/// Window `id`, closed and kept, opens again: its rows are written back on the storage thread,
/// then `s`'s next poll opens it ([`poll`]).
pub fn reopen(s: &App, id: &str) {
    let (id, wake) = (id.to_owned(), (*s.waker).clone());
    crate::storage::with_storage(move |st| {
        let store = st.clone();
        st.state().load(&[NAMESPACE], Some(Box::new(move || {
            let Some(snapshot) = store.state().json::<Value>(NAMESPACE, &id) else { return };
            let ns = opencode_storage::window_namespace(&id);
            for (k, v) in snapshot["rows"].as_object().into_iter().flatten() {
                if let Some(v) = v.as_str() {
                    store.state().set(&ns, k, v.to_owned());
                }
            }
            for (k, v) in snapshot["drafts"].as_object().into_iter().flatten() {
                store.documents().set(&opencode_storage::draft_document(&id, k), v.as_str().map(str::to_owned));
            }
            store.state().delete(NAMESPACE, &id);
            let mut index: Vec<String> = store.state().json(NAMESPACE, "index").unwrap_or_default();
            index.retain(|x| *x != id);
            store.state().set_json(NAMESPACE, "index", &index);
            BACK.lock().unwrap_or_else(|e| e.into_inner()).push(id);
            if let Some(w) = wake {
                w();
            }
        })));
    });
}

/// Every poll: windows whose rows came back open, each where the system puts a new window.
pub fn poll(s: &App) {
    let back = std::mem::take(&mut *BACK.lock().unwrap_or_else(|e| e.into_inner()));
    for id in back {
        if OPEN.with(|o| o.borrow().iter().any(|w| w.id == id)) {
            continue;
        }
        let _ = open::open(s, Window::new(id, false), None, false);
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
