//! `ui.officeSheet`: a worksheet as Desktop's `SpreadsheetGrid` draws it (`spreadsheet.tsx`,
//! `sheet-paint.ts`): Excel's light column and row headers along the edges, the cells under the
//! scroll position as one frame the workbook's thread draws, and the selection over them, tinted
//! and outlined in Excel's green with its headers lit, all on white paper in every theme. A frame
//! drawn for another scroll position keeps its place under the headers until the next one
//! arrives. Under the cells, the grid screen readers hear: the block of cells around the active
//! cell, as text.

pub mod copy;
mod input;
pub mod pointer;
pub mod select;
pub mod state;

use office_preview::xlsx::{LAST_CELL, Range};
use rune::{Color, Cursor, El, Family, LineCap, ScrollPos, Shape, TextStyle, canvas, col, el, memo, text};

use super::OfficeHost;
use super::registry::{self, Snapshot};
use super::worker::SheetFrame;
pub use input::run;
use select::{contains, cover_merges, hidden_at};

/// `sheet-paint.ts`'s colours: Excel's, kept in every theme.
fn ink_header() -> Color { rune::color::hex("#f5f5f5").unwrap_or(Color::BLACK) }
fn ink_label() -> Color { rune::color::hex("#5c5c5c").unwrap_or(Color::BLACK) }
fn ink_line() -> Color { rune::color::hex("#d4d4d4").unwrap_or(Color::BLACK) }
fn ink_edge() -> Color { rune::color::hex("#bdbdbd").unwrap_or(Color::BLACK) }
fn ink_selected_header() -> Color { rune::color::hex("#d3ecdd").unwrap_or(Color::BLACK) }
fn ink_selected_label() -> Color { rune::color::hex("#185c37").unwrap_or(Color::BLACK) }
fn ink_accent() -> Color { rune::color::hex("#217346").unwrap_or(Color::BLACK) }
/// `rgba(33, 115, 70, 0.12)`: the selected cells' tint.
fn ink_selection() -> Color { Color { a: 0.12, ..ink_accent() } }

/// The column headers' height (`headerHeight`).
pub const HEADER_H: f32 = 20.0;
/// `11px ui-sans-serif, system-ui, sans-serif`: Segoe UI on Windows, SF on macOS.
const LABEL_STYLE: TextStyle = TextStyle::new(11.0, 400.0).lh(13.0).family(Family::Named("Segoe UI, SF Pro Text"));

/// The words the grid says, in the plugin's language.
#[derive(Clone, Default)]
pub struct Labels {
    pub name: String,
    pub too_large: String,
    pub failed: String,
}

/// The key of file `id`'s scroll view, which a reveal scrolls.
pub fn scroll_key(id: u64) -> String {
    format!("office-sheet-{id}")
}

