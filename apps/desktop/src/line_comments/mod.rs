//! Comments on lines of a file, as Desktop's Files and Changes views write them
//! (`createLineCommentControllerV2`, `gui-plugins/src/file/view.tsx:327-418`,
//! `session-ui/src/v2/components/session-review-file-preview-v2.tsx:118-208`, and the narrow
//! review's `SessionReview`): lines selected in a file's code or diff open a comment box under
//! them; a comment goes to the composer as a card (`composer.attach`), shows under its lines in
//! every view of that file, and can be edited or deleted there. The prompt that sends it carries
//! it as a file of those lines and a line of its text (`opencode_kit::model::FileComment`), and
//! clears the session's comments (`submit.ts:413-417`).
//!
//! A session's comments (`comments.tsx`) and its comment focus hand-off (`focus`) are the app's;
//! each view keeps its own mode per file (`machine`).

pub mod focus;
pub mod host;
pub mod machine;
pub mod saved;
pub mod view;

use std::collections::HashMap;

use rune::machine::{Attempt, Seq};
use rune::{TextEdit, Tx};
use rune_kit::diff::Side;

use opencode_kit::model::{CommentOrigin, FileComment, PromptComment};

pub use machine::{Mode, Range};

use crate::App;
use crate::quote::comment::{self as field, Config, FieldEffect, FieldEvent, Mention};

/// A saved comment on lines of a file (`LineComment`, `composer/schema.ts`).
#[derive(Clone, Debug, PartialEq)]
pub struct LineComment {
    pub id: String,
    pub file: String,
    pub selection: Range,
    pub comment: String,
    /// When it was written, in ms since the epoch.
    pub time: i64,
}

/// The view a mode belongs to: the Files tab, the wide Changes preview, or the narrow Changes
/// list (whose files share one set of modes, `transitionReviewCommentModes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    Files,
    Review,
    Narrow,
}

impl Surface {
    fn origin(self) -> CommentOrigin {
        if self == Surface::Files { CommentOrigin::File } else { CommentOrigin::Review }
    }
}

/// Where a comment box stands: a session's file in a view.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Spot {
    pub session: String,
    pub surface: Surface,
    pub path: String,
}

/// One session's comments (`createCommentSessionState`).
#[derive(Default)]
pub struct SessionComments {
    /// In the order they were written (`time`).
    pub list: Vec<LineComment>,
    pub focus: focus::Focus,
    modes: HashMap<Surface, HashMap<String, Mode>>,
}

impl SessionComments {
    pub fn mode(&self, surface: Surface, path: &str) -> &Mode {
        static IDLE: Mode = Mode::Idle;
        self.modes.get(&surface).and_then(|m| m.get(path)).unwrap_or(&IDLE)
    }

    pub fn on_file<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a LineComment> + 'a {
        self.list.iter().filter(move |c| c.file == path)
    }
}

/// Every session's line comments.
#[derive(Default)]
pub struct LineComments {
    pub sessions: HashMap<String, SessionComments>,
    next: u64,
    focus_seq: Seq,
    field_seq: Seq,
    /// The open editor's `@` search, and the editor it's for.
    pub(crate) mention_fetch: rune::Latest<(Attempt, Option<Vec<String>>)>,
    mention_for: Option<Spot>,
    /// Takes an edit that lands after its editor closed.
    pub(crate) spare: TextEdit,
    /// The composer card the user opened last (`activeContextID`), lit until clicked again.
    pub active_card: Option<u64>,
    /// A box opened or closed under a pointer that hasn't moved since: the gutter's `+` hides
    /// until it does (`LineNotes::plus_quiet`).
    pub plus_quiet: bool,
    /// What the database keeps of every session's list (`saved`).
    pub(crate) saved: saved::Saved,
}

impl LineComments {
    pub fn session(&self, sid: &str) -> Option<&SessionComments> {
        self.sessions.get(sid)
    }

    /// What session `sid`'s comments and every view's boxes show, for a view whose rows are
    /// built ahead of their layout (the narrow Changes list).
    pub fn version(&self, sid: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        if let Some(s) = self.sessions.get(sid) {
            for c in &s.list {
                (&c.id, &c.file, c.selection, &c.comment).hash(&mut h);
            }
            let mut modes: Vec<_> = s.modes.iter().flat_map(|(surface, m)| m.iter().map(move |(p, mode)| (*surface as u8, p, mode))).collect();
            modes.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
            for (surface, path, mode) in modes {
                (surface, path).hash(&mut h);
                host::hash_mode(mode, &mut h);
            }
        }
        h.finish()
    }

    pub fn mode(&self, spot: &Spot) -> &Mode {
        static IDLE: Mode = Mode::Idle;
        self.sessions.get(&spot.session).map_or(&IDLE, |s| s.mode(spot.surface, &spot.path))
    }

