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
pub mod selection;
pub mod view;

use std::collections::HashMap;
use std::rc::Rc;

use opencode_kit::model::{CommentOrigin, FileComment, PromptComment};
use opencode_ui::popup::Menu;
use rune::machine::{Attempt, Seq};
use rune::{El, TextEdit, Tx, Waker};
use rune_kit::diff::Side;
use serde_json::Value;

pub use machine::{Mode, Range};
pub use saved::poll as poll_saved;

use crate::quote::QuoteHost;
use crate::quote::comment::{self as field, Config, FieldEffect, FieldEvent, Mention};
use crate::quote::editor::PathSearch;

/// Callback invoked when a line-comment card's popup menu closes.
pub type CommentMenuPick = Rc<dyn Fn(&mut dyn LineCommentsHost, Option<&str>, &mut Tx)>;

/// Callback invoked once persisted comment rows have been read off the UI thread.
pub type SavedCommentsLoaded = Box<dyn FnOnce(Vec<(String, String)>) + Send>;

/// Object-safe host boundary for code and diff line comments across Files, wide Review diffs,
/// and narrow Changes.
///
/// Extends [`QuoteHost`] because line comments share the composer's pending comment cards
/// ([`crate::quote::Quotes`]), composer focus, spellcheck setting, and `@` file path search.
pub trait LineCommentsHost: QuoteHost {
    /// Line-comments state (tracked read).
    fn line_comments(&self) -> &LineComments;
    /// Line-comments state (tracked write — callers only invoke when mutating).
    fn line_comments_mut(&mut self) -> &mut LineComments;

    /// Currently active session ID, when on a session route.
    fn active_session_id(&self) -> Option<&str>;

    /// Current wall-clock timestamp in epoch milliseconds (used for comment ids and ordering).
    fn now_ms(&self) -> i64 {
        0
    }

    /// Active UI zoom factor (scales vertical drag deltas on the narrow editor's resize grip).
    fn zoom(&self) -> f32 {
        1.0
    }

    /// Optional window waker for background saved-comment loads and `@` mention searches.
    fn waker(&self) -> Option<Waker> {
        None
    }

    /// The lines currently highlighted in the code or diff viewer at `spot`.
    fn view_selection(&self, spot: &Spot) -> Option<Range> {
        let _ = spot;
        None
    }

    /// Updates the highlighted lines in the code or diff viewer at `spot`.
    fn set_view_selection(&mut self, spot: &Spot, range: Option<Range>) {
        let _ = (spot, range);
    }

    /// Reads up to two preview lines (`previewSelectedLines`) from the code or diff viewer at `spot`.
    fn line_preview(&self, spot: &Spot, range: Range) -> Option<String> {
        let _ = (spot, range);
        None
    }

    /// Which comment surface of session `sid` is currently displaying `path`, if any.
    fn showing_surface(&self, sid: &str, path: &str) -> Option<Surface> {
        let _ = (sid, path);
        None
    }

    /// Scrolls the viewer at `spot` so `range` is visible.
    fn scroll_to_lines(&mut self, spot: &Spot, range: Range, tx: &mut Tx) {
        let _ = (spot, range, tx);
    }

    /// Opens the surface (`CommentOrigin::Review` -> Changes preview, otherwise Files tab) for
    /// `path` in session `sid` when a composer file-comment card is clicked.
    fn open_comment_source(&mut self, sid: &str, path: &str, origin: Option<CommentOrigin>, tx: &mut Tx) {
        let _ = (sid, path, origin, tx);
    }

    /// Offers an undo toast when a comment box closes with non-trivial typed words (`Discarded`).
    fn offer_discarded_comment(&mut self, title: &'static str, discarded: Discarded) {
        let _ = (title, discarded);
    }

    /// Wraps a comment card's `...` trigger button (`trigger`) as a popup menu trigger anchored at
    /// `anchor` that opens `menu` and forwards the picked item id (`Some("edit")`,
    /// `Some("delete")`, or `None`) to `on_pick`.
    ///
    /// Hosts implementing [`opencode_ui::popup::PopupHost`] can delegate in one line to
    /// [`view::popup_menu_button`].
    fn wrap_comment_menu_button(
        &self,
        trigger: El,
        anchor: String,
        menu: Menu,
        on_pick: CommentMenuPick,
    ) -> El {
        drop((anchor, menu, on_pick));
        trigger
    }