pub fn view<H: OfficeHost>(id: u64, labels: Labels, focus: Color) -> El {
    let snap = registry::snapshot(id).unwrap_or_default();
    let layout = snap.layout.clone().unwrap_or_default();
    let header_w = row_header_width(&snap);
    let (content_w, content_h) = (layout.width as f32, layout.height as f32);
    let anchor = snap.selection.anchor;
    let range = cover_merges(snap.selection.range(), &layout.merges);
    let active = cover_merges([anchor[0], anchor[1], anchor[0], anchor[1]], &layout.merges);
    let frozen = snap.frame.as_deref().map_or([0.0; 2], |f| frozen_size(f, layout.frozen_rows, layout.frozen_cols));
    // As in Excel, the sheet scrolls one screen past the farthest point seen (`area`): its cells,
    // a cell a key or link scrolled to, or where it is scrolled now.
    let v = snap.view;
    let cells = [(v.size[0] - header_w).max(0.0), (v.size[1] - HEADER_H).max(0.0)];
    let content_w = content_w.max(snap.reach[0]).max(v.scroll[0] + cells[0]) + cells[0];
    let content_h = content_h.max(snap.reach[1]).max(v.scroll[1] + cells[1]) + cells[1];
    let mut content = el().key("office-sheet-content").size(header_w + content_w.max(1.0), HEADER_H + content_h.max(1.0));
    if let Some(frame) = &snap.frame {
        let [x, y, w, h] = frame.viewport.map(|v| v as f32);
        content = content.child(el().key("office-sheet-cells").absolute(header_w + x, HEADER_H + y).size(w, h).fill(Color::WHITE).effect(registry::sheet_mark(id))).child(selection(frame, header_w, range, active)).children(links(frame, header_w, &labels.name));
    }
    // A drag steps the selection every frame: the strips build again only when the frame or the
    // tracks they light change, and each header only when its own place or lighting does.
    let frame_key = snap.frame.as_ref().map_or(0, |f| std::sync::Arc::as_ptr(f) as usize);
    content = content
        .child(memo(format!("office-sheet-columns-{id}"), (frame_key, header_w.to_bits(), content_w.to_bits(), range[1], range[3]), || column_headers(&snap, header_w, content_w, range)))
        .child(memo(format!("office-sheet-rows-{id}"), (frame_key, header_w.to_bits(), content_h.to_bits(), range[0], range[2]), || row_headers(&snap, header_w, content_h, range)));
    let key = scroll_key(id);
    let mut grid = col()
        .key(key.clone())
        .role("grid")
        .label(labels.name.clone())
        .read_only(true)
        .focusable(true)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(Color::WHITE)
        .cursor(Cursor::Cell)
        .scroll_xy()
        .on_scroll_offset(move |at: ScrollPos| registry::scrolled(id, at.offset, at.viewport, header_w, frozen));
    if let Some(cell) = active_cell(&snap) {
        grid = grid.active_descendant(cell);
    }
    input::keys::<H>(input::pointer::<H>(grid, id, &key), id, labels).child(content).child(reader(&snap, range)).child(ring(v.size, header_w + content_w.max(1.0), focus))
}

/// The keyboard's ring, over the cells and headers as Desktop's overlay lies over its canvas
/// (`peer-focus-visible:shadow-[inset_0_0_0_2px_var(--v2-border-border-focus)]`): 2 wide just
/// inside the view, held there as the sheet scrolls. The pointer passes through it.
fn ring(size: [f32; 2], content_w: f32, focus: Color) -> El {
    let frame = el().key("office-sheet-ring").absolute(0.0, 0.0).size(size[0], size[1]).sticky_left(0.0).focus_ring(2.0, -2.0, focus);
    el().key("office-sheet-ring-strip").absolute(0.0, 0.0).w(content_w).h(size[1]).sticky_top(0.0).paint_order(1).inert().child(frame)
}

/// The row headers fit the longest row number on show (`rowHeaderWidth`): at least 28.
fn row_header_width(snap: &Snapshot) -> f32 {
    let last = snap.frame.as_ref().map_or(1, |f| track(f.grid.start_row, f.grid.row_indices.as_deref(), f.grid.row_offsets.len().saturating_sub(1)) + 1);
    let digits = last.max(1).to_string().len() as f32;
    (digits * 6.2 + 12.0).ceil().max(28.0)
}

