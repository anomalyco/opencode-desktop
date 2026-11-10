//! Excel (`spreadsheet.worker.ts`): the workbook opens for reading, and each scroll position of a
//! sheet is a frame, a display list of the cells in that viewport with the grid that places them,
//! drawn as a scene.

mod scene;
mod text;

use anyhow::{Context, Result};
use betteroffice_xlsx::{CalculationOptions, CellRange, CellRef, SheetId, Viewport, Workbook};

use crate::shared::PageScene;

pub use betteroffice_xlsx::GridMeta;

pub struct Xlsx {
    workbook: Workbook,
    pub sheets: Vec<String>,
}

/// A sheet's extent and frozen panes, in CSS px.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SheetLayout {
    pub width: f64,
    pub height: f64,
    pub frozen_rows: u32,
    pub frozen_cols: u32,
    pub scroll_x: f64,
    pub scroll_y: f64,
    /// The last row and column, 0-based, of the active cell A1 grown over the merged cells it
    /// touches (`coverMerges`).
    pub active_end: [u32; 2],
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
        Ok(SheetLayout {
            width: f64::from(info.content_width),
            height: f64::from(info.content_height),
            frozen_rows: info.frozen_rows,
            frozen_cols: info.frozen_cols,
            scroll_x: f64::from(info.initial_scroll_x),
            scroll_y: f64::from(info.initial_scroll_y),
            active_end: self.active_end(sheet),
        })
    }

    /// A1 grown over every merged range it reaches, as Desktop's selection grows (`coverMerges`).
    fn active_end(&self, sheet: usize) -> [u32; 2] {
        let id = SheetId(sheet as u32);
        let mut end = CellRef::new(0, 0);
        loop {
            let merges = self.workbook.merged_ranges(id, CellRange::new(CellRef::new(0, 0), end)).unwrap_or_default();
            let grown = merges.iter().fold(end, |e, m| CellRef::new(e.row.max(m.end.row), e.col.max(m.end.col)));
            if (grown.row, grown.col) == (end.row, end.col) {
                return [end.row, end.col];
            }
            end = grown;
        }
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
