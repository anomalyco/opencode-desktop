//! The shown draft's undo history, kept past a quit, a crash or a kill, as its words already
//! are. Its own document per draft (`<window>:undo:<tab key>`), apart from the draft's, so a
//! keystroke still writes only the draft's words: the history is written when a step closes
//! (undo or redo, a deletion after typing, a paste: the step count moves) and otherwise once the
//! typing has held still for [`SETTLE`]. A kill mid-typing keeps the history as it stood at its
//! last write; the words typed after it come back with the draft, and the first undo takes them
//! back to where that history stood (`TextEdit::restore_history_json`).
//!
//! What's kept: the newest [`STEPS`] steps (and redo), at most [`BYTES`] of document between
//! them, for as long as the draft is kept. A draft that's sent, emptied or dropped takes its
//! history with it. A closed tab or window keeps its draft's words for reopening, and its
//! history only for the rest of the run.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use rune::Tx;

use crate::App;
use opencode_kit::servers::TabKey;
use opencode_storage::Lookup;

/// How many undo steps are kept per draft.
const STEPS: usize = 32;
/// How much document the kept steps may hold between them (each is the text before and after).
const BYTES: usize = 64 * 1024;
/// How long typing holds still before the history is written.
const SETTLE: f64 = 1.0;

/// What each draft's history was last written as, and the change waiting to hold still.
#[derive(Default)]
pub struct Kept {
    /// Drafts whose stored history was taken back this run (or that had none).
    restored: HashSet<String>,
    /// Drafts whose stored history was asked of the disk.
    asked: HashSet<String>,
    /// Per draft: the step counts and the print of what was last written.
    written: HashMap<String, ((usize, usize), u64)>,
    /// The shown draft's change waiting to hold still, and since when.
    candidate: Option<(String, u64, f64)>,
    /// What the shown draft's print was last taken from (`print`).
    print: Option<Print>,
    /// Histories a worker turned into JSON, waiting to be stored: each with the write it was
    /// (`sent`), so one a draft dropped since isn't stored after all.
    done: Arc<Mutex<Vec<(String, u64, Option<String>)>>>,
    /// Per undo document: the newest write sent to a worker and not stored yet.
    sent: HashMap<String, u64>,
    writes: u64,
}

/// The shown draft as its print was last taken: a new print only when its step counts, its
/// selection or its words differ. The field's `Tracked` version says when to look at all, so a
/// frame that wrote nothing to it compares nothing, and one that did compares its words with
/// this copy instead of hashing them all.
struct Print {
    key: String,
    version: u64,
    len: (usize, usize),
    range: (usize, usize),
    text: String,
    id: u64,
}

impl App {
    /// Each poll: the shown draft's stored history is taken back once, and its changes are
    /// written when a step closes or the typing holds still.
    pub(crate) fn poll_undo(&mut self, now: f64, tx: &mut Tx) {
        self.store_done_histories();
        if !self.persists() || self.window.id.is_empty() || self.composer.is_secure() {
            return;
        }
        let Some(key) = self.restore.shown.clone() else { return };
        let doc = undo_document(&self.window.id, &self.draft_tab_key(&key));
        if !self.restore.undo.restored.contains(&key) && !self.take_back_history(&key, &doc) {
            return;
        }
        let len = self.composer.history_len();
        let print = self.print(&key);
        // A draft with nothing stored has written an empty history.
        let (written_len, written_print) = self.restore.undo.written.get(&key).copied().unwrap_or(((0, 0), 0));
        let waiting = self.restore.undo.candidate.as_ref().filter(|(k, p, _)| *k == key && *p == print).map(|c| c.2);
        match () {
            _ if written_print == print || (len == (0, 0) && written_len == (0, 0)) => {
                // Written only when it holds one: a poll that changes nothing writes nothing.
                if self.restore.undo.candidate.is_some() {
                    self.restore.undo.candidate = None;
                }
                return;
            }
            // A step closed (or undo, redo): written now.
            _ if written_len != len => {}
            _ => match waiting {
                Some(at) if now - at + 1e-9 >= SETTLE => {}
                Some(at) => return tx.wake_at(at + SETTLE),
                None => {
                    self.restore.undo.candidate = Some((key, print, now));
                    return tx.wake_at(now + SETTLE);
                }
            },
        }
        // The JSON (the document, and the newest step however large) is made on a worker: a
        // 1.7 MB paste's history took 2-3 ms of the frame that closed its step. The field's copy
        // shares its steps, so only its words are copied here.
        let edit = (*self.composer).clone();
        let kept = &mut self.restore.undo;
        kept.writes += 1;
        let (write, done, waker) = (kept.writes, kept.done.clone(), (*self.waker).clone());
        kept.sent.insert(doc.clone(), write);
        kept.written.insert(key, (len, print));
        kept.candidate = None;
        crate::plugin_host::tasks::run(move || {
            let value = edit.history_json(STEPS, BYTES).map(|v| v.to_string());
            if let Ok(mut d) = done.lock() {
                d.push((doc, write, value));
            }
            if let Some(w) = waker {
                w();
            }
        });
    }

