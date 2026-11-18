//! Per-row shaped `rune::GridRow` cache and vector-art / span builder for terminal screens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rune::{Color, GridRow, LineCap, Shape, Span, TextStyle};
use opencode_ui::palette::Pal;
use terminal::{CellFlags, RenderedCell, TerminalRow, TerminalSnapshot, UnderlineKind};

use crate::TerminalPane;
use crate::draw::{draw_cell_art, draw_special_underline};

/// Rows key: the terminal row's id and content hash, the font and cell, and the theme.
type RowKey = (u128, u64, (u8, u32, u32, u32, u32), bool);

thread_local! {
    /// Each terminal's shaped rows from its last frame, by tab: a row that is still on screen
    /// unchanged is handed back to the grid as it was.
    static GRID_ROWS: RefCell<HashMap<String, HashMap<RowKey, Rc<GridRow>>>> = RefCell::new(HashMap::new());
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_rows(
    pane: &TerminalPane,
    tab_id: &str,
    snap: &TerminalSnapshot,
    p: &Pal,
    style: TextStyle,
    style_key: (u8, u32, u32, u32, u32),
    cell_w: f32,
    cell_h: f32,
    scale: f32,
    slot_rows: usize,
) -> Vec<Rc<GridRow>> {
    GRID_ROWS.with_borrow_mut(|cache| {
        // A closed terminal's rows go with it.
        cache.retain(|id, _| {
            id == tab_id || pane.workspaces.values().any(|ws| ws.tabs.iter().any(|t| &t.id == id))
        });
        let cache = cache.entry(tab_id.to_owned()).or_default();
        let mut kept = HashMap::with_capacity(slot_rows);
        let key_of = |id: u128, hash: u64| (id, hash, style_key, p.dark);
        let rows: Vec<Rc<GridRow>> = (0..slot_rows)
            .map(|idx| {
                let key = snap.screen_rows.get(idx).map_or(key_of(0, 0), |r| key_of(r.id, r.content_hash));
                let row = cache.remove(&key).or_else(|| kept.get(&key).cloned()).unwrap_or_else(|| {
                    Rc::new(snap.screen_rows.get(idx).map_or_else(
                        || GridRow::new(String::new(), Vec::new(), style),
                        |r| grid_row(r, p, cell_w, cell_h, scale, style),
                    ))
                });
                kept.insert(key, row.clone());
                row
            })
            .collect();
        *cache = kept;
        rows
    })
}

/// A terminal row as the grid draws it: background and selection runs, the narrow cells' text
/// in one styled line, cells that must sit exactly on their column (wide, emoji, other scripts)
/// on their own, and box drawing, curly and double underlines and strikethrough as vectors.
fn grid_row(
    row_data: &TerminalRow,
    p: &Pal,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
    base_style: TextStyle,
) -> GridRow {
    let mut shape = Shape::new();
    let mut line_text = String::with_capacity(row_data.cells.len());
    let mut line_spans: Vec<Span> = Vec::new();
    let mut pinned: Vec<(u16, u16, String, TextStyle, Color)> = Vec::new();

    for (idx, cell) in row_data.cells.iter().enumerate() {
        let col_idx = idx as u16;
        if cell.flags.contains(CellFlags::WIDE_SPACER_TAIL) || cell.flags.contains(CellFlags::INVISIBLE) {
            line_text.push(' ');
            continue;
        }
        let fg = cell_fg_color(cell);
        if cell.flags.contains(CellFlags::STRIKETHROUGH) {
            let x0 = f32::from(col_idx) * cell_w;
            let x1 = x0 + f32::from(cell.width.max(1)) * cell_w;
            let ym = (cell_h * 0.52).round();
            shape = shape.line([x0, ym], [x1, ym], 1.0, LineCap::Butt, fg);
        }
        if !matches!(cell.underline, UnderlineKind::None | UnderlineKind::Single) {
            let ul_c = cell.underline_color.map_or(fg, |c| c.to_rune());
            shape = draw_special_underline(
                shape,
                cell.underline,
                f32::from(col_idx) * cell_w,
                f32::from(cell.width.max(1)) * cell_w,
                cell_h,
                ul_c,
            );
        }
        if let Some(ch) = single_char(&cell.grapheme)
            && draw_cell_art(&mut shape, ch, f32::from(col_idx) * cell_w, cell_w, cell_h, scale, fg)
        {
            line_text.push(' ');
            continue;
        }
        // Wide glyphs, emoji and other scripts sit exactly on their cells, so a fallback font's
        // advances can't move the columns after them.
        if cell.width != 1 || !is_grid_safe_monospaced(&cell.grapheme) {
            line_text.push(' ');
            if !cell.grapheme.is_empty() && cell.grapheme != " " {
                let mut cell_style = base_style;
                if cell.flags.contains(CellFlags::BOLD) {
                    cell_style.weight = 700.0;
                }
                if cell.flags.contains(CellFlags::ITALIC) {
                    cell_style.italic = true;
                }
                pinned.push((col_idx, u16::from(cell.width.max(1)), cell.grapheme.to_string(), cell_style, fg));
            }
            continue;
        }
        let byte_start = line_text.len();
        if cell.grapheme.is_empty() {
            line_text.push(' ');
        } else {
            line_text.push_str(&cell.grapheme);
        }
        let byte_end = line_text.len();
        let weight = cell.flags.contains(CellFlags::BOLD).then_some(700.0);
        let italic = cell.flags.contains(CellFlags::ITALIC);
        let underline = (cell.underline == UnderlineKind::Single)
            .then(|| cell.underline_color.map_or(fg, |c| c.to_rune()));
        if let Some(last) = line_spans.last_mut()
            && last.end as usize == byte_start
            && last.color == Some(fg)
            && last.weight == weight
            && last.italic == italic
            && last.underline == underline
        {
            last.end = byte_end as u32;
        } else {
            let mut sp = Span::new(byte_start, byte_end).color(fg);
            sp.weight = weight;
            sp.italic = italic;
            sp.underline = underline;
            line_spans.push(sp);
        }
    }
    let trimmed_len = line_text.trim_end_matches(' ').len();
    line_text.truncate(trimmed_len);
    line_spans.retain_mut(|sp| {
        if sp.start as usize >= trimmed_len {
            return false;
        }
        sp.end = sp.end.min(trimmed_len as u32);
        sp.end > sp.start
    });

    // A line the terminal wrapped at the edge reads on into the next row as one line.
    let mut row = GridRow::new(line_text, line_spans, base_style).wraps(row_data.wrapped);
    for span in &row_data.bg_spans {
        if let Some(bg) = span.color {
            row = row.fill(span.start_col, span.end_col, bg.to_rune());
        }
        if span.selected {
            row = row.fill(span.start_col, span.end_col, p.selection);
        }
    }
    for (col, width, text, style, fg) in pinned {
        row = row.cell(col, width, text, style, fg);
    }
    row.shape(shape)
}

pub(super) fn cell_fg_color(cell: &RenderedCell) -> Color {
    let c = cell.fg.to_rune();
    if cell.flags.contains(CellFlags::FAINT) {
        c.alpha(0.55)
    } else {
        c
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    let ch = it.next()?;
    it.next().is_none().then_some(ch)
}

fn is_grid_safe_monospaced(s: &str) -> bool {
    let Some(ch) = single_char(s) else { return false };
    ch.is_ascii_graphic() || ch == ' ' || ('\u{00A0}'..='\u{024F}').contains(&ch)
}
