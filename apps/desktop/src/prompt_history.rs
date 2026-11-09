//! The prompts you sent, which ↑ and ↓ and Shift+↑ in the composer recall (the suggestion
//! machine's browsing and history list, `crate::suggestions`), backed by
//! [`session_view::composer::history`] and persisted to `prompt-history.json`.

use std::path::PathBuf;

use serde_json::Value;

pub use session_view::composer::history::Entry;
use session_view::composer::history::PromptHistory as Inner;

use crate::{App, Route};

#[derive(Default)]
pub struct PromptHistory {
    inner: Inner<Route>,
    path: Option<PathBuf>,
}

fn file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("prompt-history.json"))
}

impl PromptHistory {
    /// The saved lists; later sends save them again.
    pub fn load() -> PromptHistory {
        let path = file();
        let v: Value = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        PromptHistory { inner: Inner::from_json(&v), path }
    }

    /// ↑ or ↓ through the shared store's own walk, for its tests; the app walks in its
    /// suggestion machine.
    #[cfg(test)]
    pub fn step(&mut self, composer: &mut rune::TextEdit, route: &Route, shell: bool, up: bool) -> bool {
        self.inner.step(composer, route, shell, up)
    }

    pub fn entries(&self, shell: bool) -> &[Entry] {
        self.inner.entries(shell)
    }

    /// Records a sent prompt.
    pub fn add(&mut self, entry: Entry, shell: bool) {
        if self.inner.add(entry, shell) {
            self.save();
        }
    }

    fn save(&self) {
        let Some(path) = self.path.clone() else { return };
        let body = self.inner.to_json().to_string();
        crate::disk::later("prompt-history", move || {
            crate::disk::replace(&path, body.as_bytes());
        });
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

#[cfg(test)]
#[path = "prompt_history_tests.rs"]
mod tests;
