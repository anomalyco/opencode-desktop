//! The prompts you sent, which ↑ and ↓ and Shift+↑ in the composer recall (the suggestion
//! machine's browsing and history list, `crate::suggestions`), backed by
//! [`session_view::composer::history`] and kept in the app's database as one document
//! (`opencode_storage::PROMPT_HISTORY_DOCUMENT`; long prompts are chunked there). The first
//! frame doesn't draw it, so it comes in after (`PromptHistory::hydrate`).

use opencode_storage::{Lookup, PROMPT_HISTORY_DOCUMENT};
use serde_json::Value;

pub use session_view::composer::history::Entry;
use session_view::composer::history::PromptHistory as Inner;

use crate::{App, Route};

#[derive(Default)]
pub struct PromptHistory {
    inner: Inner<Route>,
    phase: Phase,
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
        let mut h = PromptHistory { inner: Inner::default(), phase: if persists { Phase::Waiting { sent: Vec::new(), asked: false } } else { Phase::Off } };
        h.hydrate(None);
        h
    }

    /// Takes the saved lists once the storage has them in memory, the prompts sent since launch on
    /// top; the first time they aren't, asks for them (`waker` runs once they're in).
    pub fn hydrate(&mut self, waker: Option<rune::Waker>) {
        let Phase::Waiting { asked, .. } = &mut self.phase else { return };
        if !crate::storage::is_ready() {
            return;
        }
        let v: Value = match crate::storage::doc(PROMPT_HISTORY_DOCUMENT) {
            Lookup::Ready(Some(text)) => serde_json::from_str(&text).unwrap_or_default(),
            Lookup::Ready(None) => Value::Null,
            Lookup::Pending => {
                if !*asked && let Some(w) = waker {
                    *asked = true;
                    crate::storage::load_docs(&[PROMPT_HISTORY_DOCUMENT], Box::new(move || w()));
                }
                return;
            }
        };
        let Phase::Waiting { sent, .. } = std::mem::replace(&mut self.phase, Phase::Read) else { return };
        self.inner = Inner::from_json(&v);
        let changed = sent.into_iter().fold(false, |changed, (entry, shell)| self.inner.add(entry, shell) | changed);
        if changed {
            self.save();
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
            self.save();
        }
    }

    fn save(&self) {
        crate::storage::set_doc(PROMPT_HISTORY_DOCUMENT, Some(self.inner.to_json().to_string()));
    }
}

impl App {
    /// Runs `send`, and records the prompt in the composer if that sent it (the composer is
    /// empty afterwards). A send that failed or waits leaves the text, and records nothing.
    pub fn record_sent(&mut self, send: impl FnOnce(&mut App)) {
        let (entry, shell) = (Entry::of(&self.composer), self.add.shell());
        send(self);
        if self.composer.text.is_empty() && !entry.text.is_empty() {
            self.prompts.add(entry, shell);
        }
    }
}
