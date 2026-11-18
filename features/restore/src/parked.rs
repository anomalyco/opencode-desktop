//! What a draft holds besides its words, kept per draft key while another draft shows: its
//! pills and caret, images and files, files named by path, context lines and quotes. Its words
//! stay in `Restore::drafts`, which every other draft writer already uses; the shown draft's
//! pieces live in the composer, attachments, and quotes on the host.
//!
//! All of it outlives a relaunch, in the draft's document (`draft_doc`): images and files by
//! content hash, their bytes stored once. While another draft shows, the composer's own editor
//! is kept too (`Restore::edits`), so its undo history is there when it shows again.

use std::ops::Range;

use opencode_kit::client::PathAttachment;
use opencode_kit::model::{Attachment, PromptComment};
use opencode_runtime::SavedDraft;
use rune::{Mark, MarkRange, TextEdit};

use crate::RestoreHost;

pub use opencode_runtime::ContextFile;

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
    pub fn worth_keeping(&self, text: &str) -> bool {
        !self.is_empty() || (!text.trim().is_empty() && (!self.pills.is_empty() || self.caret.is_some_and(|c| c != text.len())))
    }

    /// Builds a [`Parked`] snapshot from a headless [`SavedDraft`].
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

    /// Builds a [`Parked`] snapshot from individual composer parts.
    pub fn from_parts(
        pills: Vec<(Range<usize>, String)>,
        caret: Option<usize>,
        files: Vec<Attachment>,
        paths: Vec<PathAttachment>,
        context: Vec<ContextFile>,
        quotes: Vec<PromptComment>,
    ) -> Parked {
        Parked { pills, caret, files, paths, context, quotes }
    }

    /// A composer on `text` with these pills and caret, where they still fit it (its words may
    /// have changed while it was away, as a queued prompt's merge does).
    pub fn edit(&self, text: &str) -> TextEdit {
        let mut edit = TextEdit::new(text);
        let fits = |r: &Range<usize>| text.get(r.clone()).is_some_and(|s| !s.is_empty());
        edit.set_marks(
            self.pills
                .iter()
                .filter(|(r, _)| fits(r))
                .map(|(r, id)| MarkRange::new(r.clone(), Mark::Pill(id.clone())))
                .collect(),
        );
        if let Some(c) = self.caret.filter(|&c| text.is_char_boundary(c)) {
            edit.set_caret(c);
        }
        edit
    }
}

/// Whether the composer holds `key`'s draft: the shown one, or, before any draft showed (a
/// story), the open page's.
pub fn composer_holds(host: &dyn RestoreHost, key: &str) -> bool {
    match &host.restore().shown {
        Some(shown) => shown == key,
        None => crate::draft_key(&host.route()).as_deref() == Some(key),
    }
}

/// The composer's pieces besides its words, left as they are.
pub fn composer_parked(host: &dyn RestoreHost) -> Parked {
    let composer = host.composer();
    Parked {
        pills: composer.pills().map(|(r, id)| (r, id.to_owned())).collect(),
        caret: Some(composer.range().1.min(composer.text.len())),
        files: host.attach_files().to_vec(),
        paths: host.attach_paths().to_vec(),
        context: host.attach_context().to_vec(),
        quotes: host.quote_comments().into_iter().cloned().collect(),
    }
}

/// The shown draft's pieces as they stand in the composer, borrowed (`draft_doc`).
pub(crate) fn composer_parts(host: &dyn RestoreHost) -> Option<crate::draft_doc::Parts<'_>> {
    let composer = host.composer();
    let caret = composer.range().1.min(composer.text.len());
    let files = host.attach_files();
    let paths = host.attach_paths();
    let context = host.attach_context();
    // Words typed at their end, nothing else: the common keystroke builds nothing more.
    if caret == composer.text.len()
        && files.is_empty()
        && paths.is_empty()
        && context.is_empty()
        && !host.quote_any()
        && composer.pills().next().is_none()
    {
        return None;
    }
    Some(crate::draft_doc::Parts {
        pills: composer.pills().map(|(r, id)| (r.start, r.end, id)).collect(),
        caret: Some(caret),
        files,
        paths,
        context,
        quotes: host.quote_comments(),
    })
}

/// Draft `key`'s pieces besides its words (the composer's when it holds them), or `None`.
pub fn draft_parked(host: &dyn RestoreHost, key: &str) -> Option<Parked> {
    let p = if composer_holds(host, key) {
        composer_parked(host)
    } else {
        host.restore().parked.get(key)?.clone()
    };
    (!p.is_empty()).then_some(p)
}