/// The frozen panes' size in a frame: the offset of the first track past them (`frozenSize`).
fn frozen_size(frame: &SheetFrame, rows: u32, cols: u32) -> [f32; 2] {
    let pane = |offsets: &[f32], start: u32, indices: Option<&[u32]>, count: u32| -> f32 {
        if count == 0 {
            return 0.0;
        }
        let first = (0..offsets.len()).find(|&p| p == offsets.len() - 1 || track(start, indices, p) >= count).unwrap_or(0);
        offsets.get(first).copied().unwrap_or(0.0).max(0.0)
    };
    let g = &frame.grid;
    [pane(&g.col_offsets, g.start_col, g.col_indices.as_deref(), cols), pane(&g.row_offsets, g.start_row, g.row_indices.as_deref(), rows)]
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

/// The strip of column letters, held at the top as the sheet scrolls down; the selected columns
/// lit.
fn column_headers(snap: &Snapshot, header_w: f32, content_w: f32, range: Range) -> El {
    let width = header_w + content_w.max(1.0);
    let strip = el().key("office-sheet-columns").w(width).h(HEADER_H).fill(ink_header()).sticky_top(0.0).cursor(Cursor::Default);
    let (mut paint, mut labels) = (Shape::new(), Vec::new());
    if let Some(frame) = &snap.frame {
        let x0 = header_w + frame.viewport[0] as f32;
        let offsets = &frame.grid.col_offsets;
        for (position, pair) in offsets.windows(2).enumerate() {
            let index = track(frame.grid.start_col, frame.grid.col_indices.as_deref(), position);
            let selected = index >= range[1] && index <= range[3];
            let (x, w) = (x0 + pair[0], pair[1] - pair[0]);
            paint = header_paint(paint, [x, 0.0, w, HEADER_H], selected, true, header_w..width);
            labels.push(memo(format!("office-col-{index}"), (x.to_bits(), w.to_bits(), selected), || header_label(x, 0.0, w, HEADER_H, column_name(index), selected, true)));
        }
    }
    // `paintHeaders` draws the edge along the cells over every header.
    paint = paint.line([0.0, HEADER_H], [width, HEADER_H], 1.0, LineCap::Butt, ink_edge());
    // The canvas under the labels, as Desktop's draws its tints before its text.
    strip.child(canvas(paint).key("office-sheet-columns-paint").absolute(0.0, 0.0).size(width, HEADER_H + 1.0).inert()).children(labels).child(corner(header_w))
}

/// The strip of row numbers, held at the left as the sheet scrolls across; the selected rows lit.
fn row_headers(snap: &Snapshot, header_w: f32, content_h: f32, range: Range) -> El {
    let height = content_h.max(1.0);
    let strip = el().key("office-sheet-rows").absolute(0.0, HEADER_H).w(header_w).h(height).fill(ink_header()).sticky_left(0.0).cursor(Cursor::Default);
    let (mut paint, mut labels) = (Shape::new(), Vec::new());
    if let Some(frame) = &snap.frame {
        let y0 = frame.viewport[1] as f32;
        let offsets = &frame.grid.row_offsets;
        for (position, pair) in offsets.windows(2).enumerate() {
            let index = track(frame.grid.start_row, frame.grid.row_indices.as_deref(), position);
            let selected = index >= range[0] && index <= range[2];
            let (y, h) = (y0 + pair[0], pair[1] - pair[0]);
            paint = header_paint(paint, [0.0, y, header_w, h], selected, false, 0.0..height);
            labels.push(memo(format!("office-row-{index}"), (y.to_bits(), h.to_bits(), header_w.to_bits(), selected), || header_label(0.0, y, header_w, h, (index + 1).to_string(), selected, false)));
        }
    }
    paint = paint.line([header_w, 0.0], [header_w, height], 1.0, LineCap::Butt, ink_edge());
    strip.child(canvas(paint).key("office-sheet-rows-paint").absolute(0.0, 0.0).size(header_w + 1.0, height).inert()).children(labels)
}

/// The select-all corner over both strips, with Excel's small triangle pointing at the cells and
/// the two edges where they pass it.
fn corner(header_w: f32) -> El {
    let paint = Shape::new()
        .fill(vec![[header_w - 4.0, HEADER_H - 12.0], [header_w - 4.0, HEADER_H - 4.0], [header_w - 12.0, HEADER_H - 4.0]], rune::color::hex("#c8c8c8").unwrap_or(Color::BLACK))
        .line([0.0, HEADER_H], [header_w, HEADER_H], 1.0, LineCap::Butt, ink_edge())
        .line([header_w, 0.0], [header_w, HEADER_H], 1.0, LineCap::Butt, ink_edge());
    el().key("office-sheet-corner").absolute(0.0, 0.0).size(header_w, HEADER_H).fill(ink_header()).sticky_left(0.0).child(canvas(paint).absolute(0.0, 0.0).size(header_w + 1.0, HEADER_H + 1.0))
}

/// One header's paint, after `paint`, as `paintHeader` draws it on Desktop's canvas, at its exact
/// place: tinted with a bar toward the cells when selected, then the 1 px line after it, centred
/// on its edge where the gridline meets it. Like the canvas, it lands between device pixels at
/// any scale (a 1.5 px line at 1.5x), and the next header's tint covers this line's far half.
/// `clip` is the strip's run along the headers, which the tint stays inside.
fn header_paint(paint: Shape, [x, y, w, h]: [f32; 4], selected: bool, columns: bool, clip: std::ops::Range<f32>) -> Shape {
    if w <= 0.0 || h <= 0.0 {
        return paint;
    }
    let rect = |paint: Shape, [x, y, w, h]: [f32; 4], c: Color| {
        let (lo, hi) = if columns { (x.max(clip.start), (x + w).min(clip.end)) } else { (y.max(clip.start), (y + h).min(clip.end)) };
        if hi <= lo {
            return paint;
        }
        let [x0, y0, x1, y1] = if columns { [lo, y, hi, y + h] } else { [x, lo, x + w, hi] };
        paint.fill(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], c)
    };
    let mut paint = paint;
    if selected {
        paint = rect(paint, [x, y, w, h], ink_selected_header());
        paint = rect(paint, if columns { [x, y + h - 2.0, w, 2.0] } else { [x + w - 2.0, y, 2.0, h] }, ink_accent());
    }
    let (a, b) = if columns { ([x + w, y], [x + w, y + h]) } else { ([x, y + h], [x + w, y + h]) };
    paint.line(a, b, 1.0, LineCap::Butt, ink_line())
}