    /// Stores the histories workers finished, each unless its draft was dropped or written again
    /// since it was sent.
    fn store_done_histories(&mut self) {
        let done = std::mem::take(&mut *self.restore.undo.done.lock().unwrap_or_else(|e| e.into_inner()));
        if done.is_empty() {
            return;
        }
        let kept = &mut self.restore.undo;
        for (doc, write, value) in done {
            if kept.sent.get(&doc) == Some(&write) {
                kept.sent.remove(&doc);
                crate::storage::set_doc(&doc, value);
            }
        }
    }

    /// The shown draft's print: the same while its step counts, selection and words are.
    fn print(&mut self, key: &str) -> u64 {
        let version = rune::Tracked::version(&self.composer);
        let (len, range) = (self.composer.history_len(), self.composer.range());
        match &self.restore.undo.print {
            Some(p) if p.key == key && p.version == version => return p.id,
            Some(p) if p.key == key && p.len == len && p.range == range && p.text == self.composer.text => {
                let id = p.id;
                if let Some(p) = &mut self.restore.undo.print {
                    p.version = version;
                }
                return id;
            }
            _ => {}
        }
        let kept = &mut self.restore.undo;
        kept.writes += 1;
        let id = kept.writes;
        let text = self.composer.text.clone();
        kept.print = Some(Print { key: key.to_owned(), version, len, range, text, id });
        id
    }

    /// The shown draft's stored history, into its field once it's read; false while it's still
    /// on its way from the disk.
    fn take_back_history(&mut self, key: &str, doc: &str) -> bool {
        let stored = match crate::storage::doc(doc) {
            Lookup::Ready(text) => text,
            Lookup::Pending => {
                // Asked of the disk once the storage is open (before, a load goes nowhere).
                if crate::storage::is_ready()
                    && self.restore.undo.asked.insert(key.to_owned())
                    && let Some(w) = (*self.waker).clone()
                {
                    crate::storage::load_docs(&[doc], Box::new(move || w()));
                }
                return false;
            }
        };
        // Only into a field that has no history of its own yet this run.
        if let Some(v) = stored.as_deref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            && self.composer.history_len() == (0, 0)
        {
            self.composer.restore_history_json(&v);
        }
        self.restore.undo.restored.insert(key.to_owned());
        if stored.is_some() {
            let print = self.print(key);
            self.restore.undo.written.insert(key.to_owned(), (self.composer.history_len(), print));
        }
        true
    }

    /// Draft `tab`'s history goes with it.
    pub(super) fn forget_undo(&mut self, tab: &TabKey) {
        if !self.window.id.is_empty() {
            let doc = undo_document(&self.window.id, tab);
            // A write a worker still holds isn't stored after this.
            self.restore.undo.sent.remove(&doc);
            crate::storage::set_doc(&doc, None);
        }
        let encoded = tab.encode();
        let gone: Vec<String> = self.restore.undo.written.keys().filter(|k| self.draft_tab_key(k).encode() == encoded).cloned().collect();
        for k in gone {
            self.restore.undo.written.remove(&k);
            self.restore.undo.restored.remove(&k);
        }
    }
}

/// The document window `window`'s draft for `tab` keeps its undo history in.
fn undo_document(window: &str, tab: &TabKey) -> String {
    format!("{}:undo:{}", opencode_storage::window_namespace(window), tab.encode())
}