    fn mode_mut(&mut self, spot: &Spot) -> Option<&mut Mode> {
        self.sessions.get_mut(&spot.session)?.modes.get_mut(&spot.surface)?.get_mut(&spot.path)
    }

    /// The editor open at `spot`.
    pub fn draft_mut(&mut self, spot: &Spot) -> Option<&mut machine::Draft> {
        self.mode_mut(spot).and_then(Mode::draft_mut)
    }

    /// The text field of the editor open at `spot`, or the spare one.
    pub fn draft_text_mut(&mut self, spot: &Spot) -> &mut TextEdit {
        let has = self.mode_mut(spot).is_some_and(|m| m.draft().is_some());
        if !has {
            return &mut self.spare;
        }
        &mut self.mode_mut(spot).and_then(Mode::draft_mut).expect("an open editor").text
    }
}

/// A comment box closed with words in it (Cancel, Escape), kept for as long as its undo toast.
#[derive(Clone, Debug)]
pub struct Discarded {
    spot: Spot,
    selection: Range,
    comment: String,
    /// The comment it was editing; none for a new one.
    edit: Option<String>,
}

impl App {
    /// Undo on a closed comment box's toast: it opens again where it stood, its words in it and
    /// the caret after them.
    pub(crate) fn restore_discarded_comment(&mut self, d: Discarded, tx: &mut Tx) {
        match d.edit {
            Some(id) => self.line_comment(&d.spot, machine::Event::EditOpened { id, range: d.selection, text: d.comment }, tx),
            None => {
                self.line_comment(&d.spot, machine::Event::DraftOpened(d.selection), tx);
                if let Some(draft) = self.line_comments.draft_mut(&d.spot) {
                    draft.text.replace_text(d.comment);
                    let end = draft.text.text.len();
                    draft.text.set_caret(end);
                }
            }
        }
    }

    /// Steps the mode at `spot` and carries out its effects.
    pub fn line_comment(&mut self, spot: &Spot, event: machine::Event, tx: &mut Tx) {
        tx.instant();
        let before = self.line_comments.version(&spot.session);
        let modes = self.line_comments.sessions.entry(spot.session.clone()).or_default().modes.entry(spot.surface).or_default();
        let effects = if spot.surface == Surface::Narrow {
            machine::step_files(modes, &spot.path, event)
        } else {
            let mut mode = modes.remove(&spot.path).unwrap_or_default();
            let effects = machine::step(&mut mode, event).effects;
            if !matches!(mode, Mode::Idle) {
                modes.insert(spot.path.clone(), mode);
            }
            effects
        };
        // A box that opens takes the keyboard from the prompt (its field mounts focused).
        if self.line_comments.mode(spot).draft().is_some_and(|d| d.text.focused) {
            self.composer.focused = false;
        }
        for effect in effects {
            match effect {
                machine::Effect::Sync(range) => self.sync_view_selection(spot, range),
                machine::Effect::SubmitDraft { comment, selection } => self.add_line_comment(spot, selection, comment, tx),
                machine::Effect::SubmitEdit { id, comment, selection } => self.update_line_comment(spot, &id, selection, comment),
                machine::Effect::Delete { id } => self.remove_line_comment(&spot.session, &id),
                machine::Effect::Discarded { comment, selection, edit } => {
                    let discarded = Discarded { spot: spot.clone(), selection, comment, edit };
                    crate::undo::offer(self, opencode_i18n::t!("rune.undo.commentDiscarded"), crate::undo::Undoable::Comment(discarded));
                }
            }
        }
        if self.line_comments.version(&spot.session) != before {
            self.line_comments.plus_quiet = true;
        }
    }

    /// The editor field's own event at `spot` (keys, buttons, the `@` list).
    pub fn line_comment_field(&mut self, spot: &Spot, event: FieldEvent, tx: &mut Tx) -> bool {
        let editing = matches!(self.line_comments.mode(spot), Mode::Editing { .. });
        let LineComments { sessions, field_seq, .. } = &mut *self.line_comments;
        let mode = sessions.get_mut(&spot.session).and_then(|s| s.modes.get_mut(&spot.surface)).and_then(|m| m.get_mut(&spot.path));
        let Some(draft) = mode.and_then(Mode::draft_mut) else { return false };
        let (effects, handled) = field::field_step(field_seq, &mut draft.field, event);
        for effect in effects {
            match effect {
                FieldEffect::Search { attempt, query, due } => {
                    tx.wake_at(due);
                    self.line_comments.mention_for = Some(spot.clone());
                    if let Some(items) = crate::quote::editor::search_paths(self, attempt, query, |s| &mut s.line_comments.mention_fetch) {
                        self.line_comment_field(spot, FieldEvent::Loaded { attempt, items }, tx);
                    }
                }
                FieldEffect::Insert { path, token } => {
                    if let Some(d) = self.line_comments.mode_mut(spot).and_then(Mode::draft_mut) {
                        d.text.set_selection(token.start, token.end);
                        d.text.insert(&format!("@{path} "));
                    }
                }
                FieldEffect::Submit { .. } => {
                    self.line_comment(spot, if editing { machine::Event::EditSubmitted } else { machine::Event::DraftSubmitted }, tx);
                }
                FieldEffect::Cancel => {
                    self.line_comment(spot, if editing { machine::Event::EditCancelled } else { machine::Event::DraftCancelled }, tx);
                }
            }
        }
        handled
    }

