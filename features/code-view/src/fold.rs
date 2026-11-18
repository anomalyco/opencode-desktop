//! The fold state of each unchanged range between hunks, and Review's "Show all lines" toggle,
//! as explicit state machines.
//!
//! Desktop's separators (Pierre's `line-info-basic`, `DiffHunksRenderer.expandHunk`) open a
//! range twenty lines at a time from either end, all of it, or twenty lines at each end from
//! its count; a range of one line never folds (`collapsedContextThreshold` 1). Rune adds one
//! state Pierre has no need for: a range whose file text isn't loaded yet waits for it
//! (`Expanding`) before it opens.

use rune_kit::diff::{EXPAND_STEP, Expand};

/// How a fold opens: a separator button's step (or all of it), or its "N unmodified lines"
/// text, which opens twenty lines at each end (Pierre's `both`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    Step(Expand),
    Around,
}

/// One unchanged range between hunks (or before the first, or after the last).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    /// Folded behind its separator, with `top` lines shown under the hunk above it and `bottom`
    /// over the hunk below it.
    Folded { top: u32, bottom: u32 },
    /// A reveal asked for before the file's whole text arrived, waiting on it.
    Expanding { top: u32, bottom: u32, how: Reveal },
    /// Every line shown; no separator.
    Expanded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldEvent {
    /// A separator was pressed.
    Reveal(Reveal),
    /// The file's whole text arrived.
    TextArrived,
    /// The file's whole text can't be had.
    TextFailed,
    /// "Hide non-diff lines": back to folded.
    Fold,
    /// "Show all lines".
    Unfold,
}

/// What a transition asks of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldEffect {
    /// Load the file's whole text, then send [`FoldEvent::TextArrived`] or
    /// [`FoldEvent::TextFailed`].
    FetchText,
}

/// What a range's transitions read: its length when known, whether the file's whole text is
/// loaded, and whether it is the first or last range of its file.
#[derive(Clone, Copy, Debug)]
pub struct GapFacts {
    pub len: Option<u32>,
    pub has_text: bool,
    pub first: bool,
    pub last: bool,
}

impl Fold {
    /// A range as a file loads: shown when it is one line Desktop wouldn't fold, else folded.
    pub fn initial(g: GapFacts) -> Fold {
        if g.has_text && g.len == Some(1) { Fold::Expanded } else { Fold::Folded { top: 0, bottom: 0 } }
    }

    /// Lines shown under the hunk above and over the hunk below, when the range is `len` long.
    pub fn shown(self, len: Option<u32>) -> (u32, u32) {
        match self {
            Fold::Folded { top, bottom } | Fold::Expanding { top, bottom, .. } => (top, bottom),
            Fold::Expanded => (len.unwrap_or(0), 0),
        }
    }

    /// Lines still folded, when the range's length is known.
    pub fn hidden(self, len: Option<u32>) -> Option<u32> {
        let (top, bottom) = self.shown(len);
        len.map(|n| n.saturating_sub(top + bottom))
    }

    pub fn step(self, event: FoldEvent, g: GapFacts) -> (Fold, Option<FoldEffect>) {
        match (self, event) {
            (Fold::Folded { top, bottom }, FoldEvent::Reveal(how)) if !g.has_text => {
                (Fold::Expanding { top, bottom, how }, Some(FoldEffect::FetchText))
            }
            (Fold::Folded { top, bottom }, FoldEvent::Reveal(how)) => (open(top, bottom, how, g), None),
            (Fold::Expanding { top, bottom, how }, FoldEvent::TextArrived) if g.has_text => (open(top, bottom, how, g), None),
            (Fold::Expanding { top, bottom, .. }, FoldEvent::TextFailed) => (Fold::Folded { top, bottom }, None),
            (Fold::Folded { top: 0, bottom: 0 }, FoldEvent::TextArrived) => (Fold::initial(g), None),
            (_, FoldEvent::Fold) => (Fold::initial(g), None),
            (Fold::Folded { .. }, FoldEvent::Unfold) if g.has_text && g.len.is_some() => (Fold::Expanded, None),
            (state, _) => (state, None),
        }
    }
}

/// The range after `how` opens it from `top` and `bottom` shown.
fn open(mut top: u32, mut bottom: u32, how: Reveal, g: GapFacts) -> Fold {
    let Some(len) = g.len else { return Fold::Folded { top, bottom } };
    let hidden = len.saturating_sub(top + bottom);
    let step = hidden.min(EXPAND_STEP);
    match how {
        Reveal::Step(Expand::All) => return Fold::Expanded,
        // The last range has nothing below it to grow up from, so it opens from the top.
        Reveal::Step(Expand::Up) if g.last => top += step,
        Reveal::Step(Expand::Up) => bottom += step,
        Reveal::Step(Expand::Down) => top += step,
        Reveal::Around if hidden <= 2 * EXPAND_STEP => return Fold::Expanded,
        Reveal::Around => {
            top += EXPAND_STEP;
            bottom += EXPAND_STEP;
        }
    }
    if top + bottom >= len { Fold::Expanded } else { Fold::Folded { top, bottom } }
}

/// Review's "Show all lines" / "Hide non-diff lines" toggle (`SessionReviewV2`'s expand
/// buttons): each change folds or unfolds every range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExpandMode {
    #[default]
    Collapse,
    Expand,
}

impl ExpandMode {
    /// Choosing `to`: the new mode and what every range receives, nothing when it is the same.
    pub fn step(self, to: ExpandMode) -> (ExpandMode, Option<FoldEvent>) {
        match (self, to) {
            (from, to) if from == to => (from, None),
            (_, ExpandMode::Expand) => (to, Some(FoldEvent::Unfold)),
            (_, ExpandMode::Collapse) => (to, Some(FoldEvent::Fold)),
        }
    }
}
