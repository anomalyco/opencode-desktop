//! The prompts you sent, which ↑ and ↓ and Shift+↑ in the composer recall (the suggestion
//! machine's browsing and history list, [`crate::suggestions`]), backed by
//! [`session_view::composer::history`] and kept in the app's database as one document
//! ([`PROMPT_HISTORY_DOCUMENT`]; long prompts are chunked there). The first frame doesn't draw it,
//! so it comes in after ([`PromptHistory::hydrate`]).

use std::sync::{Arc, OnceLock};

use rune::{TextEdit, Waker};
use serde_json::Value;

pub use session_view::composer::history::Entry;
use session_view::composer::history::PromptHistory as Inner;

/// Document key matching `opencode_storage::PROMPT_HISTORY_DOCUMENT`.
pub const PROMPT_HISTORY_DOCUMENT: &str = "opencode.global.dat:prompt-history.v1";

/// Result of reading a persisted prompt-history document from memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocLookup {
    Ready(Option<Arc<str>>),
    /// Stored on disk and not read yet; call [`StorageOps::load_docs`] to fetch it.
    Pending,
}

/// Host persistence callbacks for [`PromptHistory`] (`opencode.global.dat:prompt-history.v1`).
#[derive(Clone, Copy)]
pub struct StorageOps {
    /// Whether the backing document store has hydrated into memory.
    pub is_ready: fn() -> bool,
    /// Reads document `key` from memory (or reports [`DocLookup::Pending`]).
    pub doc: fn(key: &str) -> DocLookup,
    /// Requests background loading of `keys`, calling `done` when ready.
    pub load_docs: fn(keys: &[&str], done: Box<dyn FnOnce() + Send>),
    /// Writes (or clears when `None`) document `key`.
    pub set_doc: fn(key: &str, value: Option<String>),
}

fn noop_is_ready() -> bool {
    false
}

fn noop_doc(_: &str) -> DocLookup {
    DocLookup::Ready(None)
}

fn noop_load_docs(_: &[&str], _: Box<dyn FnOnce() + Send>) {}

fn noop_set_doc(_: &str, _: Option<String>) {}

const DEFAULT_STORAGE: StorageOps = StorageOps {
    is_ready: noop_is_ready,
    doc: noop_doc,
    load_docs: noop_load_docs,
    set_doc: noop_set_doc,
};

static STORAGE: OnceLock<StorageOps> = OnceLock::new();

/// Registers the host's process-wide document storage callbacks for [`PromptHistory`].
/// Call once at startup before [`PromptHistory::load`] or [`PromptHistory::hydrate`].
pub fn configure(ops: StorageOps) {
    STORAGE.set(ops).ok();
}

fn storage_ops() -> StorageOps {
    STORAGE.get().copied().unwrap_or(DEFAULT_STORAGE)
}

#[derive(Default)]
pub struct PromptHistory {
    inner: Inner<String>,
    phase: Phase,
    ops: Option<StorageOps>,
}

/// Whether the saved lists are in memory.
#[derive(Default)]
enum Phase {
    /// Not yet: the prompts sent meanwhile (oldest first) go on top of them once they are.
    /// `asked`: their read is on its way.
    Waiting { sent: Vec<(Entry, bool)>, asked: bool },
    /// In memory: every send saves.
    Read,
    /// A run that keeps nothing.
    #[default]
    Off,
}

impl PromptHistory {
    /// The saved lists when they're in memory already, else none until [`PromptHistory::hydrate`].
    /// `persists`: sends are saved.
    pub fn load(persists: bool) -> PromptHistory {
        let mut h = PromptHistory {
            inner: Inner::default(),
            phase: if persists { Phase::Waiting { sent: Vec::new(), asked: false } } else { Phase::Off },
            ops: None,
        };
        h.hydrate(None);
        h
    }

    /// Like [`PromptHistory::load`], bound to explicit [`StorageOps`] instead of the process-wide
    /// callbacks registered with [`configure`].
    pub fn load_with(persists: bool, ops: StorageOps) -> PromptHistory {
        let mut h = PromptHistory {
            inner: Inner::default(),
            phase: if persists { Phase::Waiting { sent: Vec::new(), asked: false } } else { Phase::Off },
            ops: Some(ops),
        };
        h.hydrate(None);
        h
    }

    fn ops(&self) -> StorageOps {
        self.ops.unwrap_or_else(storage_ops)
    }

    /// Takes the saved lists once the storage has them in memory, the prompts sent since launch on
    /// top; the first time they aren't, asks for them (`waker` runs once they're in).
    pub fn hydrate(&mut self, waker: Option<Waker>) {
        self.hydrate_with(self.ops(), waker);
    }

    /// Like [`PromptHistory::hydrate`], using explicit [`StorageOps`].
    pub fn hydrate_with(&mut self, ops: StorageOps, waker: Option<Waker>) {
        let Phase::Waiting { asked, .. } = &mut self.phase else { return };
        if !(ops.is_ready)() {
            return;
        }
        let v: Value = match (ops.doc)(PROMPT_HISTORY_DOCUMENT) {
            DocLookup::Ready(Some(text)) => serde_json::from_str(&text).unwrap_or_default(),
            DocLookup::Ready(None) => Value::Null,
            DocLookup::Pending => {
                if !*asked
                    && let Some(w) = waker
                {
                    *asked = true;
                    (ops.load_docs)(&[PROMPT_HISTORY_DOCUMENT], Box::new(move || w()));
                }
                return;
            }
        };
        let Phase::Waiting { sent, .. } = std::mem::replace(&mut self.phase, Phase::Read) else { return };
        self.inner = Inner::from_json(&v);
        let changed = sent.into_iter().fold(false, |changed, (entry, shell)| self.inner.add(entry, shell) | changed);
        if changed {
            self.save_with(ops);
        }
    }

    pub fn entries(&self, shell: bool) -> &[Entry] {
        self.inner.entries(shell)
    }

    /// Records a sent prompt.
    pub fn add(&mut self, entry: Entry, shell: bool) {
        if let Phase::Waiting { sent, .. } = &mut self.phase {
            sent.push((entry.clone(), shell));
        }
        if self.inner.add(entry, shell) && matches!(self.phase, Phase::Read) {
            self.save_with(self.ops());
        }
    }

    /// Captures the composer's current prompt before a send attempt.
    pub fn capture(composer: &TextEdit, shell: bool) -> (Entry, bool) {
        (Entry::of(composer), shell)
    }

    /// Records a prompt captured before sending if sending emptied the composer.
    /// A send that failed or waits leaves the text, and records nothing.
    pub fn record_if_sent(&mut self, composer: &TextEdit, entry: Entry, shell: bool) {
        if composer.text.is_empty() && !entry.text.is_empty() {
            self.add(entry, shell);
        }
    }

    fn save_with(&self, ops: StorageOps) {
        (ops.set_doc)(PROMPT_HISTORY_DOCUMENT, Some(self.inner.to_json().to_string()));
    }
}