/// One header's label, centred in it; a track too small for its label shows none.
fn header_label(x: f32, y: f32, w: f32, h: f32, label: String, selected: bool, columns: bool) -> El {
    let fits = if columns { label.len() as f32 * 6.2 + 2.0 <= w } else { h >= 9.0 };
    if w <= 0.0 || h <= 0.0 || !fits {
        return el();
    }
    el().absolute(x, y).size(w, h).center().inert().child(text(label).font(LABEL_STYLE).color(if selected { ink_selected_label() } else { ink_label() }))
}

/// Where a range's tracks on show sit in the frame (`rangeRect`): x, y, width, height; `None`
/// when none of its rows or columns shows.
fn range_rect(frame: &SheetFrame, range: Range) -> Option<[f32; 4]> {
    let g = &frame.grid;
    let span = |start: u32, indices: Option<&[u32]>, offsets: &[f32], low: u32, high: u32| -> Option<(f32, f32)> {
        let tracks = offsets.len().saturating_sub(1);
        let mut found: Option<(usize, usize)> = None;
        for p in 0..tracks {
            let t = track(start, indices, p);
            if t < low {
                continue;
            }
            if t > high {
                break;
            }
            found = Some((found.map_or(p, |f| f.0), p));
        }
        let (first, last) = found?;
        Some((offsets[first], offsets[last + 1] - offsets[first]))
    };
    let (x, w) = span(g.start_col, g.col_indices.as_deref(), &g.col_offsets, range[1], range[3])?;
    let (y, h) = span(g.start_row, g.row_indices.as_deref(), &g.row_offsets, range[0], range[2])?;
    Some([x, y, w, h])
}

/// The selected range tinted, its active cell left clear, and outlined 2 wide in Excel's green
/// (`paintSelection`), to the frame's edge where it runs past it.
fn selection(frame: &SheetFrame, header_w: f32, range: Range, active: Range) -> El {
    let Some([x, y, w, h]) = range_rect(frame, range) else { return el() };
    let (ox, oy) = (header_w + frame.viewport[0] as f32, HEADER_H + frame.viewport[1] as f32);
    let mut layer = el().key("office-sheet-selection").absolute(ox + x - 1.0, oy + y - 1.0).size(w + 2.0, h + 2.0);
    let hole = range_rect(frame, active);
    if hole != Some([x, y, w, h]) {
        // The tint with the active cell cut out (`fill("evenodd")`): the bands around it.
        let bands = match hole {
            Some([hx, hy, hw, hh]) => {
                let (hx, hy) = (hx - x, hy - y);
                vec![[0.0, 0.0, w, hy], [0.0, hy + hh, w, h - hy - hh], [0.0, hy, hx, hh], [hx + hw, hy, w - hx - hw, hh]]
            }
            None => vec![[0.0, 0.0, w, h]],
        };
        for [bx, by, bw, bh] in bands.into_iter().filter(|b| b[2] > 0.0 && b[3] > 0.0) {
            layer = layer.child(el().absolute(1.0 + bx, 1.0 + by).size(bw, bh).fill(ink_selection()));
        }
    }
    layer.child(el().absolute(0.0, 0.0).size(w + 2.0, h + 2.0).border(2.0, ink_accent()))
}

