//! A worksheet's selection as Desktop's grid keeps it (`spreadsheet.tsx`, `sheet-cells.ts`, and
//! `@betteroffice/xlsx` 0.3.0's headless selection): an anchor and a focus cell, grown over the
//! merged cells it touches, moved by Excel's keys around merged cells and past hidden rows and
//! columns. The viewer is read-only: keys that would edit do nothing.

use office_preview::xlsx::{LAST_CELL, Range};
use rune::{Key, Mods};

/// A cell: row, column, zero-based.
pub type Cell = [u32; 2];

/// The cell the selection started from and the one keys and drags move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Cell,
    pub focus: Cell,
}

impl Selection {
    /// One cell (`selectionAt`).
    pub fn at(cell: Cell) -> Selection {
        Selection { anchor: cell, focus: cell }
    }

    /// The rectangle it spans (`normalizeRange`).
    pub fn range(&self) -> Range {
        let (a, f) = (self.anchor, self.focus);
        [a[0].min(f[0]), a[1].min(f[1]), a[0].max(f[0]), a[1].max(f[1])]
    }

    pub fn collapsed(&self) -> bool {
        self.anchor == self.focus
    }

    /// The far corner moved to `cell`, clamped into `limits` when given (`extendTo`).
    pub fn extend_to(&self, cell: Cell, limits: Option<[u32; 2]>) -> Selection {
        let focus = limits.map_or(cell, |[rows, cols]| [cell[0].min(rows.saturating_sub(1)), cell[1].min(cols.saturating_sub(1))]);
        Selection { anchor: self.anchor, focus }
    }
}

/// What the keys may reach: the rows and columns, and a page's rows (`SelectionLimits`).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub rows: u32,
    pub cols: u32,
    pub rows_per_page: u32,
}

pub fn contains(range: Range, cell: Cell) -> bool {
    cell[0] >= range[0] && cell[0] <= range[2] && cell[1] >= range[1] && cell[1] <= range[3]
}

fn overlaps(a: Range, b: Range) -> bool {
    a[1] <= b[3] && a[3] >= b[1] && a[0] <= b[2] && a[2] >= b[0]
}

/// Grows `range` until it covers every merged cell it touches, as Excel selects (`coverMerges`).
pub fn cover_merges(range: Range, merges: &[Range]) -> Range {
    let mut range = range;
    loop {
        let grown = merges.iter().fold(range, |r, m| if overlaps(r, *m) { [r[0].min(m[0]), r[1].min(m[1]), r[2].max(m[2]), r[3].max(m[3])] } else { r });
        // A merge the range grew into can touch another one it did not reach before.
        if grown == range {
            return grown;
        }
        range = grown;
    }
}

/// One cell selected; a merged cell selects from its first cell (`at`).
pub fn at_cell(cell: Cell, merges: &[Range]) -> Selection {
    Selection::at(merges.iter().find(|m| contains(**m, cell)).map_or(cell, |m| [m[0], m[1]]))
}

/// Moves a collapsed selection the way Excel moves its active cell around merged cells: landing
/// in one selects its first cell, and a step that stays inside the merged cell it started from
/// steps past it (`settleMerges`).
pub fn settle_merges(previous: Selection, next: Selection, merges: &[Range]) -> Selection {
    if !next.collapsed() {
        return next;
    }
    let Some(merge) = merges.iter().find(|m| contains(**m, next.focus)) else { return next };
    if !previous.collapsed() || !contains(*merge, previous.focus) {
        return Selection::at([merge[0], merge[1]]);
    }
    let past = |from: u32, to: u32, low: u32, high: u32, last: u32| -> u32 {
        let stepped = if to > from { i64::from(high) + 1 } else if to < from { i64::from(low) - 1 } else { i64::from(from) };
        stepped.clamp(0, i64::from(last)) as u32
    };
    let cell = [past(previous.focus[0], next.focus[0], merge[0], merge[2], LAST_CELL[0]), past(previous.focus[1], next.focus[1], merge[1], merge[3], LAST_CELL[1])];
    at_cell(cell, merges)
}

