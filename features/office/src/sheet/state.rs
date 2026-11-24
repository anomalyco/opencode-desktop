//! What a sheet's grid keeps (`spreadsheet.tsx` `SpreadsheetGrid`): the selection, the pointer
//! gesture, the cells screen readers browse around the active cell, a copy in flight and a cell
//! being scrolled into view. Each of the last three is a machine of its own; the workbook's
//! thread answers them by ticket, and an answer to a superseded ask changes nothing.

use std::rc::Rc;

use office_preview::xlsx::{LAST_CELL, Range, SheetLayout};

use super::pointer::{Hit, Pointer, PointerEffect, PointerEvent};
use super::select::{self, Cell, KeyMove, Limits, Selection};

/// The cells screen readers can browse: a block around the active cell that moves with it a step
/// at a time, so a key press rarely reads it again (`readerBlock`).
const READER_BLOCK: [u32; 2] = [10, 5];

/// What a grid asks the workbook's thread.
pub enum Ask {
    Cells { range: Range, ticket: u64, purpose: Purpose },
    Position { cell: Cell, ticket: u64 },
}

pub use office_preview::engine::Purpose;

/// What the window does for a grid once an answer arrives.
pub enum UiEffect {
    Copy { text: String, html: String },
    /// A link's web or mail address, opened outside the app.
    Open(String),
    Toast(String),
    Scroll { key: String, x: f32, y: f32 },
}

/// The block's cells as they show.
#[derive(Clone, Debug, Default)]
pub struct Read {
    pub range: Range,
    pub texts: Vec<Vec<String>>,
}

/// The block screen readers browse (`createLatest`): the last block read stays while a newer one
/// is read.
#[derive(Default)]
enum Reader {
    #[default]
    Empty,
    Asking { ticket: u64, range: Range, shown: Option<Rc<Read>> },
    Shown(Rc<Read>),
}

/// A copy: idle, or reading its cells (a newer copy supersedes it).
#[derive(Default)]
enum Copying {
    #[default]
    Idle,
    Reading { ticket: u64, failed: String },
}

/// A cell being scrolled into view, once the thread says where it is.
#[derive(Default)]
enum Reveal {
    #[default]
    Idle,
    Asking { ticket: u64, cell: Cell, align: Align },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Nearest,
}

/// Where the grid's scroll view is: its scroll offset, its size, the row headers' width and the
/// frozen panes' size, all in points.
#[derive(Clone, Copy, Debug, Default)]
pub struct View {
    pub scroll: [f32; 2],
    pub size: [f32; 2],
    pub header_w: f32,
    pub frozen: [f32; 2],
}

#[derive(Default)]
pub struct Grid {
    pub selection: Selection,
    pointer: Pointer,
    reader: Reader,
    copying: Copying,
    reveal: Reveal,
    pub view: View,
    /// The farthest point the scroll area reaches beyond its cells: where a reveal scrolled to,
    /// or a scroll near the area's end, a screen on (`reachX`, `reachY`).
    pub reach: [f32; 2],
    tickets: u64,
}

impl Grid {
    fn ticket(&mut self) -> u64 {
        self.tickets += 1;
        self.tickets
    }

    /// The block screen readers have, the last one read.
    pub fn read(&self) -> Option<&Rc<Read>> {
        match &self.reader {
            Reader::Empty => None,
            Reader::Asking { shown, .. } => shown.as_ref(),
            Reader::Shown(read) => Some(read),
        }
    }

    /// Selects `selection`; asks for the screen readers' block when the focus left it.
    pub fn select(&mut self, selection: Selection) -> Option<Ask> {
        self.selection = selection;
        let focus = selection.focus;
        let start = |at: u32, step: u32| (at / step).saturating_sub(1) * step;
        let (top, left) = (start(focus[0], READER_BLOCK[0]), start(focus[1], READER_BLOCK[1]));
        let range = [top, left, (top + READER_BLOCK[0] * 3 - 1).min(LAST_CELL[0]), (left + READER_BLOCK[1] * 3 - 1).min(LAST_CELL[1])];
        let current = match &self.reader {
            Reader::Empty => None,
            Reader::Asking { range, .. } => Some(*range),
            Reader::Shown(read) => Some(read.range),
        };
        if current.is_some_and(|r| r[0] == range[0] && r[1] == range[1]) {
            return None;
        }
        let ticket = self.ticket();
        let shown = match std::mem::take(&mut self.reader) {
            Reader::Empty => None,
            Reader::Asking { shown, .. } => shown,
            Reader::Shown(read) => Some(read),
        };
        self.reader = Reader::Asking { ticket, range, shown };
        Some(Ask::Cells { range, ticket, purpose: Purpose::Read })
    }

