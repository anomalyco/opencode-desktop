//! What one file's line comments are doing in a view, as Desktop's `transitionLineComment`
//! (`session-ui/src/components/line-comment-machine.ts`) runs it: lines selected, a new
//! comment's draft under them, a saved comment opened, or a saved comment's edit. A draft and an
//! edit live in different variants, so one never overwrites the other; typed text is never
//! dropped by a selection, click or focus elsewhere in the file.
//!
//! The draft's text and its field (`@` list, keys) are the variant's own; `draft.changed` and
//! `edit.changed` are the field's edits landing in that text in place.

use std::collections::HashMap;

use rune::TextEdit;
use rune::machine::Effects;
use rune_kit::diff::Side;

use crate::quote::comment::Field;

/// Lines a selection or comment covers (Pierre's `SelectedLineRange`): from `start` to `end`,
/// either way round; in a diff, the side each end is on (`end_side` only when it differs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: u32,
    pub end: u32,
    pub side: Option<Side>,
    pub end_side: Option<Side>,
}

impl Range {
    pub fn line(n: u32, side: Option<Side>) -> Range {
        Range { start: n, end: n, side, end_side: None }
    }

    pub fn ordered(self) -> (u32, u32) {
        (self.start.min(self.end), self.start.max(self.end))
    }

    /// The side a comment on these lines shows under (`getSide`: `endSide ?? side ??
    /// "additions"`).
    pub fn anchor_side(self) -> Side {
        self.end_side.or(self.side).unwrap_or(Side::New)
    }

    /// The line a comment on these lines shows under (`Math.max(start, end)`).
    pub fn anchor_line(self) -> u32 {
        self.start.max(self.end)
    }

    /// `sameSelectedRange`.
    pub fn same(self, other: Range) -> bool {
        self.start == other.start && self.end == other.end && self.side == other.side && self.end_side.or(self.side) == other.end_side.or(other.side)
    }

    /// `lineInSelectedRange`: whether line `line` on `side` (`None` in a file) is among these.
    pub fn contains(self, line: u32, side: Option<Side>) -> bool {
        let (start, end) = self.ordered();
        if line < start || line > end {
            return false;
        }
        let Some(side) = side else { return true };
        let (first, last) = (self.side, self.end_side.or(self.side));
        match (first, last) {
            (None, None) => true,
            (Some(f), None) | (None, Some(f)) => f == side,
            (Some(f), Some(l)) if f == l => f == side,
            (Some(f), Some(l)) => {
                if line == start {
                    f == side
                } else if line == end {
                    l == side
                } else {
                    true
                }
            }
        }
    }

    /// `formatSelectedLineLabel`: "line 3" or "lines 3-5".
    pub fn label(self) -> String {
        match self.ordered() {
            (a, b) if a == b => opencode_i18n::t!("ui.sessionReview.selection.line", line = a),
            (a, b) => opencode_i18n::t!("ui.sessionReview.selection.lines", start = a, end = b),
        }
    }
}

/// A comment being written: its text and the field's own state.
#[derive(Clone, Debug)]
pub struct Draft {
    pub text: TextEdit,
    pub field: Field,
    /// The narrow field's height once its grip has been dragged (`resize: vertical`).
    pub height: Option<f32>,
    pub grip: Grip,
}

/// The narrow field's resize grip: Chromium's textarea resizer, which follows the pointer from
/// the height the field had at the press.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Grip {
    #[default]
    Idle,
    Dragging { from: f32 },
}

/// A step of the grip's drag: the press (with the field's height then), the pointer `dy` from
/// it, the release.
pub enum GripEvent {
    Down { height: f32 },
    Move { dy: f32 },
    Up,
}

impl Draft {
    fn new(text: &str) -> Draft {
        let mut text = TextEdit::new(text);
        text.focused = true;
        Draft { text, field: Field::default(), height: None, grip: Grip::Idle }
    }

    /// Steps the grip; a move while dragging sets the field's height, never under `min`.
    pub fn grip(&mut self, event: GripEvent, min: f32) {
        match (self.grip, event) {
            (_, GripEvent::Down { height }) => self.grip = Grip::Dragging { from: height },
            (Grip::Dragging { from }, GripEvent::Move { dy }) => self.height = Some((from + dy).max(min)),
            (_, GripEvent::Up) => self.grip = Grip::Idle,
            (Grip::Idle, GripEvent::Move { .. }) => {}
        }
    }

    fn holds_text(&self) -> bool {
        !self.text.text.trim().is_empty()
    }
}

/// `LineCommentMode`.
#[derive(Clone, Debug, Default)]
pub enum Mode {
    #[default]
    Idle,
    Selecting { range: Range },
    Drafting { range: Range, draft: Draft },
    Viewing { id: String, range: Range },
    Editing { id: String, range: Range, draft: Draft },
}

