//! ↑ and ↓ in the composer recall the prompts you sent, as in Desktop (`composer/editor/interaction.ts`
//! `navigateHistory`, `composer/history/entry.ts` and `store.ts`).
//!
//! - One list for every session and project, newest first, 100 long, kept apart for shell mode.
//!   Sending records the prompt (text and its `@` pills); an empty one, or one equal to the newest,
//!   adds nothing. The lists are saved to `prompt-history.json`.
//! - ↑ starts only on an empty composer with the caret at 0. Once a recalled prompt is showing,
//!   either arrow works while the caret is at the start or the end of the text. Anywhere else the
//!   arrow moves the caret as usual, and so does ↑ past the oldest entry.
//! - ↑ puts the caret at the start of the older prompt, ↓ at the end of the newer one, so the
//!   arrows chain. ↓ past the newest puts back the draft you had, caret where it was.
//! - Editing the recalled text, changing page or switching shell mode ends the walk: the next ↑
//!   starts from the newest again. Recalling is instant.
//!
//! Not carried over: Desktop also keeps attachments and file comments in an entry, and ⇧↑ opens a
//! searchable history popover. Here an entry is its text and pills.

use std::ops::Range;
use std::path::PathBuf;

use rune::{Key, KeyEvent, Mark, MarkRange, TextEdit, Tx};
use serde_json::{Value, json};

use crate::{App, Route};

const MAX: usize = 100;

/// A prompt as typed: its text and its pills (byte range, id).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub text: String,
    pub pills: Vec<(Range<usize>, String)>,
}

impl Entry {
    pub fn of(edit: &TextEdit) -> Entry {
        Entry { text: edit.text.clone(), pills: edit.pills().map(|(r, id)| (r, id.to_owned())).collect() }
    }

    pub(crate) fn put(&self, edit: &mut TextEdit, caret: usize) {
        edit.replace_text(self.text.clone());
        edit.set_marks(self.pills.iter().map(|(r, id)| MarkRange::new(r.clone(), Mark::Pill(id.clone()))).collect());
        edit.set_caret(caret);
    }

    fn json(&self) -> Value {
        json!({ "text": self.text, "pills": self.pills.iter().map(|(r, id)| json!([r.start, r.end, id])).collect::<Vec<_>>() })
    }

    fn from_json(v: &Value) -> Option<Entry> {
        let text = v["text"].as_str()?.to_owned();
        let pills = v["pills"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let (s, e) = (p[0].as_u64()? as usize, p[1].as_u64()? as usize);
                (s < e && e <= text.len() && text.is_char_boundary(s) && text.is_char_boundary(e)).then(|| (s..e, p[2].as_str().unwrap_or_default().to_owned()))
            })
            .collect();
        Some(Entry { text, pills })
    }
}

/// A walk through the list: where it is, and the draft ↓ comes back to.
#[derive(Clone, Debug)]
struct Walk {
    index: usize,
    draft: Entry,
    caret: usize,
    /// What the composer must still hold for the walk to go on.
    shown: String,
    route: Route,
    shell: bool,
}

#[derive(Default)]
pub struct PromptHistory {
    normal: Vec<Entry>,
    shell: Vec<Entry>,
    walk: Option<Walk>,
    path: Option<PathBuf>,
}

fn file() -> Option<PathBuf> {
    crate::restore::data_dir().map(|d| d.join("prompt-history.json"))
}

impl PromptHistory {
    /// The saved lists; later sends save them again.
    pub fn load() -> PromptHistory {
        let path = file();
        let v: Value = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let list = |k: &str| v[k].as_array().into_iter().flatten().filter_map(Entry::from_json).take(MAX).collect();
        PromptHistory { normal: list("normal"), shell: list("shell"), walk: None, path }
    }

    pub fn entries(&self, shell: bool) -> &[Entry] {
        if shell { &self.shell } else { &self.normal }
    }

    /// Records a sent prompt and ends any walk.
    pub fn add(&mut self, entry: Entry, shell: bool) {
        self.walk = None;
        if entry.text.trim().is_empty() {
            return;
        }
        let list = if shell { &mut self.shell } else { &mut self.normal };
        if list.first() == Some(&entry) {
            return;
        }
        list.insert(0, entry);
        list.truncate(MAX);
        self.save();
    }

    fn save(&self) {
        let Some(path) = self.path.clone() else { return };
        let body = json!({ "normal": self.normal.iter().map(Entry::json).collect::<Vec<_>>(), "shell": self.shell.iter().map(Entry::json).collect::<Vec<_>>() }).to_string();
        crate::disk::later("prompt-history", move || {
            crate::disk::replace(&path, body.as_bytes());
        });
    }

    /// ↑ or ↓ on the composer: whether it recalled (or restored) a prompt. Arrows that aren't
    /// ours return false and move the caret.
    pub fn step(&mut self, composer: &mut TextEdit, route: &Route, shell: bool, up: bool) -> bool {
        if composer.anchor != composer.focus || composer.preedit.is_some() {
            return false;
        }
        if self.walk.as_ref().is_some_and(|w| w.shown != composer.text || w.shell != shell || &w.route != route) {
            self.walk = None;
        }
        let at = composer.focus.min(composer.text.len());
        let ok = match (&self.walk, up) {
            (Some(_), _) => at == 0 || at == composer.text.len(),
            (None, true) => at == 0 && composer.text.is_empty(),
            (None, false) => at == composer.text.len(),
        };
        let list = if shell { &self.shell } else { &self.normal };
        if !ok {
            return false;
        }
        if up {
            let next = self.walk.as_ref().map_or(0, |w| w.index + 1);
            let Some(entry) = list.get(next) else { return false };
            let (draft, caret) = match self.walk.take() {
                Some(w) => (w.draft, w.caret),
                None => (Entry::of(composer), at),
            };
            entry.put(composer, 0);
            self.walk = Some(Walk { index: next, draft, caret, shown: composer.text.clone(), route: route.clone(), shell });
            return true;
        }
        let Some(w) = self.walk.take() else { return false };
        match w.index.checked_sub(1) {
            Some(newer) => {
                let entry = &list[newer];
                entry.put(composer, entry.text.len());
                self.walk = Some(Walk { index: newer, shown: composer.text.clone(), ..w });
            }
            None => w.draft.put(composer, w.caret),
        }
        true
    }
}

impl App {
    /// The composer's ↑ and ↓. The `@` and `/` popovers, the menus and the pickers use the
    /// arrows themselves while open, so they win.
    pub fn history_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let up = match ev.key {
            Key::Up => true,
            Key::Down => false,
            _ => return false,
        };
        if ev.mods.alt || ev.mods.ctrl || ev.mods.cmd || ev.mods.shift || !self.composer.focused || !self.has_composer() {
            return false;
        }
        if self.mentions.open || self.add.open || self.slash_query().is_some() || self.pickers.open.is_some() || self.switcher.open || self.settings.open {
            return false;
        }
        let (route, shell) = (self.route.clone(), self.add.shell);
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
        let (entry, shell) = (Entry::of(&self.composer), self.add.shell);
        send(self);
        if self.composer.text.is_empty() && !entry.text.is_empty() {
            self.prompts.add(entry, shell);
        }
    }
}

#[cfg(test)]
#[path = "prompt_history_tests.rs"]
mod tests;
