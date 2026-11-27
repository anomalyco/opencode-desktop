//! A table (`widgets.md`, Table): drawn as chat draws a Markdown table, so a widget's table
//! reads like the tables in the replies around it.
//!
//! Chat's tables are rune-kit's Markdown tables in the look `opencode_ui::style::markdown_style`
//! gives them (`crates/opencode-ui/src/style.rs`: `MarkdownStyle::table_card`, then a muted header
//! at weight 530 and `scrollbar.css`'s thumb under a wide table). rune-kit keeps that renderer to
//! Markdown's own blocks, with no rows that press or headers that sort, so this draws the same
//! table from the same Kit tokens: `table_card`'s frame, body card, rules, fonts and padding
//! (`rune-kit/src/markdown/table.rs`, `MarkdownStyle::table_card` and `Renderer::table`'s framed
//! path), columns laid out as its `auto_layout` does. It differs where a widget's table says
//! more than Markdown: an `end` column sits at the end (chat's tables set every cell at the start,
//! `table_column_align(false)`), the sorted column's header carries ▲ or ▼, the selected row
//! takes the accent's tint, and with handlers rows and headers press.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use opencode_widget::{Cell, Column, ColumnAlign, Node, Sort, WidgetEvent};
use rune::{Align, El, Shadow, TextStyle, Tx, col, el, row, text};
use rune_kit::{Kit, ScrollBar, ScrollBarStyle};

use crate::Cx;

/// `table_card`'s frame: a `layer_01` fill rounded 6 with its rows inset 2 inside a half-point ring.
const RADIUS: f32 = 6.0;
const INSET: f32 = 2.0;
/// The body rows' card, rounded 4.
const BODY_RADIUS: f32 = 4.0;
/// Header cells padded 12 by 8, body cells 12 by 10.
const HEAD_PAD: [f32; 2] = [12.0, 8.0];
const CELL_PAD: [f32; 2] = [12.0, 10.0];
/// Chat's header weight (`markdown_style`'s `table_header_weight(530.0)`).
const HEAD_WEIGHT: f32 = 530.0;
/// The web host's selected row: the accent at 14% (`packages/widgets-rs/web/src/styles.ts:118`).
const SELECTED: f32 = 0.14;

/// The sort indicator after the sorted column's label, as the web host writes it
/// (`packages/widgets-rs/web/src/render.ts`, `table`).
fn indicator(descending: bool) -> &'static str {
    if descending { " ▼" } else { " ▲" }
}

/// `table_card`'s cell font: the kit's compact UI type at weight 440, tracked -0.04, with
/// tabular figures so numbers line up down a column.
fn body_font(k: &Kit) -> TextStyle {
    let f = k.ty.ui_compact;
    TextStyle { weight: 440.0, tracking: -0.04, features: tnum(f.features), ..f }
}