/// `LineCommentEvent`, less the text edits, which land in the draft in place.
#[derive(Clone, Debug)]
pub enum Event {
    /// The view's selection changed under a gesture (`line.selected`).
    LineSelected(Option<Range>),
    /// The gesture ended (`line.selection-ended`).
    SelectionEnded(Option<Range>),
    /// The gutter's `+` (`draft.opened`).
    DraftOpened(Range),
    DraftCancelled,
    DraftSubmitted,
    CommentHovered(Range),
    CommentToggled { id: String, range: Range },
    CommentFocused { id: String, range: Range },
    CommentDeleted { id: String },
    EditOpened { id: String, range: Range, text: String },
    EditCancelled,
    EditSubmitted,
    PassiveCleared,
    Reset,
}

/// `LineCommentEffect`.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// The view shows these lines selected (`sync-selected`).
    Sync(Option<Range>),
    SubmitDraft { comment: String, selection: Range },
    SubmitEdit { id: String, comment: String, selection: Range },
    Delete { id: String },
}

/// A transition's effects, and whether the mode changed (`step.state !== current`).
pub struct Step {
    pub effects: Effects<Effect>,
    pub changed: bool,
}

impl Mode {
    /// The lines the view shows selected while the mode holds them (`viewLineComment.selected`).
    pub fn selected(&self) -> Option<Range> {
        match self {
            Mode::Idle => None,
            Mode::Selecting { range } | Mode::Drafting { range, .. } | Mode::Viewing { range, .. } | Mode::Editing { range, .. } => Some(*range),
        }
    }

    /// Where the new comment's editor stands (`draftRange`).
    pub fn draft_range(&self) -> Option<Range> {
        match self {
            Mode::Drafting { range, .. } => Some(*range),
            _ => None,
        }
    }

    /// The selection the gutter's `+` comments on (`gutterSelected`).
    pub fn gutter_selected(&self) -> Option<Range> {
        match self {
            Mode::Selecting { range } | Mode::Drafting { range, .. } => Some(*range),
            _ => None,
        }
    }

    /// Whether saved comment `id` is opened (`viewCommentItem.open`).
    pub fn opened(&self, id: &str) -> bool {
        matches!(self, Mode::Viewing { id: i, .. } | Mode::Editing { id: i, .. } if i == id)
    }

    /// The edit of saved comment `id`, when it's being edited.
    pub fn editing(&self, id: &str) -> Option<&Draft> {
        match self {
            Mode::Editing { id: i, draft, .. } if i == id => Some(draft),
            _ => None,
        }
    }

    pub fn draft(&self) -> Option<&Draft> {
        match self {
            Mode::Drafting { draft, .. } | Mode::Editing { draft, .. } => Some(draft),
            _ => None,
        }
    }

    pub fn draft_mut(&mut self) -> Option<&mut Draft> {
        match self {
            Mode::Drafting { draft, .. } | Mode::Editing { draft, .. } => Some(draft),
            _ => None,
        }
    }

    fn range_mut(&mut self) -> Option<&mut Range> {
        match self {
            Mode::Idle => None,
            Mode::Selecting { range } | Mode::Drafting { range, .. } | Mode::Viewing { range, .. } | Mode::Editing { range, .. } => Some(range),
        }
    }

    /// `holdsText`: an open edit, or a draft with words in it.
    fn holds_text(&self) -> bool {
        match self {
            Mode::Editing { .. } => true,
            Mode::Drafting { draft, .. } => draft.holds_text(),
            _ => false,
        }
    }

    fn is_idle(&self) -> bool {
        matches!(self, Mode::Idle)
    }
}

