//! A copy of cells as they show, not the formulas behind them (`spreadsheet.worker.ts` `copy`):
//! tab-separated text as `@betteroffice/xlsx`'s `toTsv` writes it, and the same cells as an
//! HTML table beside it for apps that paste tables (a spreadsheet, a document).

use super::CellText;

/// Rows joined by CRLF, cells by tabs; text a spreadsheet would read as a formula gets a leading
/// `'`, and a cell holding a tab, line break or quote is quoted (`toTsv`).
pub fn tsv(cells: &[Vec<CellText>]) -> String {
    cells.iter().map(|row| row.iter().map(tsv_cell).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\r\n")
}

fn tsv_cell(cell: &CellText) -> String {
    let text = &cell.text;
    let formula_like = text.starts_with(['=', '+', '-', '@', '\t', '\r', '\n']);
    let text = if !cell.literal && formula_like { format!("'{text}") } else { text.clone() };
    if text.contains(['\t', '\n', '\r', '"']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text }
}

/// The cells as an HTML table.
pub fn html(cells: &[Vec<CellText>]) -> String {
    let mut out = String::from("<meta charset=\"utf-8\"><table><tbody>");
    for row in cells {
        out.push_str("<tr>");
        for cell in row {
            out.push_str("<td>");
            for c in cell.text.chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\n' => out.push_str("<br>"),
                    '\r' => {}
                    c => out.push(c),
                }
            }
            out.push_str("</td>");
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    out
}
