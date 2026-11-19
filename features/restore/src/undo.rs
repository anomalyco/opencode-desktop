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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use opencode_kit::servers::TabKey;
use opencode_storage::Lookup;
use rune::Tx;

use crate::RestoreHost;
use crate::places::{Settle, SettleEffect, SettleEvent};

/// How many undo steps are kept per draft.
const STEPS: usize = 32;
/// How much document the kept steps may hold between them (each is the text before and after).
const BYTES: usize = 64 * 1024;
/// How long typing holds still before the history is written.
const SETTLE: f64 = 1.0;

type DoneHistory = (String, u64, Option<String>);

/// Per-draft undo-history load and persistence state.
/// Absence from [`Kept::drafts`] means the draft's stored history has not been asked of disk yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Draft {
    /// Disk load requested; waiting for storage to populate the document.
    Asked,
    /// Restored for this run with nothing written yet (equivalent to an empty history).
    Empty,
    /// Restored and synced with the last write's step counts and print id.
    Written { len: (usize, usize), print: u64 },
}

/// What each draft's history was last written as, and the change waiting to hold still.
#[derive(Default)]
pub struct Kept {
    /// Per draft: load/restore status and the last written step counts and print.
    drafts: HashMap<String, Draft>,
    /// The shown draft's change waiting to hold still, and since when.
    candidate: Settle<(String, u64)>,
    /// What the shown draft's print was last taken from (`print`).
    print: Option<Print>,
    /// Histories a worker turned into JSON, waiting to be stored: each with the write it was
    /// (`sent`), so one a draft dropped since isn't stored after all.
    done: Arc<Mutex<Vec<DoneHistory>>>,
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

/// Each poll: the shown draft's stored history is taken back once, and its changes are
/// written when a step closes or the typing holds still.
pub fn poll_undo(host: &mut dyn RestoreHost, now: f64, tx: &mut Tx) {
    store_done_histories(host);
    if !host.persists() || host.window_id().is_empty() || host.composer().is_secure() {
        return;
    }
    let Some(key) = host.restore().shown.clone() else { return };
    let doc = undo_document(host.window_id(), &crate::draft_tab_key(host, &key));
    let restored = matches!(
        host.restore().undo.drafts.get(&key),
        Some(Draft::Empty | Draft::Written { .. })
    );
    if !restored && !take_back_history(host, &key, &doc) {
        return;
    }
    let len = host.composer().history_len();
    let p = print(host, &key);
    let (unchanged, step_closed) = match host.restore().undo.drafts.get(&key).copied() {
        Some(Draft::Written { len: written_len, print: written_print }) => {
            (written_print == p || (len == (0, 0) && written_len == (0, 0)), written_len != len)
        }
        // A draft with nothing stored has written an empty history.
        Some(Draft::Empty | Draft::Asked) | None => (len == (0, 0), len != (0, 0)),
    };
    if unchanged {
        // Written only when it holds one: a poll that changes nothing writes nothing.
        if !host.restore().undo.candidate.is_idle() {
            host.restore_mut().undo.candidate.step(SettleEvent::Matched);
        }
        return;
    }
    // A step closed (or undo, redo): written now; otherwise wait for typing to hold still.
    if !step_closed {
        if let Some(at) = host.restore().undo.candidate.wake_if(|(k, pr)| *k == key && *pr == p, now, SETTLE) {
            return tx.wake_at(at);
        }
        match host.restore_mut().undo.candidate.step(SettleEvent::Changed { value: (key.clone(), p), now, delay: SETTLE }) {
            SettleEffect::Ready(_) => {}
            SettleEffect::WakeAt(at) => return tx.wake_at(at),
            SettleEffect::None => return,
        }
    }
    // The JSON (the document, and the newest step however large) is made on a worker: a
    // 1.7 MB paste's history took 2-3 ms of the frame that closed its step. The field's copy
    // shares its steps, so only its words are copied here.
    let edit = host.composer().clone();
    let waker = host.waker();
    let (write, done) = {
        let kept = &mut host.restore_mut().undo;
        kept.writes += 1;
        let write = kept.writes;
        kept.sent.insert(doc.clone(), write);
        kept.drafts.insert(key, Draft::Written { len, print: p });
        kept.candidate.step(SettleEvent::Matched);
        (write, kept.done.clone())
    };
    host.run_background(Box::new(move || {
        let value = edit.history_json(STEPS, BYTES).map(|v| v.to_string());
        if let Ok(mut d) = done.lock() {
            d.push((doc, write, value));
        }
        if let Some(w) = waker {
            w();
        }
    }));
}

/// Stores the histories workers finished, each unless its draft was dropped or written again
/// since it was sent.
fn store_done_histories(host: &mut dyn RestoreHost) {
    let done = std::mem::take(&mut *host.restore().undo.done.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    if done.is_empty() {
        return;
    }
    for (doc, write, value) in done {
        if host.restore().undo.sent.get(&doc) == Some(&write) {
            host.restore_mut().undo.sent.remove(&doc);
            host.storage_set_doc(&doc, value);
        }
    }
}

/// The shown draft's print: the same while its step counts, selection and words are.
fn print(host: &mut dyn RestoreHost, key: &str) -> u64 {
    let version = host.composer_version();
    let (len, range) = (host.composer().history_len(), host.composer().range());
    let hit = match &host.restore().undo.print {
        Some(p) if p.key == key && p.version == version => Some((p.id, false)),
        Some(p) if p.key == key && p.len == len && p.range == range && p.text == host.composer().text => Some((p.id, true)),
        _ => None,
    };
    if let Some((id, bump)) = hit {
        if bump && let Some(p) = &mut host.restore_mut().undo.print {
            p.version = version;
        }
        return id;
    }
    let text = host.composer().text.clone();
    let kept = &mut host.restore_mut().undo;
    kept.writes += 1;
    let id = kept.writes;
    kept.print = Some(Print { key: key.to_owned(), version, len, range, text, id });
    id
}

/// The shown draft's stored history, into its field once it's read; false while it's still
/// on its way from the disk.
fn take_back_history(host: &mut dyn RestoreHost, key: &str, doc: &str) -> bool {
    let stored = match host.storage_doc(doc) {
        Lookup::Ready(text) => text,
        Lookup::Pending => {
            // Asked of the disk once the storage is open (before, a load goes nowhere).
            if host.storage_is_ready()
                && !matches!(host.restore().undo.drafts.get(key), Some(Draft::Asked))
                && let Some(w) = host.waker()
            {
                host.restore_mut().undo.drafts.insert(key.to_owned(), Draft::Asked);
                host.storage_load_docs(&[doc], Box::new(move || w()));
            }
            return false;
        }
    };
    // Only into a field that has no history of its own yet this run.
    if let Some(v) = stored.as_deref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
        && host.composer().history_len() == (0, 0)
    {
        host.composer_mut().restore_history_json(&v);
    }
    let state = if stored.is_some() {
        let p = print(host, key);
        let len = host.composer().history_len();
        Draft::Written { len, print: p }
    } else {
        Draft::Empty
    };
    host.restore_mut().undo.drafts.insert(key.to_owned(), state);
    true
}

/// Draft `tab`'s history goes with it.
pub(crate) fn forget_undo(host: &mut dyn RestoreHost, tab: &TabKey) {
    let window_id = host.window_id();
    if !window_id.is_empty() {
        let doc = undo_document(window_id, tab);
        // A write a worker still holds isn't stored after this.
        host.restore_mut().undo.sent.remove(&doc);
        host.storage_set_doc(&doc, None);
    }
    let encoded = tab.encode();
    let keys: Vec<String> = host.restore().undo.drafts.keys().cloned().collect();
    let gone: Vec<String> = keys.into_iter().filter(|k| crate::draft_tab_key(host, k).encode() == encoded).collect();
    if !gone.is_empty() {
        let undo = &mut host.restore_mut().undo;
        for k in gone {
            undo.drafts.remove(&k);
        }
    }
}

/// The document window `window`'s draft for `tab` keeps its undo history in.
pub fn undo_document(window: &str, tab: &TabKey) -> String {
    format!("{}:undo:{}", opencode_storage::window_namespace(window), tab.encode())
}