/// The links on show that a click follows, each over its cells: the pointer's hand there, and a
/// tooltip of where it goes under any text of its own (`linkTitle`).
fn links<'a>(frame: &'a SheetFrame, header_w: f32, sheet: &'a str) -> impl Iterator<Item = El> + 'a {
    let (ox, oy) = (header_w + frame.viewport[0] as f32, HEADER_H + frame.viewport[1] as f32);
    frame.links.iter().enumerate().filter_map(move |(i, link)| {
        let url = office_preview::xlsx::link_url(link);
        let location = link.location.as_deref().filter(|l| office_preview::xlsx::link_location(l, sheet).is_some());
        let target = url.or_else(|| location.map(str::to_owned))?;
        let [x, y, w, h] = range_rect(frame, [link.top, link.left, link.bottom, link.right])?;
        let title = match link.tooltip.as_deref() {
            Some(tip) if tip != target => format!("{tip}\n{target}"),
            _ => target,
        };
        Some(el().key(format!("office-sheet-link-{i}")).absolute(ox + x, oy + y).size(w, h).cursor(Cursor::Pointer).help(title))
    })
}

/// The key of the screen readers' cell `cell`.
fn cell_key(cell: [u32; 2]) -> String {
    format!("office-cell-{}-{}", cell[0], cell[1])
}

/// The cell keyboard moves reach, which screen readers announce, when the block holds it.
fn active_cell(snap: &Snapshot) -> Option<String> {
    let read = snap.read.as_ref()?;
    let focus = snap.selection.focus;
    contains(read.range, focus).then(|| cell_key(focus))
}

/// The grid screen readers hear (`sr-only`): the block's rows and columns but the hidden ones,
/// each cell's text as it shows, the selected ones marked. Nothing of it draws.
fn reader(snap: &Snapshot, range: Range) -> El {
    let Some(read) = &snap.read else { return el() };
    let layout = snap.layout.clone().unwrap_or_default();
    let [top, left, bottom, right] = read.range;
    let columns: Vec<u32> = (left..=right.min(LAST_CELL[1])).filter(|c| !hidden_at(&layout.hidden_cols, *c)).collect();
    let mut head = el().role("row").child(el().role("columnheader"));
    for c in &columns {
        head = head.child(el().role("columnheader").label(column_name(*c)));
    }
    let mut grid = el().key("office-sheet-reader").absolute(0.0, 0.0).size(1.0, 1.0).clip_rounded(0.0).child(head);
    for r in (top..=bottom.min(LAST_CELL[0])).filter(|r| !hidden_at(&layout.hidden_rows, *r)) {
        // A row builds again only when the block or the selected part of it changes, so a drag
        // down rebuilds the row it reaches, not the block.
        let span = (r >= range[0] && r <= range[2]).then(|| (range[1].max(left), range[3].min(right)));
        let key = (std::rc::Rc::as_ptr(read) as usize, std::sync::Arc::as_ptr(&layout) as usize, r, span);
        grid = grid.child(memo(format!("office-reader-row-{r}"), key, || {
            let mut row = el().role("row").child(el().role("rowheader").label((r + 1).to_string()));
            for c in &columns {
                let text = read.texts.get((r - top) as usize).and_then(|t| t.get((*c - left) as usize)).cloned().unwrap_or_default();
                row = row.child(el().key(cell_key([r, *c])).role("gridcell").label(text).focus_selected(contains(range, [r, *c])));
            }
            row
        }));
    }
    grid
}