    /// What the editor's field reads at `spot` (`LineCommentEditorConfig`): no alternate send,
    /// and an empty draft doesn't submit.
    pub fn line_comment_config(&self, spot: &Spot) -> Config {
        let blank = self.line_comments.mode(spot).draft().is_none_or(|d| d.text.text.trim().is_empty());
        Config { alternate: None, submitting: false, allow_empty: false, blank }
    }

    /// A view's lines: the mode holds the view's selection (`sync-selected`). The lines a view
    /// already shows stay as they are, so a gesture still under way goes on.
    fn sync_view_selection(&mut self, spot: &Spot, range: Option<Range>) {
        if host::shown(self, spot) == range {
            return;
        }
        match spot.surface {
            Surface::Files => {
                if let Some(d) = self.files.sessions.get_mut(&spot.session).and_then(|w| w.docs.get_mut(&spot.path)) {
                    d.set_selection(range.map(|r| crate::code_view::FileLineSelection { start: r.start, end: r.end }));
                }
            }
            Surface::Review | Surface::Narrow => {
                let Some(sr) = self.review.sessions.get_mut(&spot.session) else { return };
                let diffs = if spot.surface == Surface::Review { Some(&mut sr.diffs) } else { sr.narrow.diff_mut(&spot.path) };
                let Some(diffs) = diffs else { return };
                let Some(file) = diffs.file_index(&spot.path) else { return };
                diffs.set_selection(range.map(|r| diff_selection(file, r)));
            }
        }
    }

    /// `comment.add` and `composer.attach`: the comment shows under its lines and waits in the
    /// composer as a card, and the view that wrote it reveals it (`focus.set`).
    fn add_line_comment(&mut self, spot: &Spot, selection: Range, comment: String, tx: &mut Tx) {
        self.line_comments.next += 1;
        // Unique across launches, for the store that will keep comments across them.
        let (time, id) = (*self.now_ms, format!("lc-{}-{}", *self.now_ms, self.line_comments.next));
        let preview = self.line_preview(spot, selection);
        let lc = &mut *self.line_comments;
        let session = lc.sessions.entry(spot.session.clone()).or_default();
        session.list.push(LineComment { id: id.clone(), file: spot.path.clone(), selection, comment: comment.clone(), time: time });
        lc.saved.touch(&spot.session);
        let target = focus::Target { file: spot.path.clone(), id: id.clone() };
        for effect in focus::step(&mut lc.focus_seq, &mut session.focus, focus::Event::Set(Some(target), tx.clock())) {
            let focus::Effect::WakeAt(due) = effect;
            tx.wake_at(due);
        }
        self.quote.push(FileComment { path: spot.path.clone(), lines: Some(selection.ordered()), comment, preview, origin: Some(spot.surface.origin()), id: Some(id) });
    }

    /// `comment.update` and `composer.update`.
    fn update_line_comment(&mut self, spot: &Spot, id: &str, selection: Range, comment: String) {
        let preview = self.line_preview(spot, selection);
        if let Some(c) = self.line_comments.sessions.get_mut(&spot.session).and_then(|s| s.list.iter_mut().find(|c| c.id == id)) {
            c.comment = comment.clone();
            self.line_comments.saved.touch(&spot.session);
        }
        for item in &mut self.quote.items {
            if let PromptComment::File(f) = &mut item.comment
                && f.id.as_deref() == Some(id)
            {
                f.comment = comment.clone();
                if preview.is_some() {
                    f.preview = preview.clone();
                }
            }
        }
    }

    /// `comment.remove` and `composer.detach`.
    pub fn remove_line_comment(&mut self, sid: &str, id: &str) {
        let lc = &mut *self.line_comments;
        let Some(session) = lc.sessions.get_mut(sid) else { return };
        if let Some(gone) = session.list.iter().position(|c| c.id == id).map(|i| session.list.remove(i)) {
            let _ = focus::step(&mut lc.focus_seq, &mut session.focus, focus::Event::Removed(focus::Target { file: gone.file, id: gone.id }));
            lc.saved.touch(sid);
        }
        self.quote.items.retain(|q| q.comment.file().is_none_or(|f| f.id.as_deref() != Some(id)));
    }

