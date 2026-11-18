//! Line comments on a code or diff view (Pierre's annotations and gutter utility, as Desktop's
//! `createLineCommentController` drives them): the host says which lines carry a comment and
//! what shows under a line, and hears the view's line selections and the gutter's `+`.
//!
//! The view draws a line's annotations as Pierre does: a row under it on `--diffs-comment-bg`
//! (the accent at 6%) across the number column and the code, each annotation 12 in
//! (`[data-annotation-slot] { padding: 12px }`); in a split diff the other half gets the same
//! row, empty. The `+` sits 10 right of a hovered line's number cell, 20 square, centred on
//! the row (`[data-gutter-utility-slot]`, `comment-hover.ts`).

use rune::{El, Icon, Tx, col, el, icon, row};
use rune_kit::diff::Side;
use session_view::palette::Pal;

/// Lines a selection or comment covers (Pierre's `SelectedLineRange`): from `start` to `end`,
/// either way round; in a diff, the side each end is on (`end_side` only when it differs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
    pub side: Option<Side>,
    pub end_side: Option<Side>,
}

impl LineRange {
    pub fn line(n: u32, side: Option<Side>) -> LineRange {
        LineRange { start: n, end: n, side, end_side: None }
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
    pub fn same(self, other: LineRange) -> bool {
        self.start == other.start
            && self.end == other.end
            && self.side == other.side
            && self.end_side.or(self.side) == other.end_side.or(other.side)
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

/// What a gesture on the view's lines told the host (Pierre's `onLineSelected` and
/// `onLineSelectionEnd`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handoff {
    Selected(Option<LineRange>),
    Ended(Option<LineRange>),
    /// A press on the one selected line's number let go without dragging (Pierre's
    /// `pendingSingleLineUnselect`), which the view took to clear it.
    Unselected(LineRange),
}

/// The host of a view's line comments. `path` is the file the lines belong to.
pub trait LineNotes<S> {
    fn text_selection(&self, s: &mut S, viewer: &str, dragging: bool, selected: Option<(&str, LineRange, bool)>, tx: &mut Tx);
    /// Changes whenever anything the other reads show for `path` changes (the rows' memos).
    fn version(&self, s: &S, path: &str) -> u64;
    /// Whether line `line` on `side` carries a saved comment (`commentedLines`).
    fn commented(&self, s: &S, path: &str, side: Side, line: u32) -> bool;
    /// What shows under line `line` on `side`: each annotation's content, in order.
    fn annotations(&self, s: &S, p: &Pal, path: &str, side: Side, line: u32) -> Vec<El>;
    /// The view's selection changed under a gesture, or the gesture ended.
    fn handoff(&self, s: &mut S, path: &str, h: Handoff, tx: &mut Tx);
    /// The gutter's `+` on line `line` (`draft.opened`).
    fn plus(&self, s: &mut S, path: &str, side: Option<Side>, line: u32, tx: &mut Tx);
    /// Whether the `+` stays hidden under a pointer that hasn't moved since a box opened or
    /// closed: Pierre redraws the lines then, and they lose `[data-hovered]` until the next move.
    fn plus_quiet(&self, s: &S) -> bool;
    /// The pointer moved over the view.
    fn pointer_moved(&self, s: &mut S, tx: &mut Tx);
}

/// Read both ends from the same selection snapshot. A range crossing files can't be a file comment.
pub fn observe<S: 'static>(
    rows: El,
    viewer: String,
    notes: std::rc::Rc<dyn LineNotes<S>>,
    file: impl for<'a> Fn(&'a S, usize) -> Option<&'a str> + 'static,
) -> El {
    rows.on_selection_change(move |s: &mut S, change, tx| {
        let selected = change.selection().filter(|_| !change.dragging()).and_then(|sel| {
            let mut texts = sel.texts().iter().filter(|t| t.is_selected());
            let first = texts.next()?;
            let last = texts.next_back().unwrap_or(first);
            let (f, side, start) = decode(first.key()?)?;
            let (end_file, end_side, end) = decode(last.key()?)?;
            if f != end_file {
                return None;
            }
            let path = file(s, f)?.to_owned();
            Some((path, LineRange { start, end, side, end_side: end_side.filter(|_| side != end_side) }, sel.backward()))
        });
        notes.text_selection(s, &viewer, change.dragging(), selected.as_ref().map(|(p, r, b)| (p.as_str(), *r, *b)), tx);
    })
}

pub fn text_key(file: usize, side: Option<Side>, line: u32) -> String {
    let side = match side {
        None => "file",
        Some(Side::Old) => "old",
        Some(Side::New) => "new",
    };
    format!("source:{file}:{side}:{line}")
}

