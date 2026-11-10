//! `ui.officeSheet`: a worksheet as Desktop's `SpreadsheetGrid` draws it (`spreadsheet.tsx`,
//! `sheet-paint.ts`): Excel's light column and row headers along the edges, the cells under the
//! scroll position as one frame the workbook's thread draws, and the active cell outlined in
//! Excel's green, all on white paper in every theme. A frame drawn for another scroll position
//! keeps its place under the headers until the next one arrives.

use rune::{Color, El, ScrollPos, TextStyle, col, el, text};

use super::registry::{self, Snapshot};

/// `sheet-paint.ts`'s colours: Excel's, kept in every theme.
fn ink_header() -> Color { rune::color::hex("#f5f5f5").unwrap_or(Color::BLACK) }
fn ink_label() -> Color { rune::color::hex("#5c5c5c").unwrap_or(Color::BLACK) }
fn ink_line() -> Color { rune::color::hex("#d4d4d4").unwrap_or(Color::BLACK) }
fn ink_edge() -> Color { rune::color::hex("#bdbdbd").unwrap_or(Color::BLACK) }
fn ink_selected_header() -> Color { rune::color::hex("#d3ecdd").unwrap_or(Color::BLACK) }
fn ink_selected_label() -> Color { rune::color::hex("#185c37").unwrap_or(Color::BLACK) }
fn ink_accent() -> Color { rune::color::hex("#217346").unwrap_or(Color::BLACK) }

/// The column headers' height (`headerHeight`).
const HEADER_H: f32 = 20.0;
const LABEL_STYLE: TextStyle = TextStyle::new(11.0, 440.0).lh(13.0);

pub fn view(id: u64) -> El {
    let snap = registry::snapshot(id).unwrap_or_default();
    let layout = snap.layout.unwrap_or_default();
    let header_w = row_header_width(&snap);
    let (content_w, content_h) = (layout.width as f32, layout.height as f32);
    let mut content = el().key("office-sheet-content").size(header_w + content_w.max(1.0), HEADER_H + content_h.max(1.0));
    if let Some(frame) = &snap.frame {
        let [x, y, w, h] = frame.viewport.map(|v| v as f32);
        content = content.child(el().key("office-sheet-cells").absolute(header_w + x, HEADER_H + y).size(w, h).fill(Color::WHITE).effect(registry::sheet_mark(id))).child(selection(&snap, header_w));
    }
    content = content.child(column_headers(&snap, header_w, content_w)).child(row_headers(&snap, header_w, content_h));
    col()
        .key("office-sheet")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(Color::WHITE)
        .scroll_xy()
        .on_scroll_offset(move |at: ScrollPos| {
            let (w, h) = (at.viewport[0] - header_w, at.viewport[1] - HEADER_H);
            registry::show_sheet(id, [f64::from(at.offset[0]), f64::from(at.offset[1]), f64::from(w), f64::from(h)]);
        })
        .child(content)
}

/// The row headers fit the longest row number on show (`rowHeaderWidth`): at least 28.
fn row_header_width(snap: &Snapshot) -> f32 {
    let last = snap.frame.as_ref().map_or(1, |f| track(f.grid.start_row, f.grid.row_indices.as_deref(), f.grid.row_offsets.len().saturating_sub(1)) + 1);
    let digits = last.max(1).to_string().len() as f32;
    (digits * 6.2 + 12.0).ceil().max(28.0)
}

/// The sheet row or column at `position` in a frame's grid (`trackIndex`).
fn track(start: u32, indices: Option<&[u32]>, position: usize) -> u32 {
    indices.and_then(|i| i.get(position).copied()).unwrap_or(start + position as u32)
}