/// Moves a selection's focus off hidden rows and columns, on in the direction it moved, as
/// Excel's arrow keys skip them; a move that would leave the sheet stays where it started
/// (`skipHidden`).
pub fn skip_hidden(from: Cell, selection: Selection, rows: &[[u32; 2]], cols: &[[u32; 2]]) -> Selection {
    let focus = [clear(from[0], selection.focus[0], rows, LAST_CELL[0]), clear(from[1], selection.focus[1], cols, LAST_CELL[1])];
    if focus == selection.focus {
        return selection;
    }
    if selection.collapsed() {
        return Selection::at(focus);
    }
    Selection { anchor: selection.anchor, focus }
}

/// Whether row or column `index` lies in one of the hidden spans (`hiddenAt`).
pub fn hidden_at(spans: &[[u32; 2]], index: u32) -> bool {
    spans.iter().any(|s| index >= s[0] && index <= s[1])
}

fn clear(from: u32, to: u32, spans: &[[u32; 2]], last: u32) -> u32 {
    let Some(span) = spans.iter().find(|s| to >= s[0] && to <= s[1]) else { return to };
    if to == from {
        return to;
    }
    let next = if to > from { i64::from(span[1]) + 1 } else { i64::from(span[0]) - 1 };
    if next < 0 || next > i64::from(last) {
        return from;
    }
    clear(from, next as u32, spans, last)
}

/// What a key does to the selection (`selectionKeyReducer`'s `move`; the editing actions are
/// nothing in a read-only viewer).
pub enum KeyMove {
    Move(Selection),
    None,
}

/// `selectionKeyReducer` of `@betteroffice/xlsx` 0.3.0, as Desktop calls it, with no data: the
/// arrows step (Shift extends), with Cmd/Ctrl they go to the edge of `limits`; Enter steps down
/// and Shift+Enter up; Home goes to the row's first cell and Ctrl+Home to A1; End to its last
/// and Ctrl+End to the last cell; Page Up and Down step a page; Ctrl+A selects all of `limits`.
pub fn key(selection: Selection, key: &Key, mods: Mods, limits: Limits) -> KeyMove {
    let command = mods.cmd || mods.ctrl;
    let clamp = |row: i64, col: i64| -> Cell { [row.clamp(0, i64::from(limits.rows) - 1) as u32, col.clamp(0, i64::from(limits.cols) - 1) as u32] };
    let step = |dr: i64, dc: i64, extend: bool| {
        let f = selection.focus;
        let cell = clamp(i64::from(f[0]) + dr, i64::from(f[1]) + dc);
        KeyMove::Move(if extend { Selection { anchor: selection.anchor, focus: cell } } else { Selection::at(cell) })
    };
    let arrow = match key {
        Key::Up => Some((-1, 0)),
        Key::Down => Some((1, 0)),
        Key::Left => Some((0, -1)),
        Key::Right => Some((0, 1)),
        _ => None,
    };
    if let Some((dr, dc)) = arrow {
        if command {
            let f = selection.focus;
            let edge = match (dr, dc) {
                (-1, _) => [0, f[1]],
                (1, _) => [limits.rows - 1, f[1]],
                (_, -1) => [f[0], 0],
                _ => [f[0], limits.cols - 1],
            };
            return KeyMove::Move(if mods.shift { selection.extend_to(edge, Some([limits.rows, limits.cols])) } else { Selection::at(edge) });
        }
        return step(dr, dc, mods.shift);
    }
    let page = i64::from(limits.rows_per_page);
    match key {
        Key::Enter => step(if mods.shift { -1 } else { 1 }, 0, false),
        Key::Home if command => KeyMove::Move(Selection::at([0, 0])),
        Key::Home => KeyMove::Move(Selection::at([selection.focus[0], 0])),
        Key::End if command => KeyMove::Move(Selection::at([limits.rows - 1, limits.cols - 1])),
        Key::End => KeyMove::Move(Selection::at([selection.focus[0], limits.cols - 1])),
        Key::PageDown => step(page, 0, mods.shift),
        Key::PageUp => step(-page, 0, mods.shift),
        Key::Command(c) if c.eq_ignore_ascii_case(&'a') => KeyMove::Move(Selection { anchor: [0, 0], focus: [limits.rows - 1, limits.cols - 1] }),
        _ => KeyMove::None,
    }
}
