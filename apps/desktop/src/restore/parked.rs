//! What a draft holds besides its words, kept per draft key while another draft shows: its
//! pills and caret, images and files, files named by path, context lines and quotes. Its words
//! stay in `Restore::drafts`, which every other draft writer already uses; the shown draft's
//! pieces live in the composer, `App::attach` and `App::quote`.
//!
//! All of it outlives a relaunch, in the draft's document (`draft_doc`): images and files by
//! content hash, their bytes stored once. While another draft shows, the composer's own editor
//! is kept too (`Restore::edits`), so its undo history is there when it shows again.

use std::ops::Range;

use opencode_kit::client::PathAttachment;
use opencode_kit::model::{Attachment, PromptComment};
use rune::{Mark, MarkRange, TextEdit};

use crate::add_selection::ContextFile;
use crate::rewind::SavedDraft;
use crate::App;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parked {
    pub pills: Vec<(Range<usize>, String)>,
    pub caret: Option<usize>,
    pub files: Vec<Attachment>,
    pub paths: Vec<PathAttachment>,
    pub context: Vec<ContextFile>,
    pub quotes: Vec<PromptComment>,
}

impl Parked {
    /// Nothing worth keeping once its words are gone: no attachment, context or quote.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.paths.is_empty() && self.context.is_empty() && self.quotes.is_empty()
    }

    /// Whether to keep it beside `text`: pills and a caret only mean something with words.
    pub(super) fn worth_keeping(&self, text: &str) -> bool {
        !self.is_empty() || (!text.trim().is_empty() && (!self.pills.is_empty() || self.caret.is_some_and(|c| c != text.len())))
    }

    pub fn of(draft: &SavedDraft) -> Parked {
        Parked {
            pills: draft.entry.pills.clone(),
            caret: Some(draft.caret),
            files: draft.files.clone(),
            paths: draft.paths.clone(),
            context: Vec::new(),
            quotes: draft.quotes.clone(),
        }
    }

    /// A composer on `text` with these pills and caret, where they still fit it (its words may
    /// have changed while it was away, as a queued prompt's merge does).
    fn edit(&self, text: &str) -> TextEdit {
        let mut edit = TextEdit::new(text);
        let fits = |r: &Range<usize>| text.get(r.clone()).is_some_and(|s| !s.is_empty());
        edit.set_marks(self.pills.iter().filter(|(r, _)| fits(r)).map(|(r, id)| MarkRange::new(r.clone(), Mark::Pill(id.clone()))).collect());
        if let Some(c) = self.caret.filter(|&c| text.is_char_boundary(c)) {
            edit.set_caret(c);
        }
        edit
    }
}

impl App {
    /// Whether the composer holds `key`'s draft: the shown one, or, before any draft showed (a
    /// story), the open page's.
    pub(crate) fn composer_holds(&self, key: &str) -> bool {
        match &self.restore.shown {
            Some(shown) => shown == key,
            None => crate::restore::draft_key(&*self.route).as_deref() == Some(key),
        }
    }

    /// The composer's pieces besides its words, left as they are.
    pub(super) fn composer_parked(&self) -> Parked {
        Parked {
            pills: self.composer.pills().map(|(r, id)| (r, id.to_owned())).collect(),
            caret: Some(self.composer.range().1.min(self.composer.text.len())),
            files: self.attach.files.clone(),
            paths: self.attach.paths.clone(),
            context: self.attach.context.clone(),
            quotes: self.quote.quotes(),
        }
    }

    /// The shown draft's pieces as they stand in the composer, borrowed (`draft_doc`).
    pub(super) fn composer_parts(&self) -> Option<super::draft_doc::Parts<'_>> {
        let caret = self.composer.range().1.min(self.composer.text.len());
        // Words typed at their end, nothing else: the common keystroke builds nothing more.
        if caret == self.composer.text.len() && !self.attach.any() && self.attach.context.is_empty() && !self.quote.any() && self.composer.pills().next().is_none() {
            return None;
        }
        Some(super::draft_doc::Parts {
            pills: self.composer.pills().map(|(r, id)| (r.start, r.end, id)).collect(),
            caret: Some(caret),
            files: &self.attach.files,
            paths: &self.attach.paths,
            context: &self.attach.context,
            quotes: self.quote.items.iter().map(|q| &q.comment).collect(),
        })
    }

    /// Draft `key`'s pieces besides its words (the composer's when it holds them), or None.
    pub(crate) fn draft_parked(&self, key: &str) -> Option<Parked> {
        let p = if self.composer_holds(key) { self.composer_parked() } else { self.restore.parked.get(key)?.clone() };
        (!p.is_empty()).then_some(p)
    }

    /// Keeps `text` and `parked` as draft `key`'s while another shows.
    pub(crate) fn park_draft(&mut self, key: String, text: String, parked: Parked) {
        if parked.worth_keeping(&text) {
            self.restore.parked.insert(key.clone(), parked);
        } else {
            self.restore.parked.remove(&key);
        }
        if text.trim().is_empty() {
            self.restore.drafts.remove(&key);
        } else {
            self.restore.drafts.insert(key, text);
        }
    }

    /// Takes the shown draft out of the composer: its words and the rest.
    pub(super) fn take_composer_draft(&mut self) -> (String, Parked) {
        let parked = Parked {
            files: std::mem::take(&mut self.attach.files),
            paths: std::mem::take(&mut self.attach.paths),
            context: std::mem::take(&mut self.attach.context),
            quotes: self.quote.take(),
            ..self.composer_parked()
        };
        (std::mem::take(&mut self.composer.text), parked)
    }

    /// Puts draft `key`'s words and pieces in the composer, keeping its focus.
    pub(super) fn put_composer_draft(&mut self, key: Option<&str>) {
        let text = key.and_then(|k| self.restore.drafts.get(k).cloned()).unwrap_or_default();
        let parked = key.and_then(|k| self.restore.parked.remove(k)).unwrap_or_default();
        let focused = self.composer.focused;
        // Its own editor, undo history and all, when its words are as it left them.
        let kept = key.and_then(|k| self.restore.edits.remove(k)).filter(|e| e.text == text);
        *self.composer = kept.unwrap_or_else(|| parked.edit(&text));
        self.composer.focused = focused;
        self.attach.files = parked.files;
        self.attach.paths = parked.paths;
        self.attach.context = parked.context;
        for q in parked.quotes {
            self.quote.push(q);
        }
    }

    /// The shown draft is gone (its tab closed or moved to another window): the composer empties
    /// and holds no draft until the next shows.
    pub(crate) fn drop_shown_draft(&mut self) {
        self.restore.shown = None;
        let _ = self.take_composer_draft();
        let focused = self.composer.focused;
        *self.composer = TextEdit::new("");
        self.composer.focused = focused;
    }

    /// A finished upload lands in the draft that started it, shown or not.
    pub(crate) fn land_upload(&mut self, key: Option<&str>, r: PathAttachment) {
        match key {
            Some(k) if !self.composer_holds(k) => {
                let paths = &mut self.restore.parked.entry(k.to_owned()).or_default().paths;
                if !paths.iter().any(|x| x.path == r.path) {
                    paths.push(r);
                }
            }
            _ => self.attach.add_path(r),
        }
    }
}
