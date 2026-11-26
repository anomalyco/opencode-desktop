//! Where a terminal quote's comment editor stands: beside the rows the quote came from, as
//! official's does (`packages/gui-plugins/src/terminal/quote.tsx:112-122` hands the composer an
//! anchor; `quote-machine.ts` `anchorOf`, `selectQuoteSourcePlace` and `quotePosition` place it
//! from the selection's first and last cells, inside the terminal's bounds).

use rune::{El, el};
use terminal::TerminalSnapshot;

/// The mark the editor measures from: the top left of the first quoted row.
pub const ORIGIN_KEY: &str = "terminal-quote-origin";

/// The quoted rows' place, for the comment editor (`ComposerQuoteAnchor`).
#[derive(Clone, Debug, PartialEq)]
pub struct QuotePlace {
    /// The element the editor stays inside: the terminal's pane (`quotePosition`'s `bounds`).
    pub bounds: &'static str,
    /// The element `boxes` are measured from.
    pub origin: &'static str,
    /// One row: its selected cells' box. More rows: the first and the last row at the column
    /// the selection ends in, which the editor is centred on (`quotePosition`'s `center`).
    pub boxes: Vec<[f32; 4]>,
    /// The selection is one row: the editor goes above it, else below its end.
    pub single: bool,
}

/// Where the quoted rows stood when they were read (`QuoteSource`, `quote-machine.ts:46-52`):
/// the first row in the view, the rows scrolled above the view, the bytes written so far and
/// the grid's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QuoteSource {
    row: u16,
    offset: u64,
    written: u64,
    grid: (u16, u16),
}

impl crate::TerminalTab {
    /// The quote last taken with its comment editor, while its rows still hold. The screen is
    /// drawn again when it changes: the mark the editor measures from is on the screen in the
    /// frame the editor opens, not when the shell next draws.
    pub fn quote_source(&self) -> Option<QuoteSource> {
        self.quote_source.get()
    }
}

/// The selection's place in `snap`.
pub(super) fn place(snap: &TerminalSnapshot, cell_w: f32, cell_h: f32) -> Option<(QuoteSource, QuotePlace)> {
    let rows: Vec<(usize, u16, u16)> = snap.screen_rows.iter().enumerate()
        .filter_map(|(r, row)| {
            let mut selected = row.bg_spans.iter().filter(|s| s.selected);
            let first = selected.next()?;
            Some((r, first.start_col, selected.last().unwrap_or(first).end_col))
        })
        .collect();
    let (&(top, start, first_end), &(bottom, _, end)) = (rows.first()?, rows.last()?);
    let single = top == bottom;
    let end_x = f32::from(end) * cell_w;
    let boxes = if single {
        vec![[f32::from(start) * cell_w, 0.0, f32::from(first_end.saturating_sub(start)) * cell_w, cell_h]]
    } else {
        vec![[end_x, 0.0, 0.0, cell_h], [end_x, (bottom - top) as f32 * cell_h, 0.0, cell_h]]
    };
    let source = QuoteSource { row: top as u16, offset: snap.scrollbar.offset, written: snap.written, grid: (snap.cols, snap.rows) };
    Some((source, QuotePlace { bounds: super::VIEWPORT_KEY, origin: ORIGIN_KEY, boxes, single }))
}

/// The mark at the first quoted row's top left in the screen's layer, in or out of view. None
/// once output was written or the grid resized since the read: either can move or evict the
/// rows, and the editor then stands at the composer (`selectQuoteSourcePlace`,
/// `quote-machine.ts:258-281`). Scrolling moves the rows by whole rows, and the mark with them.
pub(super) fn origin(snap: &TerminalSnapshot, source: QuoteSource, cell_h: f32) -> Option<El> {
    if source.written != snap.written || source.grid != (snap.cols, snap.rows) {
        return None;
    }
    let row = i64::from(source.row) + source.offset as i64 - snap.scrollbar.offset as i64;
    Some(el().key(ORIGIN_KEY).absolute(0.0, row as f32 * cell_h).size(1.0, 1.0).inert())
}