/// Keeps `text` and `parked` as draft `key`'s while another shows.
pub fn park_draft(host: &mut dyn RestoreHost, key: String, text: String, parked: Parked) {
    let r = host.restore_mut();
    if parked.worth_keeping(&text) {
        r.parked.insert(key.clone(), parked);
    } else {
        r.parked.remove(&key);
    }
    if text.trim().is_empty() {
        r.drafts.remove(&key);
    } else {
        r.drafts.insert(key, text);
    }
}

/// Takes the shown draft out of the composer: its words and the rest.
pub(crate) fn take_composer_draft(host: &mut dyn RestoreHost) -> (String, Parked) {
    let (pills, caret) = {
        let c = host.composer();
        (c.pills().map(|(r, id)| (r, id.to_owned())).collect(), Some(c.range().1.min(c.text.len())))
    };
    let parked = Parked {
        pills,
        caret,
        files: host.take_attach_files(),
        paths: host.take_attach_paths(),
        context: host.take_attach_context(),
        quotes: host.take_quotes(),
    };
    (std::mem::take(&mut host.composer_mut().text), parked)
}

/// Puts draft `key`'s words and pieces in the composer, keeping its focus.
pub(crate) fn put_composer_draft(host: &mut dyn RestoreHost, key: Option<&str>) {
    let text = key.and_then(|k| host.restore().drafts.get(k).cloned()).unwrap_or_default();
    let (parked, kept) = match key {
        Some(k) if host.restore().parked.contains_key(k) || host.restore().edits.contains_key(k) => {
            let r = host.restore_mut();
            let parked = r.parked.remove(k).unwrap_or_default();
            // Its own editor, undo history and all, when its words are as it left them.
            let kept = r.edits.remove(k).filter(|e| e.text == text);
            (parked, kept)
        }
        _ => (Parked::default(), None),
    };
    let composer = host.composer_mut();
    let focused = composer.focused;
    *composer = kept.unwrap_or_else(|| parked.edit(&text));
    composer.focused = focused;
    host.set_attach_files(parked.files);
    host.set_attach_paths(parked.paths);
    host.set_attach_context(parked.context);
    for q in parked.quotes {
        host.push_quote(q);
    }
}

/// Whether draft `key` holds anything worth reopening: words, files, context or cards.
pub fn draft_has_content(host: &dyn RestoreHost, key: &str) -> bool {
    if composer_holds(host, key) {
        return !host.composer().text.trim().is_empty()
            || !host.attach_files().is_empty()
            || !host.attach_paths().is_empty()
            || !host.attach_context().is_empty()
            || host.quote_any();
    }
    let r = host.restore();
    r.drafts.contains_key(key) || r.parked.get(key).is_some_and(|p| !p.is_empty())
}

/// The shown draft's tab closed but the draft stays (it can be reopened): it's parked as a
/// tab switch parks it, its field's undo history with it, and the composer holds no draft.
pub fn park_shown_draft(host: &mut dyn RestoreHost) {
    if host.restore().shown.is_none() {
        return;
    }
    let Some(key) = host.restore_mut().shown.take() else { return };
    host.swap_composer_mode_draft(Some(key.as_str()), None);
    let edit = host.composer().clone();
    let (text, parked) = take_composer_draft(host);
    host.restore_mut().edits.insert(key.clone(), edit);
    park_draft(host, key, text, parked);
    let composer = host.composer_mut();
    let focused = composer.focused;
    *composer = TextEdit::new("");
    composer.focused = focused;
}

/// The shown draft is gone (its tab closed or moved to another window): the composer empties
/// and holds no draft until the next shows.
pub fn drop_shown_draft(host: &mut dyn RestoreHost) {
    if host.restore().shown.is_some() {
        host.restore_mut().shown = None;
    }
    let _ = take_composer_draft(host);
    let composer = host.composer_mut();
    let focused = composer.focused;
    *composer = TextEdit::new("");
    composer.focused = focused;
}

/// A finished upload lands in the draft that started it, shown or not.
pub fn land_upload(host: &mut dyn RestoreHost, key: Option<&str>, r: PathAttachment) {
    match key {
        Some(k) if !composer_holds(host, k) => {
            let paths = &mut host.restore_mut().parked.entry(k.to_owned()).or_default().paths;
            if !paths.iter().any(|x| x.path == r.path) {
                paths.push(r);
            }
        }
        _ => host.add_attach_path(r),
    }
}
