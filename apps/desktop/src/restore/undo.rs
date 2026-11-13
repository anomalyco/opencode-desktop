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

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

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
    /// Per draft: the step counts and the fingerprint of what was last written.
    written: HashMap<String, ((usize, usize), u64)>,
    /// The shown draft's change waiting to hold still, and since when.
    candidate: Option<(String, u64, f64)>,
}

impl App {
    /// Each poll: the shown draft's stored history is taken back once, and its changes are
    /// written when a step closes or the typing holds still.
    pub(crate) fn poll_undo(&mut self, now: f64, tx: &mut Tx) {
        if !self.persists() || self.window.id.is_empty() || self.composer.is_secure() {
            return;
        }
        let Some(key) = self.restore.shown.clone() else { return };
        let doc = undo_document(&self.window.id, &self.draft_tab_key(&key));
        if !self.restore.undo.restored.contains(&key) && !self.take_back_history(&key, &doc) {
            return;
        }
        let len = self.composer.history_len();
        let print = fingerprint(&self.composer);
        let kept = &mut self.restore.undo;
        // A draft with nothing stored has written an empty history.
        let (written_len, written_print) = kept.written.get(&key).copied().unwrap_or(((0, 0), 0));
        match () {
            _ if written_print == print || (len == (0, 0) && written_len == (0, 0)) => {
                kept.candidate = None;
                return;
            }
            // A step closed (or undo, redo): written now.
            _ if written_len != len => {}
            _ => match &kept.candidate {
                Some((k, p, at)) if *k == key && *p == print && now - at + 1e-9 >= SETTLE => {}
                Some((k, p, at)) if *k == key && *p == print => return tx.wake_at(at + SETTLE),
                _ => {
                    kept.candidate = Some((key, print, now));
                    return tx.wake_at(now + SETTLE);
                }
            },
        }
        let value = self.composer.history_json(STEPS, BYTES).map(|v| v.to_string());
        crate::storage::set_doc(&doc, value);
        let kept = &mut self.restore.undo;
        kept.written.insert(key, (len, print));
        kept.candidate = None;
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
        let kept = &mut self.restore.undo;
        kept.restored.insert(key.to_owned());
        if stored.is_some() {
            kept.written.insert(key.to_owned(), (self.composer.history_len(), fingerprint(&self.composer)));
        }
        true
    }

    /// Draft `tab`'s history goes with it.
    pub(super) fn forget_undo(&mut self, tab: &TabKey) {
        if !self.window.id.is_empty() {
            crate::storage::set_doc(&undo_document(&self.window.id, tab), None);
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

/// What the stored history depends on: its step counts, and the document it leads to.
fn fingerprint(edit: &rune::TextEdit) -> u64 {
    let mut h = DefaultHasher::new();
    (edit.history_len(), &edit.text, edit.range()).hash(&mut h);
    h.finish()
}
