//! Excel (`spreadsheet.worker.ts`): the workbook opens for reading, and each scroll position of a
//! sheet is a frame, a display list of the cells in that viewport with the grid that places them,
//! drawn as a scene.

mod scene;
mod text;

use anyhow::{Context, Result};
use betteroffice_xlsx::{CalculationOptions, CellRef, SheetId, Viewport, Workbook};
use betteroffice_xlsx::{RangeAddress, RangeTarget, ReadRequest, XlsxAnchor, XlsxExportOptions, XlsxExportScope};

use crate::shared::PageScene;

pub use betteroffice_xlsx::GridMeta;

pub struct Xlsx {
    workbook: Workbook,
    pub sheets: Vec<String>,
}

/// A range of cells, zero-based and inclusive: top, left, bottom, right (`CellRange`).
pub type Range = [u32; 4];

/// A sheet's extent, frozen panes and what selecting on it needs (`spreadsheet.worker.ts`
/// `show`): its used range, merged cells and hidden rows and columns. CSS px.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetLayout {
    pub width: f64,
    pub height: f64,
    pub frozen_rows: u32,
    pub frozen_cols: u32,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub used: Option<Range>,
    pub merges: Vec<Range>,
    /// Runs of hidden rows and columns, zero-based and inclusive (`parseSpans`).
    pub hidden_rows: Vec<[u32; 2]>,
    pub hidden_cols: Vec<[u32; 2]>,
}

/// One cell as it shows, for a copy or a screen reader (`readCells`).
pub struct CellText {
    pub text: String,
    /// Pastes as it is: a number shown with a sign (`literal`), which a spreadsheet must not
    /// read as text.
    pub literal: bool,
}

/// One viewport of a sheet: its cells as a scene, and the grid its headers follow.
pub struct SheetFrame {
    pub page: PageScene,
    pub grid: GridMeta,
}

impl Xlsx {
    /// Opens `bytes` and calculates its formulas as Desktop's engine does at open
    /// (`Session::open`), so formulas the file stored no result for show one; `now_serial` is the
    /// local time as a 1900-system serial, for `NOW()` and `TODAY()`.
    pub fn open(bytes: &[u8], now_serial: Option<f64>) -> Result<Xlsx> {
        let workbook = Workbook::open_recalculated(bytes, CalculationOptions { now_serial })?;
        let sheets = (0..workbook.sheet_count()).map(|i| workbook.sheet(SheetId(i as u32)).map(|s| s.name.clone()).unwrap_or_default()).collect();
        Ok(Xlsx { workbook, sheets })
    }

    pub fn layout(&mut self, sheet: usize) -> Result<SheetLayout> {
        self.workbook.set_active_sheet(SheetId(u32::try_from(sheet).context("sheet index")?))?;
        let info = self.workbook.sheet_info()?;
        let mut layout = SheetLayout {
            width: f64::from(info.content_width),
            height: f64::from(info.content_height),
            frozen_rows: info.frozen_rows,
            frozen_cols: info.frozen_cols,
            scroll_x: f64::from(info.initial_scroll_x),
            scroll_y: f64::from(info.initial_scroll_y),
            ..SheetLayout::default()
        };
        // One cell is enough: the lists before the cells are complete however soon the export
        // stops (`spreadsheet.worker.ts` `layout`).
        let options = XlsxExportOptions {
            scope: Some(vec![XlsxExportScope { sheet: sheet as u32, range: Some("A1:XFD1048576".into()) }]),
            max_cells: Some(1),
            include_defined_names: Some(false),
            ..XlsxExportOptions::default()
        };
        if let Ok(Ok(export)) = self.workbook.export_structured(&options)
            && let Some(exported) = export.content.sheets.first()
        {
            layout.used = exported.used_range.as_deref().and_then(parse_range);
            layout.merges = exported.merges.iter().filter_map(|m| anchor_range(&m.anchor)).collect();
            layout.hidden_rows = spans(&exported.hidden_rows, false);
            layout.hidden_cols = spans(&exported.hidden_columns, true);
        }
        Ok(layout)
    }

    /// The cells of `range` on `sheet` as they show, row by row (`read`).
    pub fn cells(&self, sheet: usize, range: Range) -> Result<Vec<Vec<CellText>>> {
        let [top, left, bottom, right] = range;
        let target = RangeTarget {
            sheet_id: format!("sheet:{sheet}"),
            range: RangeAddress::RowCol { start: betteroffice_xlsx::CellPosition { row: top, col: left }, end: betteroffice_xlsx::CellPosition { row: bottom, col: right } },
        };
        let read = self.workbook.read_cells(&ReadRequest { ranges: vec![target] })?.map_err(|refusal| anyhow::anyhow!("{refusal:?}"))?;
        let cells = read.ranges.into_iter().next().map(|r| r.cells).unwrap_or_default();
        Ok(cells.into_iter().map(|row| row.into_iter().map(|cell| CellText { literal: literal(&cell), text: cell.display_text }).collect()).collect())
    }

