//! The line comments of a session's file in one view: what the code and diff views ask
//! (`code_view::notes::LineNotes`), and the hand-offs a view takes part in: revealing a
//! comment a card or a new comment asked for (`comment.focus`), and showing a mode's lines
//! selected again after the view reloaded them (a changed diff, a reread file).

use std::hash::{Hash, Hasher};

use rune::{El, Tx};
use rune_kit::diff::Side;

use super::machine::{Event, Mode};
use super::{Range, Spot, Surface, focus};
use crate::App;
use crate::code_view::notes::{Handoff, LineNotes};
use crate::palette::Pal;

/// A view's line comments: session `session`'s, in `surface`.
pub struct Notes {
    pub session: String,
    pub surface: Surface,
}

impl Notes {
    pub fn new(session: &str, surface: Surface) -> std::rc::Rc<Notes> {
        std::rc::Rc::new(Notes { session: session.to_owned(), surface })
    }

    fn spot(&self, path: &str) -> Spot {
        Spot { session: self.session.clone(), surface: self.surface, path: path.to_owned() }
    }
}

impl LineNotes<App> for Notes {
    fn version(&self, s: &App, path: &str) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        if let Some(session) = s.line_comments.session(&self.session) {
            for c in session.on_file(path) {
                (&c.id, c.selection, &c.comment).hash(&mut h);
            }
            hash_mode(session.mode(self.surface, path), &mut h);
        }
        (s.settings.spellcheck, s.line_comments.plus_quiet).hash(&mut h);
        h.finish()
    }

    fn commented(&self, s: &App, path: &str, side: Side, line: u32) -> bool {
        s.line_comments.session(&self.session).is_some_and(|c| c.on_file(path).any(|c| c.selection.contains(line, Some(side))))
    }

    fn annotations(&self, s: &App, p: &Pal, path: &str, side: Side, line: u32) -> Vec<El> {
        super::view::annotations(s, p, &self.spot(path), side, line)
    }

    fn handoff(&self, s: &mut App, path: &str, h: Handoff, tx: &mut Tx) {
        let spot = self.spot(path);
        match h {
            Handoff::Selected(r) => s.line_comment(&spot, Event::LineSelected(r), tx),
            Handoff::Ended(r) => s.line_comment(&spot, Event::SelectionEnded(r), tx),
            // Desktop's file view lets an empty box go and leaves the line lit; its diff view,
            // whose selection the mode controls, keeps both (Pierre's `pendingSingleLineUnselect`
            // ends with the controlled range).
            Handoff::Unselected(line) if self.surface == Surface::Files => {
                s.line_comment(&spot, Event::LineSelected(None), tx);
                if matches!(s.line_comments.mode(&spot), Mode::Idle) {
                    s.sync_view_selection(&spot, Some(line));
                }
            }
            Handoff::Unselected(line) => {
                s.sync_view_selection(&spot, Some(line));
                s.line_comment(&spot, Event::SelectionEnded(Some(line)), tx);
            }
        }
    }

    fn plus(&self, s: &mut App, path: &str, side: Option<Side>, line: u32, tx: &mut Tx) {
        let spot = self.spot(path);
        let current = s.line_comments.mode(&spot).gutter_selected();
        let range = current.filter(|r| r.contains(line, side)).unwrap_or(Range::line(line, side));
        s.line_comment(&spot, Event::DraftOpened(range), tx);
    }

    fn plus_quiet(&self, s: &App) -> bool {
        s.line_comments.plus_quiet
    }

    fn pointer_moved(&self, s: &mut App, tx: &mut Tx) {
        if !s.line_comments.plus_quiet {
            tx.live_only();
            return;
        }
        s.line_comments.plus_quiet = false;
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
pub fn reveal(s: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    let now = tx.clock();
    let pending: Vec<(String, focus::Target, rune::machine::Attempt, f64)> = s
        .line_comments
        .sessions
        .iter()
        .filter_map(|(sid, c)| Some((sid.clone(), c.focus.target()?.clone(), c.focus.deadline()?.0, c.focus.deadline()?.1)))
        .collect();
    for (sid, target, attempt, due) in pending {
        if now + 1e-3 >= due {
            if let Some(c) = s.line_comments.sessions.get_mut(&sid) {
                let _ = focus::step(&mut s.line_comments.focus_seq, &mut c.focus, focus::Event::TimedOut { attempt, now });
            }
            changed = true;
            continue;
        }
        let Some(surface) = showing(s, &sid, &target.file) else { continue };
        let Some(range) = s.line_comments.session(&sid).and_then(|c| c.list.iter().find(|c| c.id == target.id)).map(|c| c.selection) else { continue };
        let spot = Spot { session: sid.clone(), surface, path: target.file.clone() };
        s.line_comment(&spot, Event::CommentFocused { id: target.id.clone(), range }, tx);
        scroll_to(s, &spot, range, tx);
        if let Some(c) = s.line_comments.sessions.get_mut(&sid) {
            let _ = focus::step(&mut s.line_comments.focus_seq, &mut c.focus, focus::Event::Set(None, now));
        }
        changed = true;
    }
    resync(s);
    changed
}

/// The view of session `sid` showing file `path` now, if one does.
fn showing(s: &App, sid: &str, path: &str) -> Option<Surface> {
    if s.active_file_tab().is_some_and(|(session, p)| session == sid && p.as_deref() == Some(path)) {
        return Some(Surface::Files);
    }
    let sr = s.review.sessions.get(sid)?;
    if s.narrow() {
        return sr.narrow.is_open(path).then_some(Surface::Narrow);
    }
    (s.is_review_active() && sr.active_path.as_deref() == Some(path)).then_some(Surface::Review)
}

/// Brings the comment's lines into view.
fn scroll_to(s: &mut App, spot: &Spot, r: Range, tx: &mut Tx) {
    match spot.surface {
        Surface::Files => {
            let key = format!("file-code-{}-{}-scroll", spot.session, spot.path);
            if let Some(d) = s.files.sessions.get_mut(&spot.session).and_then(|w| w.docs.get_mut(&spot.path)) {
                d.scroll_to_line(&key, r.ordered().0, false, tx);
            }
        }
        Surface::Review => {
            if let Some(sr) = s.review.sessions.get_mut(&spot.session)
                && let Some(f) = sr.diffs.file_index(&spot.path)
            {
                sr.diffs.scroll_to_line(f, rune_kit::diff::LineRef::new(r.side.unwrap_or(Side::New), r.ordered().0), tx);
            }
        }
        Surface::Narrow => {}
    }
}

/// A view that reloaded its lines (a changed diff, a reread file) shows the mode's lines again,
/// as Desktop hands its viewer `selectedLines` on every render.
fn resync(s: &mut App) {
    let wanted: Vec<(Spot, Range)> = s
        .line_comments
        .sessions
        .iter()
        .flat_map(|(sid, c)| c.modes.iter().flat_map(move |(surface, m)| m.iter().filter_map(move |(path, mode)| Some((Spot { session: sid.clone(), surface: *surface, path: path.clone() }, mode.selected()?)))))
        .collect();
    for (spot, range) in wanted {
        if shown(s, &spot) != Some(range) {
            s.sync_view_selection(&spot, Some(range));
        }
    }
}

/// The lines the view at `spot` shows selected.
pub(super) fn shown(s: &App, spot: &Spot) -> Option<Range> {
    match spot.surface {
        Surface::Files => s.files.sessions.get(&spot.session)?.docs.get(&spot.path)?.selection().map(|l| Range { start: l.start, end: l.end, side: None, end_side: None }),
        Surface::Review | Surface::Narrow => {
            let sr = s.review.sessions.get(&spot.session)?;
            let diffs = if spot.surface == Surface::Review { &sr.diffs } else { sr.narrow.diff(&spot.path)? };
            diffs.selection().map(crate::code_view::notes::diff_range)
        }
    }
}
