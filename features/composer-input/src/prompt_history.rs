//! The prompts you sent, which ↑ and ↓ and Shift+↑ in the composer recall (the suggestion
//! machine's browsing and history list, [`crate::suggestions`]), backed by
//! [`session_view::composer::history`] and kept in the app's database as one document
//! ([`PROMPT_HISTORY_DOCUMENT`]; long prompts are chunked there). The first frame doesn't draw it,
//! so it comes in after ([`PromptHistory::hydrate`]).

use opencode_platform::storage;
use rune::{TextEdit, Waker};
use serde_json::Value;

pub use opencode_platform::storage::DocLookup;
pub use session_view::composer::history::Entry;
use session_view::composer::history::PromptHistory as Inner;

/// Document key matching `opencode_storage::PROMPT_HISTORY_DOCUMENT`.
pub const PROMPT_HISTORY_DOCUMENT: &str = "opencode.global.dat:prompt-history.v1";

#[derive(Default)]
pub struct PromptHistory {
    inner: Inner<String>,
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
        let mut h = PromptHistory {
            inner: Inner::default(),
            phase: if persists { Phase::Waiting { sent: Vec::new(), asked: false } } else { Phase::Off },
        };
        h.hydrate(None);
        h
    }

    /// Takes the saved lists once the storage has them in memory, the prompts sent since launch on
    /// top; the first time they aren't, asks for them (`waker` runs once they're in).
    pub fn hydrate(&mut self, waker: Option<Waker>) {
        let Phase::Waiting { asked, .. } = &mut self.phase else { return };
        if !storage::is_ready() {
            return;
        }
        let v: Value = match storage::doc(PROMPT_HISTORY_DOCUMENT) {
            DocLookup::Ready(Some(text)) => serde_json::from_str(&text).unwrap_or_default(),
            DocLookup::Ready(None) => Value::Null,
            DocLookup::Pending => {
                if !*asked
                    && let Some(w) = waker
                {
                    *asked = true;
                    storage::load_docs(&[PROMPT_HISTORY_DOCUMENT], Box::new(move || w()));
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

    /// Drops a prompt whose send failed and returned to the composer (`removeHistoryEntry`, `entry.ts:67-76`).
    pub fn remove(&mut self, entry: &Entry, shell: bool) {
        if let Phase::Waiting { sent, .. } = &mut self.phase {
            sent.retain(|(e, sh)| !(*sh == shell && e == entry));
        }
        if self.inner.remove(entry, shell) && matches!(self.phase, Phase::Read) {
            self.save();
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

    fn save(&self) {
        storage::set_doc(PROMPT_HISTORY_DOCUMENT, Some(self.inner.to_json().to_string()));
    }
}