    /// Where cell `row`, `col` of `sheet` starts and the next one does, in the sheet's scroll
    /// coordinates (`position`): x, y, then the cell after it diagonally.
    pub fn position(&self, sheet: usize, row: u32, col: u32) -> Result<[f64; 4]> {
        let rect = self.workbook.cell_rect(SheetId(u32::try_from(sheet).context("sheet index")?), CellRef::new(row, col))?;
        Ok([f64::from(rect.x), f64::from(rect.y), f64::from(rect.x + rect.w), f64::from(rect.y + rect.h)])
    }

    /// The cells of `sheet` in the viewport at `x`, `y` (the body's scroll; frozen panes stay),
    /// `width` by `height` px, their bordered lines snapped to device pixels at `ratio` device
    /// pixels per px.
    pub fn frame(&self, sheet: usize, x: f64, y: f64, width: f64, height: f64, ratio: f64) -> Result<SheetFrame> {
        let viewport = Viewport { x: x.max(0.0) as f32, y: y.max(0.0) as f32, width: width.max(1.0) as f32, height: height.max(1.0) as f32 };
        let list = self.workbook.display_list_for(SheetId(u32::try_from(sheet).context("sheet index")?), &viewport)?;
        Ok(SheetFrame { page: scene::translate_sheet(&list, ratio as f32)?, grid: list.grid })
    }
}

/// The sheet's last cell, zero-based: row 1,048,576 in column XFD (`lastCell`).
pub const LAST_CELL: [u32; 2] = [1_048_575, 16_383];

/// The cells an export anchor names: a cell or range anchor's (`anchorRange`).
fn anchor_range(anchor: &XlsxAnchor) -> Option<Range> {
    match anchor {
        XlsxAnchor::Cell { a1, .. } | XlsxAnchor::Range { a1, .. } => parse_range(a1),
        _ => None,
    }
}

/// An A1 cell or range, such as "B2" or "A1:F20" (`parseRange`).
fn parse_range(a1: &str) -> Option<Range> {
    let mut corners = a1.split(':');
    let first = parse_cell(corners.next()?)?;
    let last = match corners.next() {
        Some(c) => parse_cell(c)?,
        None => first,
    };
    Some([first[0].min(last[0]), first[1].min(last[1]), first[0].max(last[0]), first[1].max(last[1])])
}

/// "$B$2" or "B2" as row, column.
fn parse_cell(a1: &str) -> Option<[u32; 2]> {
    let a1 = a1.trim_start_matches('$');
    let split = a1.find(|c: char| !c.is_ascii_alphabetic())?;
    let (letters, digits) = a1.split_at(split);
    let col = column_index(letters)?;
    let row = digits.trim_start_matches('$').parse::<u32>().ok().filter(|r| *r >= 1)? - 1;
    (row <= LAST_CELL[0] && col <= LAST_CELL[1]).then_some([row, col])
}

/// A column's zero-based index from its one to three letters (`columnIndex`).
fn column_index(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 3 || !letters.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    Some(letters.bytes().fold(0u32, |n, b| n * 26 + u32::from(b.to_ascii_uppercase() - b'A' + 1)) - 1)
}

/// Hidden row spans such as "5:7", or column spans such as "C:D" (`parseSpans`).
fn spans(list: &[String], columns: bool) -> Vec<[u32; 2]> {
    list.iter()
        .filter_map(|span| {
            let (first, last) = span.split_once(':').unwrap_or((span, span));
            let index = |s: &str| if columns { column_index(s) } else { s.parse::<u32>().ok().filter(|n| *n >= 1).map(|n| n - 1) };
            let (start, end) = (index(first)?, index(last)?);
            (end >= start).then_some([start, end])
        })
        .collect()
}

/// Whether a cell's text pastes as it is: a number shown with a sign, such as "-12.50", which
/// quoting would paste as text (`literal`).
fn literal(cell: &betteroffice_xlsx::CellRead) -> bool {
    let text = cell.display_text.as_str();
    let Some(rest) = text.strip_prefix(['-', '+']) else { return false };
    if !matches!(cell.value, betteroffice_xlsx::CellValue::Number { .. }) || rest.is_empty() {
        return false;
    }
    let lower = rest.to_ascii_lowercase();
    // An exponent may close it: "-1.2e+05".
    let body = match lower.rsplit_once('e') {
        Some((b, e)) if e.len() > 1 && e.starts_with(['-', '+']) && e[1..].bytes().all(|c| c.is_ascii_digit()) => b,
        _ => lower.as_str(),
    };
    !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || ".,%".contains(c) || is_currency(c))
}

/// A currency symbol (`\p{Sc}`).
fn is_currency(c: char) -> bool {
    matches!(c, '$' | '\u{a2}'..='\u{a5}' | '\u{58f}' | '\u{60b}' | '\u{7fe}' | '\u{7ff}' | '\u{9f2}' | '\u{9f3}' | '\u{9fb}' | '\u{af1}' | '\u{bf9}' | '\u{e3f}' | '\u{17db}' | '\u{20a0}'..='\u{20c0}' | '\u{a838}' | '\u{fdfc}' | '\u{fe69}' | '\u{ff04}' | '\u{ffe0}' | '\u{ffe1}' | '\u{ffe5}' | '\u{ffe6}')
}
