//! The line comments of a session's file in one view: what the code and diff views ask
//! (`opencode_code_view::notes::LineNotes`), and the hand-offs a view takes part in: revealing a
//! comment a card or a new comment asked for (`comment.focus`), and showing a mode's lines
//! selected again after the view reloaded them (a changed diff, a reread file).

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use opencode_code_view::notes::{Handoff, LineNotes, diff_range};
use opencode_code_view::{CodeDocState, DiffDocState, FileLineSelection};
use rune::{El, Tx};
use rune_kit::diff::Side;
use session_view::palette::Pal;

use super::machine::{Event, Mode};
use super::selection;
use super::{LineCommentsHost, Range, Spot, Surface, focus, line_comment, sync_view_selection};

/// A view's line comments: session `session`'s, in `surface`.
///
/// Implements [`LineNotes<H>`] for any host `H: LineCommentsHost + 'static` so viewers can pass
/// `Notes::new(session_id, surface)` with no type arguments.
pub struct Notes {
    pub session: String,
    pub surface: Surface,
}

impl Notes {
    pub fn new(session: &str, surface: Surface) -> Rc<Notes> {
        Rc::new(Notes { session: session.to_owned(), surface })
    }

    fn spot(&self, path: &str) -> Spot {
        Spot { session: self.session.clone(), surface: self.surface, path: path.to_owned() }
    }
}

fn text_selection_impl(
    notes: &Notes,
    host: &mut dyn LineCommentsHost,
    viewer: &str,
    dragging: bool,
    selected: Option<(&str, Range, bool)>,
    tx: &mut Tx,
) {
    let event = if dragging {
        selection::Event::Press
    } else if let Some((path, range, backward)) = selected {
        selection::Event::Release(
            selection::Offer { spot: notes.spot(path), range, viewer: viewer.to_owned(), backward },
            tx.clock(),
        )
    } else {
        selection::Event::Clear
    };
    selection::dispatch(host, event, tx);
}

fn version_impl(notes: &Notes, host: &dyn LineCommentsHost, path: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    let lc = host.line_comments();
    if let Some(session) = lc.session(&notes.session) {
        for c in session.on_file(path) {
            (&c.id, c.selection, &c.comment).hash(&mut h);
        }
        hash_mode(session.mode(notes.surface, path), &mut h);
    }
    (host.spellcheck(), lc.plus_quiet).hash(&mut h);
    h.finish()
}

fn commented_impl(notes: &Notes, host: &dyn LineCommentsHost, path: &str, side: Side, line: u32) -> bool {
    host.line_comments()
        .session(&notes.session)
        .is_some_and(|c| c.on_file(path).any(|c| c.selection.contains(line, Some(side))))
}

fn handoff_impl(notes: &Notes, host: &mut dyn LineCommentsHost, path: &str, h: Handoff, tx: &mut Tx) {
    let spot = notes.spot(path);
    match h {
        Handoff::Selected(r) => line_comment(host, &spot, Event::LineSelected(r), tx),
        Handoff::Ended(r) => line_comment(host, &spot, Event::SelectionEnded(r), tx),
        // Desktop's file view lets an empty box go and leaves the line lit; its diff view,
        // whose selection the mode controls, keeps both (Pierre's `pendingSingleLineUnselect`
        // ends with the controlled range).
        Handoff::Unselected(line) if notes.surface == Surface::Files => {
            line_comment(host, &spot, Event::LineSelected(None), tx);
            if matches!(host.line_comments().mode(&spot), Mode::Idle) {
                sync_view_selection(host, &spot, Some(line));
            }
        }
        Handoff::Unselected(line) => {
            sync_view_selection(host, &spot, Some(line));
            line_comment(host, &spot, Event::SelectionEnded(Some(line)), tx);
        }
    }
}