fn decode(key: &str) -> Option<(usize, Option<Side>, u32)> {
    let mut parts = key.strip_prefix("source:")?.split(':');
    let file = parts.next()?.parse().ok()?;
    let side = match parts.next()? {
        "file" => None,
        "old" => Some(Side::Old),
        "new" => Some(Side::New),
        _ => return None,
    };
    Some((file, side, parts.next()?.parse().ok()?))
}

/// `--diffs-comment-bg`: `background-bg-accent` at 6% over the code's background, laid as CSS
/// lays it.
pub fn comment_bg(p: &Pal) -> rune::Color {
    session_view::palette::over(p.code_bg, p.bg_accent, 0.06)
}

/// A line's annotation row: the number column's width of comment background, then the
/// annotations, each 12 in (`[data-line-annotation]`, `[data-gutter-buffer='annotation']`).
pub fn annotation_row(p: &Pal, gutter_w: f32, items: Vec<El>) -> El {
    row()
        .key("annotation")
        .w_full()
        .items_stretch()
        .fill(comment_bg(p))
        .child(el().key("buffer").w(gutter_w).self_stretch())
        .child(annotation_slots(items))
}

/// The annotations of one line, one under another, each in its 12-point slot.
pub fn annotation_slots(items: Vec<El>) -> El {
    col()
        .key("slots")
        .grow(1.0)
        .min_w(0.0)
        .children(items.into_iter().enumerate().map(|(i, item)| col().key(format!("slot-{i}")).w_full().padding(12.0).child(item)))
}

/// `comment-hover.ts`'s drawn plus.
static PLUS: Icon = Icon {
    name: "line-comment-plus",
    view: 12.0,
    body: r#"<path d="M6 2V10M2 6H10" stroke="currentColor" stroke-width="1.5"/>"#,
};

fn plus_button_shell(p: &Pal, cell_w: f32, label: &str) -> El {
    // Out of the Tab order: the keyboard comments from the line itself (Return or C).
    let button = el()
        .key("comment-plus")
        .role("button")
        .label(label.to_owned())
        .tab_index(-1)
        .absolute(cell_w + 10.0, 2.0)
        .size(20.0, 20.0)
        .radius(6.0)
        .center()
        .fill(p.interactive);
    shadow_xs(p.dark)
        .into_iter()
        .rev()
        .fold(button, El::box_shadow)
        .group_opacity(0.0, 1.0)
        .child(icon(&PLUS, 12.0).color(rune::Color::WHITE))
}

/// The gutter's `+` for a line's number cell `cell_w` wide: 20 square, its left edge 10 past
/// the cell's right one, on `--icon-interactive-base` with `--shadow-xs`, shown while the cell's
/// row is under the pointer.
pub fn plus_button<S: 'static>(p: &Pal, cell_w: f32, label: &str, on: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    plus_button_shell(p, cell_w, label).on_press_tx(move |s: &mut S, tx: &mut Tx| {
        tx.instant();
        on(s, tx);
    })
}

/// `--shadow-xs` (`light-dark` alphas), in CSS order.
fn shadow_xs(dark: bool) -> [rune::Shadow; 3] {
    let s = |y: f32, blur: f32, spread: f32, a: f32| rune::Shadow {
        y,
        blur,
        spread,
        color: rune::Color::BLACK.alpha(a),
        ..rune::Shadow::default()
    };
    let (a, b, c) = if dark { (0x0f_u8, 0x14_u8, 0x1a_u8) } else { (0x0a, 0x06, 0x0d) };
    [s(1.0, 2.0, -0.5, f32::from(a) / 255.0), s(0.5, 1.5, 0.0, f32::from(b) / 255.0), s(1.0, 3.0, 0.0, f32::from(c) / 255.0)]
}

/// A diff's selected lines as a comment's: each end's line and side, the end's only when it
/// differs.
pub fn diff_range(sel: rune_kit::diff::LineRange) -> LineRange {
    let end_side = (sel.end.side != sel.start.side).then_some(sel.end.side);
    LineRange { start: sel.start.line, end: sel.end.line, side: Some(sel.start.side), end_side }
}

/// Tells `notes` what a gesture did to the selection: a change, and with `ended` its end (with
/// the line a press on the one selected line's number left, when it did that).
pub(crate) fn hand_off<S>(
    notes: &dyn LineNotes<S>,
    s: &mut S,
    path: &str,
    before: Option<rune_kit::diff::LineRange>,
    after: Option<rune_kit::diff::LineRange>,
    ended: Option<Option<rune_kit::diff::LineRange>>,
    tx: &mut Tx,
) {
    let after_range = after.map(diff_range);
    if let Some(Some(line)) = ended {
        return notes.handoff(s, path, Handoff::Unselected(diff_range(line)), tx);
    }
    if before.map(diff_range) != after_range {
        notes.handoff(s, path, Handoff::Selected(after_range), tx);
    }
    if ended.is_some() {
        notes.handoff(s, path, Handoff::Ended(after_range), tx);
    }
}