    /// A sent prompt took the session's comments with it (`input.comments.clear()`).
    pub fn clear_line_comments(&mut self, sid: &str) {
        let lc = &mut *self.line_comments;
        if let Some(session) = lc.sessions.get_mut(sid) {
            session.list.clear();
            let _ = focus::step(&mut lc.focus_seq, &mut session.focus, focus::Event::Reset);
            lc.saved.touch(sid);
        }
    }

    /// The first two of the commented lines (`previewSelectedLines`): from the file's text in
    /// Files, from the side's text in a diff.
    fn line_preview(&self, spot: &Spot, r: Range) -> Option<String> {
        let (a, b) = r.ordered();
        let lines: Vec<String> = match spot.surface {
            Surface::Files => {
                let doc = self.files.sessions.get(&spot.session)?.docs.get(&spot.path)?;
                (a.max(1)..=b).take(2).map(|n| doc.line(n).unwrap_or("").to_owned()).collect()
            }
            Surface::Review | Surface::Narrow => {
                let sr = self.review.sessions.get(&spot.session)?;
                let diffs = if spot.surface == Surface::Review { &sr.diffs } else { sr.narrow.diff(&spot.path)? };
                let f = diffs.file_index(&spot.path)?;
                let side = r.anchor_side();
                (a.max(1)..=b).take(2).map(|n| diffs.side_line(f, side, n).unwrap_or("").to_owned()).collect()
            }
        };
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// A composer card's click (`toggleContext`, `openComment`): it lights or goes dark, and the
    /// view the comment was written in opens its file and reveals it.
    pub fn open_line_comment_card(&mut self, key: u64, tx: &mut Tx) {
        tx.instant();
        self.line_comments.active_card = if self.line_comments.active_card == Some(key) { None } else { Some(key) };
        let Some(f) = self.quote.items.iter().find(|q| q.key == key).and_then(|q| q.comment.file()).cloned() else { return };
        let (Some(id), Some(sid)) = (f.id.clone(), self.active_session_id().map(str::to_owned)) else { return };
        match f.origin {
            Some(CommentOrigin::Review) => self.open_review_for_session(&sid, None, None, Some(&f.path), None, tx),
            _ => self.open_workspace_file_for_session(&sid, &f.path, false, None, false, tx),
        }
        let lc = &mut *self.line_comments;
        let session = lc.sessions.entry(sid).or_default();
        for effect in focus::step(&mut lc.focus_seq, &mut session.focus, focus::Event::Opened(focus::Target { file: f.path, id }, tx.clock())) {
            let focus::Effect::WakeAt(due) = effect;
            tx.wake_at(due);
        }
    }

    /// The composer card's remove (`onContextRemove`): the comment goes too.
    pub fn remove_line_comment_card(&mut self, key: u64) {
        let Some(id) = self.quote.items.iter().find(|q| q.key == key).and_then(|q| q.comment.file()).and_then(|f| f.id.clone()) else {
            self.quote.remove(key);
            return;
        };
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.remove_line_comment(&sid, &id);
        if self.line_comments.active_card == Some(key) {
            self.line_comments.active_card = None;
        }
    }
}

/// A comment's lines as a diff's selection of file `file`.
pub fn diff_selection(file: usize, r: Range) -> rune_kit::diff::LineRange {
    use rune_kit::diff::{LineRange, LineRef};
    let side = r.side.unwrap_or(Side::New);
    LineRange { file, start: LineRef::new(side, r.start), end: LineRef::new(r.end_side.unwrap_or(side), r.end) }
}

/// Lands the editors' `@` searches, reveals a comment a card or a new comment asked for, and
/// lets a lapsed request go.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some((attempt, items)) = s.line_comments.mention_fetch.take()
        && let Some(spot) = s.line_comments.mention_for.clone()
    {
        s.line_comment_field(&spot, items.map_or(FieldEvent::Failed { attempt }, |items| FieldEvent::Loaded { attempt, items }), tx);
        changed = true;
    }
    let now = tx.clock();
    let spots: Vec<Spot> = s
        .line_comments
        .sessions
        .iter()
        .flat_map(|(sid, c)| c.modes.iter().flat_map(move |(surface, m)| m.iter().map(move |(path, mode)| (sid, *surface, path, mode))))
        .filter_map(|(sid, surface, path, mode)| match mode.draft().map(|d| &d.field.mention) {
            Some(Mention::Loading { due, .. }) if now + 1e-9 >= *due => Some(Spot { session: sid.clone(), surface, path: path.clone() }),
            _ => None,
        })
        .collect();
    for spot in spots {
        let attempt = s.line_comments.mode(&spot).draft().map(|d| d.field.attempt).unwrap_or_default();
        s.line_comment_field(&spot, FieldEvent::Failed { attempt }, tx);
        changed = true;
    }
    changed | host::reveal(s, tx)
}
