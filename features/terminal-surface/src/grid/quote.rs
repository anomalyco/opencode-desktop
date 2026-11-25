//! Where a terminal quote's comment editor stands: beside the rows the quote came from, as
//! official's does (`packages/gui-plugins/src/terminal/quote.tsx` hands the composer an anchor;
//! `quote-machine.ts` `anchorOf` and `quotePosition` place it from the selection's first and
//! last cells, inside the terminal's bounds).

use rune::{El, el};
use terminal::TerminalSnapshot;

/// The mark the editor measures from: the top left of the first quoted row, while that row is
/// on the screen. Official keeps the selection's cells and the scrollback offset
/// (`selectQuoteSourcePlace`); a row here has an id, so the mark follows its row.
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

/// The selection's place in `snap`, and its first row's id.
pub(super) fn place(snap: &TerminalSnapshot, cell_w: f32, cell_h: f32) -> Option<(u128, QuotePlace)> {
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
    Some((snap.screen_rows[top].id, QuotePlace { bounds: super::VIEWPORT_KEY, origin: ORIGIN_KEY, boxes, single }))
}

/// The mark at row `id`'s top left in the screen's layer, while the row is on the screen.
pub(super) fn origin(snap: &TerminalSnapshot, id: u128, cell_h: f32) -> Option<El> {
    let row = snap.screen_rows.iter().position(|r| r.id == id)?;
    Some(el().key(ORIGIN_KEY).absolute(0.0, row as f32 * cell_h).size(1.0, 1.0).inert())
}