fn plus_impl(notes: &Notes, host: &mut dyn LineCommentsHost, path: &str, side: Option<Side>, line: u32, tx: &mut Tx) {
    let spot = notes.spot(path);
    let current = host.line_comments().mode(&spot).gutter_selected();
    let range = current.filter(|r| r.contains(line, side)).unwrap_or(Range::line(line, side));
    line_comment(host, &spot, Event::DraftOpened(range), tx);
}

fn pointer_moved_impl(host: &mut dyn LineCommentsHost, tx: &mut Tx) {
    if !host.line_comments().plus_quiet {
        tx.live_only();
        return;
    }
    host.line_comments_mut().plus_quiet = false;
}

impl<H: LineCommentsHost + 'static> LineNotes<H> for Notes {
    fn text_selection(&self, s: &mut H, viewer: &str, dragging: bool, selected: Option<(&str, Range, bool)>, tx: &mut Tx) {
        text_selection_impl(self, s, viewer, dragging, selected, tx);
    }

    fn version(&self, s: &H, path: &str) -> u64 {
        version_impl(self, s, path)
    }

    fn commented(&self, s: &H, path: &str, side: Side, line: u32) -> bool {
        commented_impl(self, s, path, side, line)
    }

    fn annotations(&self, s: &H, p: &Pal, path: &str, side: Side, line: u32) -> Vec<El> {
        super::view::annotations::<H>(s, p, &self.spot(path), side, line)
    }

    fn handoff(&self, s: &mut H, path: &str, h: Handoff, tx: &mut Tx) {
        handoff_impl(self, s, path, h, tx);
    }

    fn plus(&self, s: &mut H, path: &str, side: Option<Side>, line: u32, tx: &mut Tx) {
        plus_impl(self, s, path, side, line, tx);
    }

    fn plus_quiet(&self, s: &H) -> bool {
        s.line_comments().plus_quiet
    }

    fn pointer_moved(&self, s: &mut H, tx: &mut Tx) {
        pointer_moved_impl(s, tx);
    }
}

pub(super) fn hash_mode(mode: &Mode, h: &mut impl Hasher) {
    std::mem::discriminant(mode).hash(h);
    mode.selected().hash(h);
    if let Some(d) = mode.draft() {
        (&d.text.text, d.text.range(), d.text.focused, d.height.map(f32::to_bits)).hash(h);
        (d.field.mention.items(), d.field.mention.active()).hash(h);
    }
    if let Mode::Viewing { id, .. } | Mode::Editing { id, .. } = mode {
        id.hash(h);
    }
}

/// Reveals the comment a session asked for in a view that shows its file (`focusComment`, the
/// review's focus driver), and acknowledges it; lets a lapsed request go; and shows each mode's
/// lines selected in a view that lost them to a reload.
pub fn reveal(host: &mut dyn LineCommentsHost, tx: &mut Tx) -> bool {
    let mut changed = false;
    let now = tx.clock();
    let pending: Vec<(String, focus::Target, rune::machine::Attempt, f64)> = host
        .line_comments()
        .sessions
        .iter()
        .filter_map(|(sid, c)| Some((sid.clone(), c.focus.target()?.clone(), c.focus.deadline()?.0, c.focus.deadline()?.1)))
        .collect();
    for (sid, target, attempt, due) in pending {
        if now + 1e-3 >= due {
            let lc = host.line_comments_mut();
            if let Some(c) = lc.sessions.get_mut(&sid) {
                drop(focus::step(&mut lc.focus_seq, &mut c.focus, focus::Event::TimedOut { attempt, now }));
            }
            changed = true;
            continue;
        }
        let Some(surface) = host.showing_surface(&sid, &target.file) else { continue };
        let Some(range) = host
            .line_comments()
            .session(&sid)
            .and_then(|c| c.list.iter().find(|c| c.id == target.id))
            .map(|c| c.selection)
        else {
            continue;
        };
        let spot = Spot { session: sid.clone(), surface, path: target.file.clone() };
        line_comment(host, &spot, Event::CommentFocused { id: target.id.clone(), range }, tx);
        host.scroll_to_lines(&spot, range, tx);
        let lc = host.line_comments_mut();
        if let Some(c) = lc.sessions.get_mut(&sid) {
            drop(focus::step(&mut lc.focus_seq, &mut c.focus, focus::Event::Set(None, now)));
        }
        changed = true;
    }
    resync(host);
    changed
}