    /// Whether persistent storage is initialized and ready for loading/saving comments.
    fn storage_ready(&self) -> bool {
        false
    }

    /// Loads all persisted `(key, json_value)` rows in `namespace` (`saved::NAMESPACE`) off the
    /// UI thread and invokes `on_loaded` once ready.
    fn load_saved_comments(&self, namespace: &'static str, on_loaded: SavedCommentsLoaded) {
        drop((namespace, on_loaded));
    }

    /// Persists a session's encoded comments JSON under `(namespace, key)`.
    fn save_comments_json(&self, namespace: &'static str, key: &str, value: &Value) {
        let _ = (namespace, key, value);
    }

    /// Deletes persisted comments under `(namespace, key)` when a session's comment list empties.
    fn delete_saved_comments(&self, namespace: &'static str, key: &str) {
        let _ = (namespace, key);
    }
}

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
    pub fn origin(self) -> CommentOrigin {
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
    pub selection: selection::State,
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
    /// Connects the window waker to the line-comment editor's `@` file search.
    pub fn set_waker(&mut self, waker: Waker) {
        self.mention_fetch.set_waker(waker);
    }

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
        self.sessions
            .get_mut(&spot.session)
            .and_then(|s| s.modes.get_mut(&spot.surface))
            .and_then(|m| m.get_mut(&spot.path))
            .and_then(Mode::draft_mut)
            .map_or(&mut self.spare, |d| &mut d.text)
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

/// Connects the window waker to the line-comment editor's `@` mention search.
pub fn setup(host: &mut dyn LineCommentsHost, waker: Waker) {
    host.line_comments_mut().set_waker(waker);
}

/// Undo on a closed comment box's toast: it opens again where it stood, its words in it and
/// the caret after them.
pub fn restore_discarded_comment(host: &mut dyn LineCommentsHost, d: Discarded, tx: &mut Tx) {
    match d.edit {
        Some(id) => line_comment(host, &d.spot, machine::Event::EditOpened { id, range: d.selection, text: d.comment }, tx),
        None => {
            line_comment(host, &d.spot, machine::Event::DraftOpened(d.selection), tx);
            if let Some(draft) = host.line_comments_mut().draft_mut(&d.spot) {
                draft.text.replace_text(d.comment);
                let end = draft.text.text.len();
                draft.text.set_caret(end);
            }
        }
    }
}

/// Steps the mode at `spot` and carries out its effects.
pub fn line_comment(host: &mut dyn LineCommentsHost, spot: &Spot, event: machine::Event, tx: &mut Tx) {
    tx.instant();
    let before = host.line_comments().version(&spot.session);
    let effects = {
        let modes = host
            .line_comments_mut()
            .sessions
            .entry(spot.session.clone())
            .or_default()
            .modes
            .entry(spot.surface)
            .or_default();
        if spot.surface == Surface::Narrow {
            machine::step_files(modes, &spot.path, event)
        } else {
            let mut mode = modes.remove(&spot.path).unwrap_or_default();
            let effects = machine::step(&mut mode, event).effects;
            if !matches!(mode, Mode::Idle) {
                modes.insert(spot.path.clone(), mode);
            }
            effects
        }
    };
    // A box that opens takes the keyboard from the prompt (its field mounts focused).
    if host.line_comments().mode(spot).draft().is_some_and(|d| d.text.focused) {
        host.set_composer_focused(false);
    }
    for effect in effects {
        match effect {
            machine::Effect::Sync(range) => sync_view_selection(host, spot, range),
            machine::Effect::SubmitDraft { comment, selection } => add_line_comment(host, spot, selection, comment, tx),
            machine::Effect::SubmitEdit { id, comment, selection } => update_line_comment(host, spot, &id, selection, comment),
            machine::Effect::Delete { id } => remove_line_comment(host, &spot.session, &id),
            machine::Effect::Discarded { comment, selection, edit } => {
                let discarded = Discarded { spot: spot.clone(), selection, comment, edit };
                host.offer_discarded_comment(opencode_i18n::t!("rune.undo.commentDiscarded"), discarded);
            }
        }
    }
    if host.line_comments().version(&spot.session) != before {
        host.line_comments_mut().plus_quiet = true;
    }
}

/// The editor field's own event at `spot` (keys, buttons, the `@` list).
pub fn line_comment_field(host: &mut dyn LineCommentsHost, spot: &Spot, event: FieldEvent, tx: &mut Tx) -> bool {
    let mode = host.line_comments().mode(spot);
    if mode.draft().is_none() {
        return false;
    }
    let editing = matches!(mode, Mode::Editing { .. });
    let (effects, handled) = {
        let LineComments { sessions, field_seq, .. } = host.line_comments_mut();
        let mode = sessions.get_mut(&spot.session).and_then(|s| s.modes.get_mut(&spot.surface)).and_then(|m| m.get_mut(&spot.path));
        let Some(draft) = mode.and_then(Mode::draft_mut) else { return false };
        field::field_step(field_seq, &mut draft.field, event)
    };
    for effect in effects {
        match effect {
            FieldEffect::Search { attempt, query, due } => {
                tx.wake_at(due);
                host.line_comments_mut().mention_for = Some(spot.clone());
                match crate::quote::editor::prepare_path_search(host, &query) {
                    PathSearch::Immediate(items) => {
                        line_comment_field(host, spot, FieldEvent::Loaded { attempt, items }, tx);
                    }
                    PathSearch::Live { server, directory } => {
                        crate::quote::editor::request_path_search(
                            &mut host.line_comments_mut().mention_fetch,
                            server,
                            directory,
                            attempt,
                            query,
                        );
                    }
                }
            }
            FieldEffect::Insert { path, token } => {
                if let Some(d) = host.line_comments_mut().mode_mut(spot).and_then(Mode::draft_mut) {
                    d.text.set_selection(token.start, token.end);
                    d.text.insert(&format!("@{path} "));
                }
            }
            FieldEffect::Submit { .. } => {
                line_comment(host, spot, if editing { machine::Event::EditSubmitted } else { machine::Event::DraftSubmitted }, tx);
            }
            FieldEffect::Cancel => {
                line_comment(host, spot, if editing { machine::Event::EditCancelled } else { machine::Event::DraftCancelled }, tx);
            }
        }
    }
    handled
}

/// What the editor's field reads at `spot` (`LineCommentEditorConfig`): no alternate send,
/// and an empty draft doesn't submit.
pub fn line_comment_config(host: &dyn LineCommentsHost, spot: &Spot) -> Config {
    let blank = host.line_comments().mode(spot).draft().is_none_or(|d| d.text.text.trim().is_empty());
    Config { alternate: None, submitting: false, allow_empty: false, blank }
}

/// A view's lines: the mode holds the view's selection (`sync-selected`). The lines a view
/// already shows stay as they are, so a gesture still under way goes on.
pub(crate) fn sync_view_selection(host: &mut dyn LineCommentsHost, spot: &Spot, range: Option<Range>) {
    if host.view_selection(spot) == range {
        return;
    }
    host.set_view_selection(spot, range);
}

/// `comment.add` and `composer.attach`: the comment shows under its lines and waits in the
/// composer as a card, and the view that wrote it reveals it (`focus.set`).
fn add_line_comment(host: &mut dyn LineCommentsHost, spot: &Spot, selection: Range, comment: String, tx: &mut Tx) {
    let time = host.now_ms();
    let preview = host.line_preview(spot, selection);
    let now = tx.clock();
    let id = {
        let lc = host.line_comments_mut();
        lc.next += 1;
        // Unique across launches, for the store that will keep comments across them.
        let id = format!("lc-{time}-{}", lc.next);
        let session = lc.sessions.entry(spot.session.clone()).or_default();
        session.list.push(LineComment { id: id.clone(), file: spot.path.clone(), selection, comment: comment.clone(), time });
        lc.saved.touch(&spot.session);
        let target = focus::Target { file: spot.path.clone(), id: id.clone() };
        for effect in focus::step(&mut lc.focus_seq, &mut session.focus, focus::Event::Set(Some(target), now)) {
            let focus::Effect::WakeAt(due) = effect;
            tx.wake_at(due);
        }
        id
    };
    host.quotes_mut().push(FileComment {
        path: spot.path.clone(),
        lines: Some(selection.ordered()),
        comment,
        preview,
        origin: Some(spot.surface.origin()),
        id: Some(id),
    });
}

/// `comment.update` and `composer.update`.
fn update_line_comment(host: &mut dyn LineCommentsHost, spot: &Spot, id: &str, selection: Range, comment: String) {
    let preview = host.line_preview(spot, selection);
    {
        let lc = host.line_comments_mut();
        if let Some(c) = lc.sessions.get_mut(&spot.session).and_then(|s| s.list.iter_mut().find(|c| c.id == id)) {
            c.comment = comment.clone();
            lc.saved.touch(&spot.session);
        }
    }
    for item in &mut host.quotes_mut().items {
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
pub fn remove_line_comment(host: &mut dyn LineCommentsHost, sid: &str, id: &str) {
    {
        let lc = host.line_comments_mut();
        let Some(session) = lc.sessions.get_mut(sid) else { return };
        if let Some(gone) = session.list.iter().position(|c| c.id == id).map(|i| session.list.remove(i)) {
            drop(focus::step(&mut lc.focus_seq, &mut session.focus, focus::Event::Removed(focus::Target { file: gone.file, id: gone.id })));
            lc.saved.touch(sid);
        }
    }
    host.quotes_mut().items.retain(|q| q.comment.file().is_none_or(|f| f.id.as_deref() != Some(id)));
}

/// A sent prompt took the session's comments with it (`input.comments.clear()`).
pub fn clear_line_comments(host: &mut dyn LineCommentsHost, sid: &str) {
    let lc = host.line_comments_mut();
    if let Some(session) = lc.sessions.get_mut(sid) {
        session.list.clear();
        drop(focus::step(&mut lc.focus_seq, &mut session.focus, focus::Event::Reset));
        lc.saved.touch(sid);
    }
}

/// A composer card's click (`toggleContext`, `openComment`): it lights or goes dark, and the
/// view the comment was written in opens its file and reveals it.
pub fn open_line_comment_card(host: &mut dyn LineCommentsHost, key: u64, tx: &mut Tx) {
    tx.instant();
    {
        let lc = host.line_comments_mut();
        lc.active_card = if lc.active_card == Some(key) { None } else { Some(key) };
    }
    let Some(f) = host.quotes().items.iter().find(|q| q.key == key).and_then(|q| q.comment.file()).cloned() else { return };
    let (Some(id), Some(sid)) = (f.id.clone(), host.active_session_id().map(str::to_owned)) else { return };
    host.open_comment_source(&sid, &f.path, f.origin, tx);
    let now = tx.clock();
    let lc = host.line_comments_mut();
    let session = lc.sessions.entry(sid).or_default();
    for effect in focus::step(&mut lc.focus_seq, &mut session.focus, focus::Event::Opened(focus::Target { file: f.path, id }, now)) {
        let focus::Effect::WakeAt(due) = effect;
        tx.wake_at(due);
    }
}

/// The composer card's remove (`onContextRemove`): the comment goes too.
pub fn remove_line_comment_card(host: &mut dyn LineCommentsHost, key: u64) {
    let Some(id) = host
        .quotes()
        .items
        .iter()
        .find(|q| q.key == key)
        .and_then(|q| q.comment.file())
        .and_then(|f| f.id.clone())
    else {
        host.quotes_mut().remove(key);
        return;
    };
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    remove_line_comment(host, &sid, &id);
    if host.line_comments().active_card == Some(key) {
        host.line_comments_mut().active_card = None;
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
pub fn poll(host: &mut dyn LineCommentsHost, tx: &mut Tx) -> bool {
    let mut changed = selection::dispatch(host, selection::Event::Tick(tx.clock()), tx);
    let fetched = {
        let lc = host.line_comments();
        lc.mention_fetch.take().and_then(|res| Some((res, lc.mention_for.clone()?)))
    };
    if let Some(((attempt, items), spot)) = fetched {
        line_comment_field(host, &spot, items.map_or(FieldEvent::Failed { attempt }, |items| FieldEvent::Loaded { attempt, items }), tx);
        changed = true;
    }
    let now = tx.clock();
    let spots: Vec<Spot> = host
        .line_comments()
        .sessions
        .iter()
        .flat_map(|(sid, c)| c.modes.iter().flat_map(move |(surface, m)| m.iter().map(move |(path, mode)| (sid, *surface, path, mode))))
        .filter_map(|(sid, surface, path, mode)| match mode.draft().map(|d| &d.field.mention) {
            Some(Mention::Loading { due, .. }) if now + 1e-9 >= *due => Some(Spot { session: sid.clone(), surface, path: path.clone() }),
            _ => None,
        })
        .collect();
    for spot in spots {
        let attempt = host.line_comments().mode(&spot).draft().map(|d| d.field.attempt).unwrap_or_default();
        line_comment_field(host, &spot, FieldEvent::Failed { attempt }, tx);
        changed = true;
    }
    changed | host::reveal(host, tx)
}