/// A column's letters (`columnName`): A..Z, AA...
fn column_name(index: u32) -> String {
    let mut n = index + 1;
    let mut name = Vec::new();
    while n > 0 {
        n -= 1;
        name.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

/// The strip of column letters, held at the top as the sheet scrolls down.
fn column_headers(snap: &Snapshot, header_w: f32, content_w: f32) -> El {
    let mut strip = el().key("office-sheet-columns").w(header_w + content_w.max(1.0)).h(HEADER_H).fill(ink_header()).sticky_top(0.0);
    if let Some(frame) = &snap.frame {
        let x0 = header_w + frame.viewport[0] as f32;
        let offsets = &frame.grid.col_offsets;
        for (position, pair) in offsets.windows(2).enumerate() {
            let index = track(frame.grid.start_col, frame.grid.col_indices.as_deref(), position);
            strip = strip.child(header_cell(x0 + pair[0], 0.0, pair[1] - pair[0], HEADER_H, column_name(index), index <= active_end(snap)[1], true));
        }
    }
    strip.child(el().absolute(0.0, HEADER_H - 1.0).w(header_w + content_w.max(1.0)).h(1.0).fill(ink_edge())).child(corner(header_w))
}

/// The strip of row numbers, held at the left as the sheet scrolls across.
fn row_headers(snap: &Snapshot, header_w: f32, content_h: f32) -> El {
    let mut strip = el().key("office-sheet-rows").absolute(0.0, HEADER_H).w(header_w).h(content_h.max(1.0)).fill(ink_header()).sticky_left(0.0);
    if let Some(frame) = &snap.frame {
        let y0 = frame.viewport[1] as f32;
        let offsets = &frame.grid.row_offsets;
        for (position, pair) in offsets.windows(2).enumerate() {
            let index = track(frame.grid.start_row, frame.grid.row_indices.as_deref(), position);
            strip = strip.child(header_cell(0.0, y0 + pair[0], header_w, pair[1] - pair[0], (index + 1).to_string(), index <= active_end(snap)[0], false));
        }
    }
    strip.child(el().absolute(header_w - 1.0, 0.0).w(1.0).h(content_h.max(1.0)).fill(ink_edge()))
}

/// The select-all corner over both strips, with Excel's small triangle pointing at the cells.
fn corner(header_w: f32) -> El {
    let triangle = rune::Shape::new().fill(vec![[header_w - 4.0, HEADER_H - 12.0], [header_w - 4.0, HEADER_H - 4.0], [header_w - 12.0, HEADER_H - 4.0]], rune::color::hex("#c8c8c8").unwrap_or(Color::BLACK));
    el().key("office-sheet-corner").absolute(0.0, 0.0).size(header_w, HEADER_H).fill(ink_header()).sticky_left(0.0).child(rune::canvas(triangle).absolute(0.0, 0.0).size(header_w, HEADER_H)).child(el().absolute(0.0, HEADER_H - 1.0).w(header_w).h(1.0).fill(ink_edge())).child(el().absolute(header_w - 1.0, 0.0).w(1.0).h(HEADER_H).fill(ink_edge()))
}

/// One header: its label centred, tinted with a bar toward the cells when selected, and the line
/// after it where the gridline meets it.
fn header_cell(x: f32, y: f32, w: f32, h: f32, label: String, selected: bool, columns: bool) -> El {
    if w <= 0.0 || h <= 0.0 {
        return el();
    }
    let mut cell = el().absolute(x, y).size(w, h).center().fill(if selected { ink_selected_header() } else { ink_header() });
    // A track too small for its label shows none.
    let fits = if columns { label.len() as f32 * 6.2 + 2.0 <= w } else { h >= 9.0 };
    if fits {
        cell = cell.child(text(label).font(LABEL_STYLE).color(if selected { ink_selected_label() } else { ink_label() }));
    }
    if selected {
        cell = cell.child(if columns { el().absolute(0.0, h - 2.0).w(w).h(2.0).fill(ink_accent()) } else { el().absolute(w - 2.0, 0.0).w(2.0).h(h).fill(ink_accent()) });
    }
    let rule = if columns { el().absolute(w - 1.0, 0.0).w(1.0).h(h).fill(ink_line()) } else { el().absolute(0.0, h - 1.0).w(w).h(1.0).fill(ink_line()) };
    cell.child(rule)
}

/// The last row and column of the active range: A1 and the merged cells it reaches.
fn active_end(snap: &Snapshot) -> [u32; 2] {
    snap.layout.map_or([0, 0], |l| l.active_end)
}

/// The active range, A1 and the merged cells it reaches, outlined 2 wide in Excel's green
/// (`paintSelection`), to the frame's edge where it runs past it.
fn selection(snap: &Snapshot, header_w: f32) -> El {
    let Some(frame) = &snap.frame else { return el() };
    let grid = &frame.grid;
    let [last_row, last_col] = active_end(snap);
    let span = |start: u32, indices: Option<&[u32]>, offsets: &[f32], last: u32| -> Option<(f32, f32)> {
        let tracks = offsets.len().saturating_sub(1);
        let first = (0..tracks).find(|p| track(start, indices, *p) == 0)?;
        let end = (first..tracks).take_while(|p| track(start, indices, *p) <= last).last().unwrap_or(first);
        Some((offsets[first], offsets[end + 1] - offsets[first]))
    };
    let (Some((x, w)), Some((y, h))) = (span(grid.start_col, grid.col_indices.as_deref(), &grid.col_offsets, last_col), span(grid.start_row, grid.row_indices.as_deref(), &grid.row_offsets, last_row)) else {
        return el();
    };
    el().key("office-sheet-selection").absolute(header_w + frame.viewport[0] as f32 + x - 1.0, HEADER_H + frame.viewport[1] as f32 + y - 1.0).size(w + 2.0, h + 2.0).border(2.0, ink_accent())
}