/// A view that reloaded its lines (a changed diff, a reread file) shows the mode's lines again,
/// as Desktop hands its viewer `selectedLines` on every render.
fn resync(host: &mut dyn LineCommentsHost) {
    let wanted: Vec<(Spot, Range)> = host
        .line_comments()
        .sessions
        .iter()
        .flat_map(|(sid, c)| {
            c.modes.iter().flat_map(move |(surface, m)| {
                m.iter().filter_map(move |(path, mode)| {
                    Some((Spot { session: sid.clone(), surface: *surface, path: path.clone() }, mode.selected()?))
                })
            })
        })
        .collect();
    for (spot, range) in wanted {
        if host.view_selection(&spot) != Some(range) {
            host.set_view_selection(&spot, Some(range));
        }
    }
}

/// Reads the highlighted line range of a [`CodeDocState`] in [`Range`] form.
pub fn code_doc_selection(doc: &CodeDocState) -> Option<Range> {
    doc.selection().map(|l| Range { start: l.start, end: l.end, side: None, end_side: None })
}

/// Applies `range` to a [`CodeDocState`]'s line selection.
pub fn set_code_doc_selection(doc: &mut CodeDocState, range: Option<Range>) {
    doc.set_selection(range.map(|r| FileLineSelection { start: r.start, end: r.end }));
}

/// Formats up to two preview lines (`previewSelectedLines`) from a [`CodeDocState`].
pub fn code_doc_preview(doc: &CodeDocState, range: Range) -> Option<String> {
    let (a, b) = range.ordered();
    let lines: Vec<String> = (a.max(1)..=b).take(2).map(|n| doc.line(n).unwrap_or("").to_owned()).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Scrolls a Files viewer [`CodeDocState`] to the start of `range`.
pub fn scroll_code_doc(doc: &mut CodeDocState, session: &str, path: &str, range: Range, tx: &mut Tx) {
    let key = format!("file-code-{session}-{path}-scroll");
    doc.scroll_to_line(&key, range.ordered().0, false, tx);
}

/// Reads the highlighted line range of a [`DiffDocState`] in [`Range`] form.
pub fn diff_doc_selection(diffs: &DiffDocState) -> Option<Range> {
    diffs.selection().map(diff_range)
}

/// Applies `range` to a [`DiffDocState`]'s selection for file `path`.
pub fn set_diff_doc_selection(diffs: &mut DiffDocState, path: &str, range: Option<Range>) {
    let Some(file) = diffs.file_index(path) else { return };
    diffs.set_selection(range.map(|r| super::diff_selection(file, r)));
}

/// Formats up to two preview lines (`previewSelectedLines`) from a [`DiffDocState`] for `path`.
pub fn diff_doc_preview(diffs: &DiffDocState, path: &str, range: Range) -> Option<String> {
    let (a, b) = range.ordered();
    let f = diffs.file_index(path)?;
    let side = range.anchor_side();
    let lines: Vec<String> = (a.max(1)..=b).take(2).map(|n| diffs.side_line(f, side, n).unwrap_or("").to_owned()).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Scrolls a Review viewer [`DiffDocState`] to the start of `range` in `path`.
pub fn scroll_diff_doc(diffs: &mut DiffDocState, path: &str, range: Range, tx: &mut Tx) {
    let Some(f) = diffs.file_index(path) else { return };
    diffs.scroll_to_line(f, rune_kit::diff::LineRef::new(range.side.unwrap_or(Side::New), range.ordered().0), tx);
}
