//! The file panel's decoded table, `ArtifactTable` in `gui-plugins/src/file/artifact-view.tsx`.
//! Cell content is plain text. The copy button copies that same value, including embedded
//! quotes and newlines; neither drawing nor copying interprets formulas or markup.

use std::borrow::Cow;
use std::sync::Arc;

use rune::{El, GrowthSpec, ScrollSpec, TextHighlight, Tx, col, el, icon, row, text};
use rune_kit::ScrollBar;
use rune_theme_opencode::icons as ic;
use session_view::palette::Pal;
use session_view::style::{UI, UI_MED, UI_SMALL};

use super::delimited::{Table, cell_key, row_key, scroll_key};

const ROW_H: f32 = 28.0;

fn column_width(chars: u8) -> f32 {
    48.0 + 7.5 * f32::from(chars)
}

/// Search supplies marks in decoded cell bytes for this immutable table. Only mounted cells
/// ask for paint. `marks` receives the document identity so results from a previous read can
/// be discarded, even when a new document has the same number of rows and columns.
pub(crate) fn view<H: 'static>(
    sid: &str,
    path: &str,
    p: &Pal,
    doc: Arc<Table>,
    marks: impl Fn(&Arc<Table>, (usize, usize)) -> Vec<TextHighlight> + 'static,
) -> El {
    let key = scroll_key(sid, path);
    let width: f32 = doc.widths().iter().map(|c| column_width(*c)).sum();
    let pal = *p;
    let truncated = doc.total_rows > doc.shown();
    let rows = col()
        .key("rows")
        .role("table")
        .label(path.to_owned())
        .w_full()
        .min_w(width)
        .growth(GrowthSpec::default())
        .virtual_rows(
            doc.shown() + usize::from(truncated),
            ROW_H,
            |_: &H, n| Cow::Owned(row_key(n)),
            move |_: &H, n| {
                if n == doc.shown() {
                    return row().role("note").h(ROW_H).px(16.0).center_y().child(
                        text(opencode_i18n::t!(
                            "plugin.file.view.table.truncated",
                            shown = doc.shown().saturating_sub(1),
                            total = doc.total_rows.saturating_sub(1)
                        ))
                        .font(UI_SMALL)
                        .color(pal.muted),
                    );
                }
                doc.widths()
                    .iter()
                    .enumerate()
                    .fold(row().role("row").h(ROW_H).w_full(), |line, (i, chars)| {
                        let value = doc.cell(n, i).unwrap_or("");
                        let src = Arc::clone(&doc);
                        let copy = el()
                            .key("copy")
                            .role("button")
                            .label_static(opencode_i18n::t!("plugin.file.view.table.copyCell"))
                            .help(opencode_i18n::t!("plugin.file.view.table.copyCell"))
                            .size(24.0, 24.0)
                            .radius(4.0)
                            .center()
                            .hover(pal.hover)
                            .group_opacity(0.0, 1.0)
                            .on_press_tx(move |_: &mut H, tx: &mut Tx| {
                                tx.copy(src.cell(n, i).unwrap_or("").to_owned())
                            })
                            .child(icon(&ic::OUTLINE_COPY, 14.0).color(pal.icon_muted));
                        let words = text(value.to_owned())
                            .key("value")
                            .font(if n == 0 { UI_MED } else { UI })
                            .color(pal.text)
                            .grow(1.0)
                            .min_w(0.0)
                            .shrink()
                            .selectable(true)
                            .truncate()
                            .text_highlights(marks(&doc, (n, i)));
                        let cell = row()
                            .key(cell_key(n, i))
                            .role(if n == 0 { "columnheader" } else { "cell" })
                            .w(column_width(*chars))
                            .grow(1.0)
                            .h(ROW_H)
                            .pl(12.0)
                            .pr(6.0)
                            .gap(6.0)
                            .center_y()
                            .hover_group_focus()
                            .child(words)
                            .child(copy)
                            .child(
                                el().pin(None, None, Some(0.0), Some(0.0))
                                    .w(0.5)
                                    .h_full()
                                    .fill(pal.border_muted),
                            )
                            .child(
                                el().pin(Some(0.0), None, None, Some(0.0))
                                    .h(0.5)
                                    .w_full()
                                    .fill(pal.border_muted),
                            );
                        line.child(cell)
                    })
            },
        );
    col()
        .key(key)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(p.bg_base)
        .scroll(ScrollSpec { stick_bottom: false })
        .scroll_xy()
        .child(rows)
        .scroll_bar_styled(session_view::scroller::native_bar(p))
}