/// `transitionLineComment`.
pub fn step(mode: &mut Mode, event: Event) -> Step {
    match event {
        Event::LineSelected(None) | Event::SelectionEnded(None) => clear(mode),
        Event::LineSelected(Some(r)) => {
            // The view reports every highlight it applies, the mode's own included.
            if mode.selected().is_some_and(|s| s.same(r)) {
                return stay();
            }
            match std::mem::take(mode) {
                editing @ Mode::Editing { .. } => {
                    *mode = editing;
                    restore(mode, Some(r))
                }
                Mode::Drafting { draft, .. } => enter(mode, Mode::Drafting { range: r, draft }),
                _ => enter(mode, Mode::Selecting { range: r }),
            }
        }
        Event::SelectionEnded(Some(r)) => {
            if matches!(mode, Mode::Editing { .. }) {
                return restore(mode, Some(r));
            }
            open_draft(mode, r)
        }
        Event::DraftOpened(r) => open_draft(mode, r),
        Event::DraftCancelled => {
            if !matches!(mode, Mode::Drafting { .. }) {
                return stay();
            }
            enter(mode, Mode::Idle)
        }
        Event::DraftSubmitted => {
            let Mode::Drafting { range, draft } = &*mode else { return stay() };
            let comment = draft.text.text.trim().to_owned();
            if comment.is_empty() {
                return stay();
            }
            let selection = *range;
            let mut s = enter(mode, Mode::Idle);
            s.effects.push(Effect::SubmitDraft { comment, selection });
            s
        }
        Event::CommentHovered(r) => match mode {
            Mode::Idle => enter(mode, Mode::Selecting { range: r }),
            Mode::Selecting { range } if range.same(r) => stay(),
            Mode::Selecting { .. } => enter(mode, Mode::Selecting { range: r }),
            _ => stay(),
        },
        Event::CommentToggled { id, range } => {
            if mode.holds_text() {
                return stay();
            }
            if matches!(mode, Mode::Viewing { id: i, .. } if *i == id) {
                return enter(mode, Mode::Idle);
            }
            enter(mode, Mode::Viewing { id, range })
        }
        Event::CommentFocused { id, range } => {
            if mode.holds_text() {
                return stay();
            }
            if matches!(mode, Mode::Viewing { id: i, range: r } if *i == id && r.same(range)) {
                return stay();
            }
            enter(mode, Mode::Viewing { id, range })
        }
        Event::CommentDeleted { id } => {
            let effect = Effect::Delete { id: id.clone() };
            if mode.opened(&id) {
                let mut s = enter(mode, Mode::Idle);
                s.effects.push(effect);
                return s;
            }
            Step { effects: Effects::one(effect), changed: false }
        }
        Event::EditOpened { id, range, text } => {
            if mode.holds_text() {
                return stay();
            }
            enter(mode, Mode::Editing { id, range, draft: Draft::new(&text) })
        }
        Event::EditCancelled => {
            if !matches!(mode, Mode::Editing { .. }) {
                return stay();
            }
            enter(mode, Mode::Idle)
        }
        Event::EditSubmitted => {
            let Mode::Editing { id, range, draft } = &*mode else { return stay() };
            let comment = draft.text.text.trim().to_owned();
            if comment.is_empty() {
                return stay();
            }
            let (id, selection) = (id.clone(), *range);
            let mut s = enter(mode, Mode::Idle);
            s.effects.push(Effect::SubmitEdit { id, comment, selection });
            s
        }
        // A passive mode and a reset end without moving the view's selection.
        Event::PassiveCleared => {
            if !matches!(mode, Mode::Selecting { .. } | Mode::Viewing { .. }) {
                return stay();
            }
            *mode = Mode::Idle;
            Step { effects: Effects::none(), changed: true }
        }
        Event::Reset => {
            if mode.is_idle() {
                return stay();
            }
            *mode = Mode::Idle;
            Step { effects: Effects::none(), changed: true }
        }
    }
}

/// `transitionReviewCommentModes`: `event` for file `file` of a list of files. A file that
/// becomes active drops the passive modes of the others and keeps their drafts and edits.
pub fn step_files(modes: &mut HashMap<String, Mode>, file: &str, event: Event) -> Effects<Effect> {
    let mut mode = modes.remove(file).unwrap_or_default();
    let s = step(&mut mode, event);
    if s.changed {
        let idle = mode.is_idle();
        modes.retain(|_, m| idle || matches!(m, Mode::Drafting { .. } | Mode::Editing { .. }));
    }
    if !mode.is_idle() {
        modes.insert(file.to_owned(), mode);
    }
    s.effects
}

fn open_draft(mode: &mut Mode, r: Range) -> Step {
    match mode {
        Mode::Editing { .. } => stay(),
        Mode::Drafting { range, .. } if range.same(r) => stay(),
        Mode::Drafting { .. } => {
            let Mode::Drafting { draft, .. } = std::mem::take(mode) else { unreachable!() };
            enter(mode, Mode::Drafting { range: r, draft })
        }
        _ => enter(mode, Mode::Drafting { range: r, draft: Draft::new("") }),
    }
}

fn clear(mode: &mut Mode) -> Step {
    if mode.is_idle() {
        return stay();
    }
    if mode.holds_text() {
        return restore(mode, None);
    }
    enter(mode, Mode::Idle)
}

/// The view highlighted a range before the mode saw it: a mode holding text keeps its own range
/// and hands it back to the view.
fn restore(mode: &mut Mode, r: Option<Range>) -> Step {
    let held = mode.selected();
    if r.zip(held).is_some_and(|(r, h)| h.same(r)) {
        return stay();
    }
    if let (Some(range), Some(h)) = (mode.range_mut(), held) {
        *range = h;
    }
    Step { effects: Effects::one(Effect::Sync(held)), changed: true }
}

fn enter(mode: &mut Mode, next: Mode) -> Step {
    let sync = next.selected();
    *mode = next;
    Step { effects: Effects::one(Effect::Sync(sync)), changed: true }
}

fn stay() -> Step {
    Step { effects: Effects::none(), changed: false }
}
