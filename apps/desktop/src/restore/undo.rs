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
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

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
    ready: Arc<AtomicBool>,
    /// Per document only the newest generation can deliver. Superseded workers only own
    /// their old result cell, not permission to write to storage.
    sent: HashMap<String, Arc<Job>>,
    writes: u64,
}

enum ResultState { Encoding(Vec<Box<dyn FnOnce() + Send>>), Ready(Option<String>), Delivered }
struct Job { state: Mutex<ResultState>, done: Condvar }
impl Job {
    fn wait(&self) {
        drop(self.done.wait_while(self.state.lock().unwrap_or_else(|e| e.into_inner()), |s| matches!(s, ResultState::Encoding(_))).unwrap_or_else(|e| e.into_inner()));
    }
    fn deliver(&self, doc: &str) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*state, ResultState::Encoding(_)) { return false; }
        let old = std::mem::replace(&mut *state, ResultState::Delivered);
        drop(state);
        if let ResultState::Ready(value) = old { crate::storage::set_doc(doc, value); }
        true
    }
}

/// Owned only by a terminal close; on a reversible quit the live window keeps delivery rights.
#[cfg(not(target_arch="wasm32"))]
static CLOSING: Mutex<Option<HashMap<String, Arc<Job>>>> = Mutex::new(None);

#[cfg(not(target_arch="wasm32"))]
pub(crate) fn finish_closing() {
    let closed=CLOSING.lock().unwrap_or_else(|e|e.into_inner()).take();
    for (doc,job) in closed.into_iter().flatten() { job.wait(); job.deliver(&doc); }
}

#[cfg(not(target_arch="wasm32"))]
pub(crate) fn forget_closing(window:&str) {
    if let Some(pending)=CLOSING.lock().unwrap_or_else(|e|e.into_inner()).as_mut() {
        let prefix=format!("{}:undo:",opencode_storage::window_namespace(window));
        pending.retain(|doc,_| !doc.starts_with(&prefix));
    }
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
        self.capture_undo(now, tx, false);
    }

    /// A tab switch is the last turn that can read this editor as the shown one. Submit
    /// its current generation before parking the field, even inside the typing debounce.
    /// `sent` is per document, so a later quit on Home waits for this generation too.
    pub(super) fn park_undo(&mut self) {
        self.capture_undo(*self.clock, &mut Tx::default(), true);
    }

    fn capture_undo(&mut self, now:f64, tx:&mut Tx, closing:bool) {
        self.store_done_histories();
        if !self.persists() || self.window.id.is_empty() || self.composer.is_secure() {
            return;
        }
        let Some(key) = self.restore.shown.clone() else { return };
        // Attaching a nonempty history to a temporary empty field invents a deletion step:
        // the first Undo would merely reveal the saved draft instead of reversing its last edit.
        if self.restore.late_shown.is_some() && self.composer.text.is_empty() && self.composer.history_len()==(0,0) { return; }
        let doc = undo_document(&self.window.id, &self.draft_tab_key(&key));
        if !self.restore.undo.restored.contains(&key) && !(closing && self.composer.history_len() != (0,0)) && !self.take_back_history(&key, &doc) {
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
            _ if closing || written_len != len => {}
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
        let job=Arc::new(Job { state: Mutex::new(ResultState::Encoding(Vec::new())), done:Condvar::new() });
        let (ready,waker)=(kept.ready.clone(),(*self.waker).clone());
        kept.sent.insert(doc.clone(), job.clone());
        kept.written.insert(key, (len, print));
        kept.candidate = None;
        crate::plugin_host::tasks::run(move || {
            let value = edit.history_json(STEPS, BYTES).map(|v| v.to_string());
            let old=std::mem::replace(&mut *job.state.lock().unwrap_or_else(|e|e.into_inner()),ResultState::Ready(value));
            ready.store(true,Ordering::Release);
            job.done.notify_all();
            if let ResultState::Encoding(notify)=old { for done in notify {done()} }
            if let Some(w) = waker {
                w();
            }
        });
    }

    /// Stores the histories workers finished, each unless its draft was dropped or written again
    /// since it was sent.
    fn store_done_histories(&mut self) {
        let kept = &mut self.restore.undo;
        if kept.ready.swap(false,Ordering::AcqRel) {
            kept.sent.retain(|doc,job| !job.deliver(doc));
            #[cfg(all(windows,not(feature="hot")))]
            if kept.sent.is_empty(){crate::quit_save::workspace_activity();}
        }
    }

    /// An immediate close bypasses the typing debounce, not the serializer or its generation
    /// check. A terminal legacy close delivers after teardown, before the database closes.
    #[cfg(not(target_arch="wasm32"))]
    pub(crate) fn close_undo(&mut self) {
        self.capture_undo(*self.clock,&mut Tx::default(),true);
        if !self.restore.undo.sent.is_empty() {
            CLOSING.lock().unwrap_or_else(|e|e.into_inner()).get_or_insert_with(HashMap::new).extend(self.restore.undo.sent.drain());
        }
    }

    /// No UI wait. Once encoding finishes the coordinator re-enters this live window,
    /// validates its quit ticket, and drains only still-current generations.
    #[cfg(windows)]
    pub(crate) fn quit_undo_delivered(&mut self, notify:impl FnOnce()+Send+'static) -> bool {
        self.capture_undo(*self.clock,&mut Tx::default(),true);
        for job in self.restore.undo.sent.values() {
            let mut state=job.state.lock().unwrap_or_else(|e|e.into_inner());
            if let ResultState::Encoding(waiters)=&mut *state {waiters.push(Box::new(notify));return false;}
        }
        self.restore.undo.sent.retain(|doc,job| !job.deliver(doc));
        true
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
                {
                    // A ready store can precede the host waker. Do not consume the once-only
                    // request without submitting the load (including a scripted reopen).
                    let wake=(*self.waker).clone();
                    crate::storage::load_docs(&[doc], Box::new(move || if let Some(w)=wake { w() }));
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
    #[cfg(windows)]
    pub(super) fn adopt_recovered_history(&mut self,key:&str){
        let doc=undo_document(&self.window.id,&self.draft_tab_key(key));
        self.restore.undo.sent.remove(&doc);
        self.restore.undo.print=None;
        self.restore.undo.candidate=None;
        let print=self.print(key);
        self.restore.undo.restored.insert(key.to_owned());
        self.restore.undo.written.insert(key.to_owned(),(self.composer.history_len(),print));
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