    /// A pointer step; the asks it makes, and the cell whose link a click follows. A whole row or
    /// column keeps its far end as the anchor, so the arrow keys move on from its first cell
    /// (`runPointer`).
    pub fn pointer(&mut self, event: PointerEvent, layout: &SheetLayout) -> (Vec<Ask>, Option<Cell>) {
        let (next, effects) = self.pointer.step(event);
        self.pointer = next;
        let mut asks = Vec::new();
        let mut follow = None;
        for effect in effects {
            let anchor = self.selection.anchor;
            let selection = match effect {
                PointerEffect::SelectAll => Selection { anchor: LAST_CELL, focus: [0, 0] },
                PointerEffect::SelectColumn { col, extend } => Selection { anchor: [LAST_CELL[0], if extend { anchor[1] } else { col }], focus: [0, col] },
                PointerEffect::SelectRow { row, extend } => Selection { anchor: [if extend { anchor[0] } else { row }, LAST_CELL[1]], focus: [row, 0] },
                PointerEffect::SelectCell { cell, extend: true } => self.selection.extend_to(cell, None),
                PointerEffect::SelectCell { cell, extend: false } => select::at_cell(cell, &layout.merges),
                PointerEffect::Extend(focus) => Selection { anchor, focus },
                PointerEffect::FollowLink(cell) => {
                    follow = Some(cell);
                    continue;
                }
            };
            asks.extend(self.select(selection));
        }
        (asks, follow)
    }

    /// Selects `cell` and scrolls it to the start of the view: where a link on this sheet goes
    /// (`follow`), or a link from another sheet opened this one at.
    pub fn go_to(&mut self, cell: Cell, layout: &SheetLayout) -> Vec<Ask> {
        let mut asks: Vec<Ask> = self.select(select::at_cell(cell, &layout.merges)).into_iter().collect();
        asks.extend(self.reveal(cell, Align::Start));
        asks
    }

    /// What a drag under way moves along.
    pub fn axis(&self) -> Option<super::pointer::Axis> {
        match self.pointer {
            Pointer::Dragging(axis) => Some(axis),
            Pointer::Idle => None,
        }
    }

    /// A key on the grid (`key`): the asks it makes, or `None` when the grid doesn't take it.
    /// Copy is the caller's (`copy`).
    pub fn key(&mut self, key: &rune::Key, mods: rune::Mods, layout: &SheetLayout) -> Option<Vec<Ask>> {
        let command = mods.cmd || mods.ctrl;
        // Tab leaves the grid for the next control, and Alt keeps its meaning.
        if matches!(key, rune::Key::Tab) || mods.alt {
            return None;
        }
        // As in Excel, Ctrl+Home goes to the first cell past the frozen panes and scrolls back to
        // the start.
        if command && matches!(key, rune::Key::Home) {
            let home = [layout.frozen_rows, layout.frozen_cols];
            let mut asks: Vec<Ask> = self.select(select::at_cell(home, &layout.merges)).into_iter().collect();
            asks.extend(self.reveal(home, Align::Start));
            return Some(asks);
        }
        let all = matches!(key, rune::Key::Command(c) if c.eq_ignore_ascii_case(&'a'));
        let bounded = matches!(key, rune::Key::Home | rune::Key::End) || (command && (all || matches!(key, rune::Key::Up | rune::Key::Down | rune::Key::Left | rune::Key::Right)));
        let KeyMove::Move(moved) = select::key(self.selection, key, mods, self.limits(bounded, layout)) else { return None };
        let next = select::skip_hidden(self.selection.focus, select::settle_merges(self.selection, moved, &layout.merges), &layout.hidden_rows, &layout.hidden_cols);
        let mut asks: Vec<Ask> = self.select(next).into_iter().collect();
        // Selecting everything leaves the view where it is, as in Excel.
        if !all {
            asks.extend(self.reveal(next.focus, Align::Nearest));
        }
        Some(asks)
    }

    /// Jumps to an edge stop at the used cells, or where the active cell already is past them.
    fn limits(&self, bounded: bool, layout: &SheetLayout) -> Limits {
        let rows_per_page = (((self.view.size[1] - super::HEADER_H - self.view.frozen[1]) / 20.0).floor() as u32).max(1);
        if !bounded {
            return Limits { rows: LAST_CELL[0] + 1, cols: LAST_CELL[1] + 1, rows_per_page };
        }
        let used = layout.used.unwrap_or_default();
        let focus = self.selection.focus;
        Limits { rows: (used[2] + 1).max(focus[0] + 1), cols: (used[3] + 1).max(focus[1] + 1), rows_per_page }
    }