/// `features` with tabular figures on (as rune-kit's Markdown tables set them).
fn tnum(features: &'static str) -> &'static str {
    if features.is_empty() {
        return "'tnum' 1";
    }
    if features.contains("tnum") {
        return features;
    }
    thread_local! {
        static WITH: RefCell<Vec<(&'static str, &'static str)>> = const { RefCell::new(Vec::new()) };
    }
    WITH.with_borrow_mut(|known| {
        if let Some(&(_, out)) = known.iter().find(|(f, _)| *f == features) {
            return out;
        }
        let out: &'static str = Box::leak(format!("{features}, 'tnum' 1").into_boxed_str());
        known.push((features, out));
        out
    })
}

/// Per column, padded: its content's width on one line, and its longest word.
struct Widths {
    natural: Vec<f32>,
    fit: Vec<f32>,
}

thread_local! {
    /// Column widths by table content and fonts: a view rebuilds on every change.
    static WIDTHS: RefCell<HashMap<u64, Rc<Widths>>> = RefCell::new(HashMap::new());
}

/// The widest of `texts` in `font`, on one line and by its longest word.
fn content(texts: impl Iterator<Item = impl AsRef<str>>, font: TextStyle) -> (f32, f32) {
    texts.fold((0.0f32, 0.0f32), |(natural, fit), t| {
        let t = t.as_ref();
        if t.is_empty() {
            return (natural, fit);
        }
        let (min, max) = rune::text::content_widths(t, &[], font);
        // A hair over, so a cell at its column's natural width never wraps its last word.
        (natural.max(max + 0.01), fit.max(min + 0.01))
    })
}

fn widths(columns: &[Column], rows: &[Vec<Cell>], marked: impl Fn(usize) -> Option<&'static str>, head: TextStyle, body: TextStyle) -> Rc<Widths> {
    let mut h = DefaultHasher::new();
    (rune::text::font_epoch(), head.size.to_bits(), head.weight.to_bits(), body.size.to_bits(), body.weight.to_bits(), body.tracking.to_bits(), body.features).hash(&mut h);
    for (j, c) in columns.iter().enumerate() {
        (&c.label, marked(j)).hash(&mut h);
    }
    for r in rows {
        r.len().hash(&mut h);
        for c in r {
            c.text().hash(&mut h);
        }
    }
    let key = h.finish();
    if let Some(hit) = WIDTHS.with_borrow(|w| w.get(&key).cloned()) {
        return hit;
    }
    let (mut natural, mut fit) = (Vec::with_capacity(columns.len()), Vec::with_capacity(columns.len()));
    for (j, c) in columns.iter().enumerate() {
        let label = format!("{}{}", c.label, marked(j).unwrap_or(""));
        let (hn, hf) = content(std::iter::once(label), head);
        let (bn, bf) = content(rows.iter().filter_map(|r| r.get(j)).map(Cell::text), body);
        let [hx, bx] = [2.0 * HEAD_PAD[0], 2.0 * CELL_PAD[0]];
        let some = !rows.is_empty();
        natural.push(if some { (hn + hx).max(bn + bx) } else { hn + hx });
        fit.push(if some { (hf + hx).max(bf + bx) } else { hf + hx });
    }
    let out = Rc::new(Widths { natural, fit });
    WIDTHS.with_borrow_mut(|w| {
        if w.len() >= 64 {
            w.clear();
        }
        w.insert(key, out.clone());
    });
    out
}

/// A browser's automatic layout of a `width: 100%` table in `avail` points, as rune-kit's
/// Markdown tables take it (`auto_layout`, proportional shrinking): columns in proportion to
/// their content when it all fits, between their longest word and their content when only the
/// words fit, and at their longest word otherwise (the table then scrolls).
fn layout(w: &Widths, avail: f32) -> Vec<f32> {
    let (widest, narrowest): (f32, f32) = (w.natural.iter().sum(), w.fit.iter().sum());
    if widest <= avail {
        let k = if widest > 0.0 { avail / widest } else { 1.0 };
        return w.natural.iter().map(|n| n * k).collect();
    }
    if narrowest >= avail || widest <= narrowest {
        return w.fit.clone();
    }
    let k = (avail - narrowest) / (widest - narrowest);
    w.natural.iter().zip(&w.fit).map(|(n, f)| f + (n - f).max(0.0) * k).collect()
}

/// A table node (`Node::Table`); anything else draws as nothing.
pub(crate) fn view<S: 'static>(cx: &Cx<S>, n: &Node) -> El {
    let Node::Table { id, columns, rows, sort, selected, selectable, sortable } = n else { return el() };
    let k = cx.look.kit;
    let c = &k.c;
    let hooks = cx.act().zip(id.clone());
    let (press, sorts) = (hooks.clone().filter(|_| *selectable), hooks.filter(|_| *sortable));
    let sort: Option<Sort> = *sort;
    let sorted = |j: usize| sort.filter(|s| usize::try_from(s.column).ok() == Some(j)).map(|s| indicator(s.descending));
    let body = body_font(k);
    let head = TextStyle { weight: HEAD_WEIGHT, ..body };
    // A sortable table keeps room for the indicator on every header, so sorting moves no column.
    let room = |j: usize| sorted(j).or(sorts.as_ref().map(|_| indicator(true)));
    let w = widths(columns, rows, room, head, body);
    // The text's width a Markdown node in the widget gets (`session-view`'s widget frame).
    let text_w = cx.look.width - if cx.look.fullscreen { 0.0 } else { 2.0 * crate::PAD };
    let avail = (text_w - 2.0 * INSET).max(0.0);
    let widths = layout(&w, avail);
    let total: f32 = widths.iter().sum();
    let over = total > avail + 0.5;

    let cell = |j: usize, s: String, font: TextStyle, color, [x, y]: [f32; 2], role: &'static str| {
        let align = match columns[j].align {
            ColumnAlign::End => Align::End,
            ColumnAlign::Start => Align::Start,
        };
        let t = text(s).font(font).color(color).align(align).wrap_anywhere().w_full();
        // Clipped: a word wider than a shrunk column must not run into the next.
        col().key(format!("c{j}")).role(role).w(widths[j]).px(x).py(y).clip_soft(0.0).child(t)
    };

    let mut header = row().key("th").role("row").w_full();
    for (j, column) in columns.iter().enumerate() {
        let label = format!("{}{}", column.label, sorted(j).unwrap_or(""));
        let mut th = cell(j, label, head, c.text_muted, HEAD_PAD, "columnheader");
        if let Some(s) = sort.filter(|s| usize::try_from(s.column).ok() == Some(j)) {
            th = th.sorted(Some(!s.descending));
        }
        if let Some((hooks, id)) = sorts.clone() {
            // The web host's pressable header: `.pw-table .pw-press { cursor: pointer }`
            // (`packages/widgets-rs/web/src/styles.ts:116-117`).
            th = k.interactive(th.cursor(rune::Cursor::Pointer)).on_press_tx(move |s: &mut S, tx: &mut Tx| hooks.send(s, WidgetEvent::change(id.clone(), j), tx));
        }
        header = header.child(th);
    }
    // The header's overflow stops at the frame's rounded top corners.
    let r = RADIUS - INSET;
    let mut frame = col().w(total + 2.0 * INSET).padding(INSET).child(header.clip_corners([r, r, 0.0, 0.0]));

    if !rows.is_empty() {
        let mut body_rows = col().key("tbc").w_full().clip_rounded(BODY_RADIUS);
        for (i, cells) in rows.iter().enumerate() {
            let mut line = row().key(format!("r{i}")).role("row").w_full();
            for j in 0..columns.len() {
                let s = cells.get(j).map(|c| c.text().into_owned()).unwrap_or_default();
                line = line.child(cell(j, s, body, c.text_base, CELL_PAD, "cell"));
            }
            // `tbody tr:not(:last-child) td { border-bottom: 0.5px }`, as chat's tables rule rows.
            if i + 1 < rows.len() {
                line = line.border_sides([0.0, 0.0, 0.5, 0.0], c.border_muted).border_layout();
            }
            let on = usize::try_from(selected.unwrap_or(-1)).ok() == Some(i);
            if on {
                line = line.fill(c.bg_accent.alpha(SELECTED));
            }
            if let Some((hooks, id)) = press.clone() {
                // The web host's pressable row (`styles.ts:116-117`).
                line = k.interactive(line.cursor(rune::Cursor::Pointer)).on_press_tx(move |s: &mut S, tx: &mut Tx| hooks.send(s, WidgetEvent::press_row(id.clone(), i), tx));
            }
            body_rows = body_rows.child(line);
        }
        let card = col().key("tb").w_full().radius(BODY_RADIUS).fill(c.bg_base);
        frame = frame.child(k.shadow_layers(&k.elevation.raised).fold(card, |card, sh| card.box_shadow(sh)).child(body_rows));
    }

    // The rows scroll inside a frame that stays put, as a web page's `overflow-x: auto` wrapper
    // keeps its fill and ring while the table in it scrolls.
    let ring = Shadow { spread: 0.5, color: c.border_muted, inset: true, ..Shadow::default() };
    let decor = col().role("table").w_full().radius(RADIUS).fill(c.layer_01).box_shadow(ring);
    let viewport = col().key("scroll").w_full().scroll_x().clip_rounded(RADIUS).child(frame);
    let e = if over {
        let bar = ScrollBarStyle::native(c.border_muted, c.border_strong).horizontal();
        decor.child(viewport.scroll_bar_styled(bar).pb(bar.strip()))
    } else {
        decor.child(viewport)
    };
    // In a stack, a table that takes no input lets the pointer through to the layer below.
    if cx.layered && press.is_none() && sorts.is_none() { e.inert() } else { e }
}
