//! The document behind a CSV or TSV preview. Drawing and Find must read the same decoded
//! cells: a quote escaped as `""` is one visible character, not two hits in the file source.
//! Mirrors `gui-plugins/src/file/artifact.ts::parseDelimited`. The first retained row is the
//! header; empty lines are omitted, but a row of empty fields separated by a delimiter is real.

use std::ops::Range;

const ROW_LIMIT: usize = 1_000; // Includes the header, as in official.
// Rows virtualize vertically. Until columns virtualize too, leave very wide sheets in the
// source viewer instead of building thousands of off-screen cells for a single visible row.
const COLUMN_LIMIT: usize = 128;

/// The scroll key is scoped to both the session and the open file, never the basename alone.
pub fn scroll_key(sid: &str, path: &str) -> String {
    format!("file-table-{sid}-{path}-scroll")
}

pub fn row_key(row: usize) -> String {
    format!("tr-{row}")
}

pub fn cell_key(row: usize, column: usize) -> String {
    format!("tc-{row}-{column}")
}

/// Sparse rows in document order. Empty trailing cells in shorter rows need no storage.
/// All offsets refer to the decoded UTF-8 buffer, never to the original CSV markup.
pub struct Table {
    text: String,
    cells: Vec<Range<usize>>,
    rows: Vec<Range<usize>>,
    /// Intrinsic column widths in character units, capped at the upstream 40ch. Calculated
    /// when the read lands, not when scrolling or when a search result changes.
    widths: Vec<u8>,
    /// One SEARCH slab row per displayed cell, including embedded newlines in that cell.
    slab: opencode_search::SlabFind,
    pub columns: usize,
    pub total_rows: usize,
}

impl std::fmt::Debug for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Table").field("rows", &self.total_rows).field("columns", &self.columns).finish()
    }
}

impl Table {
    pub fn parse(path: &str, src: &str) -> Option<Self> {
        let ext = std::path::Path::new(path).extension()?.to_str()?;
        let delimiter = if ext.eq_ignore_ascii_case("csv") {
            ','
        } else if ext.eq_ignore_ascii_case("tsv") {
            '\t'
        } else {
            return None;
        };
        if src.len() > super::RENDER_LIMIT {
            return None;
        }
        let mut table = Self {
            text: String::new(),
            cells: Vec::new(),
            rows: Vec::new(),
            widths: Vec::new(),
            slab: Default::default(),
            columns: 0,
            total_rows: 0,
        };
        let mut chars = src.chars().peekable();
        let mut quoted = false;
        let mut field_has_text = false;
        let mut separated = false;
        let mut cell_start = 0;
        let mut row_start = 0;
        while let Some(ch) = chars.next() {
            if quoted {
                if ch == '"' && chars.next_if_eq(&'"').is_none() {
                    quoted = false;
                    continue;
                }
            } else if ch == '"' && !field_has_text {
                quoted = true;
                continue;
            } else if ch == delimiter {
                if table.total_rows < ROW_LIMIT {
                    table.cells.push(cell_start..table.text.len());
                    if table.cells.len() - row_start >= COLUMN_LIMIT {
                        return None;
                    }
                    cell_start = table.text.len();
                }
                separated = true;
                field_has_text = false;
                continue;
            } else if ch == '\r' {
                continue;
            } else if ch == '\n' {
                if separated || field_has_text {
                    table.end_row(row_start, cell_start);
                }
                cell_start = table.text.len();
                row_start = table.cells.len();
                separated = false;
                field_has_text = false;
                continue;
            }
            // Past the display limit keep just the parser state and count, not the cell text.
            if table.total_rows < ROW_LIMIT {
                table.text.push(ch);
            }
            field_has_text = true;
        }
        if separated || field_has_text {
            table.end_row(row_start, cell_start);
        }
        let mut widths = vec![1; table.columns];
        let mut slab = opencode_search::SlabFind::default();
        for ((_, col), value) in table.labels() {
            slab.push(value);
            widths[col] = widths[col].max(value.lines().map(|line| line.chars().take(40).count() as u8).max().unwrap_or(0));
        }
        table.widths = widths;
        table.slab = slab;
        Some(table)
    }

    fn end_row(&mut self, row_start: usize, cell_start: usize) {
        if self.total_rows < ROW_LIMIT {
            self.cells.push(cell_start..self.text.len());
            let end = self.cells.len();
            self.columns = self.columns.max(end - row_start);
            self.rows.push(row_start..end);
        }
        self.total_rows += 1;
    }

    pub fn shown(&self) -> usize {
        self.rows.len()
    }

    pub fn widths(&self) -> &[u8] {
        &self.widths
    }

    pub fn matches(&self, query: &str) -> Vec<opencode_search::SlabMatch> {
        let mut hits = Vec::new();
        self.slab.find(query, |hit| hits.push(hit));
        hits
    }

    pub fn coordinates(&self, cell: usize) -> Option<(usize, usize)> {
        let r = self.rows.partition_point(|range| range.end <= cell);
        Some((r, cell.checked_sub(self.rows.get(r)?.start)?))
    }

    pub fn cell_id(&self, row: usize, column: usize) -> Option<usize> {
        let cells = self.rows.get(row)?;
        let at = cells.start.checked_add(column)?;
        (at < cells.end).then_some(at)
    }

    /// Drawing, tooltips and copying use the full cell value rather than the ellipsized text.
    pub fn cell(&self, row: usize, column: usize) -> Option<&str> {
        let range = self.rows.get(row)?;
        let at = range.start.checked_add(column)?;
        if at >= range.end {
            return None;
        }
        Some(&self.text[self.cells[at].clone()])
    }

    /// SEARCH's unit is a visible cell. Matches cannot cross from one cell to the next, even
    /// though the backing text happens to be contiguous. Coordinates survive scrolling.
    pub fn labels(&self) -> impl Iterator<Item = ((usize, usize), &str)> {
        self.rows.iter().enumerate().flat_map(move |(row, cells)| {
            cells.clone().enumerate().map(move |(column, at)| ((row, column), &self.text[self.cells[at].clone()]))
        })
    }
}