    /// Asks where `cell` is, to scroll it into view (`reveal`).
    fn reveal(&mut self, cell: Cell, align: Align) -> Option<Ask> {
        let ticket = self.ticket();
        self.reveal = Reveal::Asking { ticket, cell, align };
        Some(Ask::Position { cell, ticket })
    }

    /// Where the cell being revealed is: x, y and the cell after it diagonally, in scroll
    /// coordinates; the scroll that shows it, past the frozen panes (`reveal`, `place`).
    pub fn positioned(&mut self, ticket: u64, rect: Result<[f64; 4], String>, layout: &SheetLayout, key: &str) -> Option<UiEffect> {
        let Reveal::Asking { ticket: asked, cell, align } = self.reveal else { return None };
        if asked != ticket {
            return None;
        }
        self.reveal = Reveal::Idle;
        let [x, y, next_x, next_y] = rect.ok()?.map(|v| v as f32);
        let v = self.view;
        let visible = [v.size[0] - v.header_w - v.frozen[0], v.size[1] - super::HEADER_H - v.frozen[1]];
        let left = if cell[1] < layout.frozen_cols { v.scroll[0] } else { place(x, next_x - x, v.scroll[0], visible[0], align) };
        let top = if cell[0] < layout.frozen_rows { v.scroll[1] } else { place(y, next_y - y, v.scroll[1], visible[1], align) };
        if (left - v.scroll[0]).abs() < 0.5 && (top - v.scroll[1]).abs() < 0.5 {
            return None;
        }
        // Grow the scroll area first, so the scroll is not cut short at its current edge.
        self.reach = [left + v.size[0] - v.header_w, top + v.size[1] - super::HEADER_H];
        Some(UiEffect::Scroll { key: key.to_owned(), x: left, y: top })
    }

    /// Starts a copy of the selection, clipped to the used cells (`copy`); a toast when it is too
    /// large.
    pub fn copy(&mut self, layout: &SheetLayout, too_large: &str, failed: &str) -> Result<Ask, UiEffect> {
        let range = select::cover_merges(self.selection.range(), &layout.merges);
        let used = layout.used.unwrap_or_default();
        let clipped = [range[0], range[1], range[2].min(range[0].max(used[2])), range[3].min(range[1].max(used[3]))];
        let cells = u64::from(clipped[2] - clipped[0] + 1) * u64::from(clipped[3] - clipped[1] + 1);
        if cells > super::copy::LIMIT {
            return Err(UiEffect::Toast(too_large.to_owned()));
        }
        let ticket = self.ticket();
        self.copying = Copying::Reading { ticket, failed: failed.to_owned() };
        Ok(Ask::Cells { range: clipped, ticket, purpose: Purpose::Copy })
    }

    /// The thread read the screen readers' block.
    pub fn cells_read(&mut self, ticket: u64, range: Range, cells: Result<Vec<Vec<office_preview::xlsx::CellText>>, String>) {
        let Reader::Asking { ticket: asked, .. } = &self.reader else { return };
        if *asked != ticket {
            return;
        }
        let texts = cells.unwrap_or_default().into_iter().map(|row| row.into_iter().map(|c| c.text).collect()).collect();
        self.reader = Reader::Shown(Rc::new(Read { range, texts }));
    }

    /// The thread read a copy's cells and wrote them out as text and a table; what the window
    /// does with them.
    pub fn copied(&mut self, ticket: u64, copy: Result<(String, String), String>) -> Option<UiEffect> {
        let Copying::Reading { ticket: asked, failed } = std::mem::take(&mut self.copying) else { return None };
        if asked != ticket {
            self.copying = Copying::Reading { ticket: asked, failed };
            return None;
        }
        Some(match copy {
            Ok((text, html)) => UiEffect::Copy { text, html },
            Err(_) => UiEffect::Toast(failed),
        })
    }
}

/// Where to scroll so a cell shows: at the start, or just enough to bring it fully into view.
fn place(start: f32, size: f32, current: f32, visible: f32, align: Align) -> f32 {
    if align == Align::Start || start < current || size >= visible {
        return start;
    }
    if start + size > current + visible {
        return start + size - visible;
    }
    current
}

/// Where a pointer went down, from the cell under it and where it is past the headers (`hit`).
pub fn hit(cell: Cell, spot: [f32; 2]) -> Hit {
    match (spot[0] < 0.0, spot[1] < 0.0) {
        (true, true) => Hit::All,
        (false, true) => Hit::Column(cell),
        (true, false) => Hit::Row(cell),
        (false, false) => Hit::Cell(cell),
    }
}
