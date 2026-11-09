//! ↑ and ↓ in the composer recall the prompts you sent, backed by
//! [`session_view::composer::history`] and persisted to `prompt-history.json`.

use std::path::PathBuf;

use rune::{KeyEvent, TextEdit, Tx};
use serde_json::Value;

pub use session_view::composer::history::Entry;
use session_view::composer::history::{PromptHistory as Inner, arrow_up};

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

    pub fn entries(&self, shell: bool) -> &[Entry] {
        self.inner.entries(shell)
    }

    /// Records a sent prompt and ends any walk.
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

    /// ↑ or ↓ on the composer: whether it recalled (or restored) a prompt.
    pub fn step(&mut self, composer: &mut TextEdit, route: &Route, shell: bool, up: bool) -> bool {
        self.inner.step(composer, route, shell, up)
    }
}

impl App {
    /// The composer's ↑ and ↓. The `@` and `/` popovers, the menus and the pickers use the
    /// arrows themselves while open, so they win.
    pub fn history_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(up) = arrow_up(ev) else { return false };
        if !self.composer.focused || !self.has_composer() {
            return false;
        }
        if self.mentions.open
            || self.add.open()
            || self.slash_query().is_some()
            || self.pickers.open.is_some()
            || self.switcher.open
            || self.settings.open
        {
            return false;
        }
        let (route, shell) = (self.route.clone(), self.add.shell());
        if !self.prompts.step(&mut self.composer, &route, shell, up) {
            return false;
        }
        self.mentions_settle();
        tx.instant();
        true
    }

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
