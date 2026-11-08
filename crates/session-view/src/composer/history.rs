//! ↑ and ↓ in the composer recall sent prompts (`p254`), matching OpenCode Desktop's
//! `composer/editor/interaction.ts` (`navigateHistory`).
//!
//! - Up to 100 prompts newest-first, kept apart for normal vs shell mode.
//! - ↑ starts only on an empty composer with the caret at 0. Once a recalled prompt is showing,
//!   ↑ and ↓ step while the caret sits at 0 or at the end; anywhere else the arrow moves the caret.
//! - ↑ places the caret at 0, ↓ at the end, and ↓ past the newest restores the earlier draft.
//! - Editing recalled text, switching scope/session, or toggling shell mode ends the walk.

use std::ops::Range;

use rune::{Key, KeyEvent, Mark, MarkRange, TextEdit};
use serde_json::{Value, json};

const MAX: usize = 100;

/// A prompt as typed: its text and its `@` pills (byte range, id).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub text: String,
    pub pills: Vec<(Range<usize>, String)>,
}

impl Entry {
    pub fn of(edit: &TextEdit) -> Entry {
        Entry {
            text: edit.text.clone(),
            pills: edit.pills().map(|(r, id)| (r, id.to_owned())).collect(),
        }
    }

    pub fn put(&self, edit: &mut TextEdit, caret: usize) {
        edit.replace_text(self.text.clone());
        edit.set_marks(
            self.pills
                .iter()
                .map(|(r, id)| MarkRange::new(r.clone(), Mark::Pill(id.clone())))
                .collect(),
        );
        edit.set_caret(caret);
    }

    pub fn json(&self) -> Value {
        json!({
            "text": self.text,
            "pills": self.pills.iter().map(|(r, id)| json!([r.start, r.end, id])).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Option<Entry> {
        let text = v["text"].as_str()?.to_owned();
        let pills = v["pills"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let (s, e) = (p[0].as_u64()? as usize, p[1].as_u64()? as usize);
                (s < e && e <= text.len() && text.is_char_boundary(s) && text.is_char_boundary(e))
                    .then(|| (s..e, p[2].as_str().unwrap_or_default().to_owned()))
            })
            .collect();
        Some(Entry { text, pills })
    }
}

#[derive(Clone, Debug)]
struct Walk<Scope> {
    index: usize,
    draft: Entry,
    caret: usize,
    shown: String,
    scope: Scope,
    shell: bool,
}

/// Shared prompt-recall store parameterized by page/session scope (`String` by default).
#[derive(Clone, Debug)]
pub struct PromptHistory<Scope = String> {
    normal: Vec<Entry>,
    shell: Vec<Entry>,
    walk: Option<Walk<Scope>>,
}

impl<Scope> Default for PromptHistory<Scope> {
    fn default() -> Self {
        Self {
            normal: Vec::new(),
            shell: Vec::new(),
            walk: None,
        }
    }
}

impl<Scope: Clone + PartialEq> PromptHistory<Scope> {
    pub fn from_json(v: &Value) -> Self {
        let list = |k: &str| {
            v[k].as_array()
                .into_iter()
                .flatten()
                .filter_map(Entry::from_json)
                .take(MAX)
                .collect()
        };
        Self {
            normal: list("normal"),
            shell: list("shell"),
            walk: None,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "normal": self.normal.iter().map(Entry::json).collect::<Vec<_>>(),
            "shell": self.shell.iter().map(Entry::json).collect::<Vec<_>>(),
        })
    }

    pub fn entries(&self, shell: bool) -> &[Entry] {
        if shell { &self.shell } else { &self.normal }
    }

    /// Records a sent prompt and ends any active walk. Returns `true` when the stored list changed.
    pub fn add(&mut self, entry: Entry, shell: bool) -> bool {
        self.walk = None;
        if entry.text.trim().is_empty() {
            return false;
        }
        let list = if shell { &mut self.shell } else { &mut self.normal };
        if list.first() == Some(&entry) {
            return false;
        }
        list.insert(0, entry);
        list.truncate(MAX);
        true
    }

    /// ↑ or ↓ on the composer: `true` when it recalled or restored a prompt.
    pub fn step(&mut self, composer: &mut TextEdit, scope: &Scope, shell: bool, up: bool) -> bool {
        if composer.anchor != composer.focus || composer.preedit.is_some() {
            return false;
        }
        if self.walk.as_ref().is_some_and(|w| {
            w.shown != composer.text || w.shell != shell || &w.scope != scope
        }) {
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
            self.walk = Some(Walk {
                index: next,
                draft,
                caret,
                shown: composer.text.clone(),
                scope: scope.clone(),
                shell,
            });
            return true;
        }
        let Some(w) = self.walk.take() else { return false };
        match w.index.checked_sub(1) {
            Some(newer) => {
                let entry = &list[newer];
                entry.put(composer, entry.text.len());
                self.walk = Some(Walk {
                    index: newer,
                    shown: composer.text.clone(),
                    ..w
                });
            }
            None => w.draft.put(composer, w.caret),
        }
        true
    }
}

/// Unmodified ↑ (`Some(true)`) or ↓ (`Some(false)`).
pub fn arrow_up(ev: &KeyEvent) -> Option<bool> {
    if ev.mods.alt || ev.mods.ctrl || ev.mods.cmd || ev.mods.shift {
        return None;
    }
    match ev.key {
        Key::Up => Some(true),
        Key::Down => Some(false),
        _ => None,
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
