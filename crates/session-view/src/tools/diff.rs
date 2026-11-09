//! A file's change as Desktop's timeline draws it with Pierre (`session-ui/src/pierre/index.ts`):
//! one column, 13 px mono on 24 px rows over `background-stronger`; a changed line tinted green
//! or red with a 2 pt bar at its left edge and its number in the same colour, the words that
//! changed in it on a stronger tint; no `+`/`-` column; a quiet 24 px row between hunks
//! (`hunkSeparators: "simple"`, as a tool's partial patch has). Each hunk's sides are coloured
//! as whole chunks, so a comment or string that spans lines keeps its colour.

use std::ops::Range;
use std::rc::Rc;

use rune::{El, Span, col, el, row, text};
use rune_kit::diff::{FileDiff, Hunk, LineKind};
use rune_kit::syntax::{Highlight, SyntaxTheme};

use crate::palette::Pal;
use crate::style::{CODE, INSTANT, readable};

/// Pierre's row height (`--diffs-line-height`).
const ROW: f32 = 24.0;
/// Pierre's tab size (`--diffs-tab-size`).
const TAB: &str = "  ";

/// `fd`'s hunks, each line on its own row.
pub fn view(p: &Pal, fd: &FileDiff) -> El {
    let theme = syntax(p);
    let lang = language(fd);
    let font = CODE.lh(ROW);
    let ch = rune::text::measure("0", &[], font, None).0;
    let most = fd.hunks.iter().map(|h| (h.old_start + h.old_count).max(h.new_start + h.new_count)).max().unwrap_or(1);
    // Numbers sit 2ch in (room for the bar), right-aligned in at least 3ch; code starts 2ch on.
    let numbers = ch * (2.0 + most.to_string().len().max(3) as f32);
    let mut out = col().key("diff").w_full().fill(p.code_bg);
    for (hi, h) in fd.hunks.iter().enumerate() {
        if hi > 0 {
            out = out.child(el().key(format!("sep-{hi}")).w_full().h(ROW).fill(p.diff.separator));
        }
        let paint = HunkPaint::new(h, lang.as_deref());
        for (li, l) in h.lines.iter().enumerate() {
            let (fill, ink, bar, number, side) = match l.kind {
                LineKind::Added => (Some(p.diff.add_line), p.diff.add_bar, Some(p.diff.add_bar), l.new, &paint.new),
                LineKind::Removed => (Some(p.diff.del_line), p.diff.del_bar, Some(p.diff.del_bar), l.old, &paint.old),
                _ => (None, p.faint, None, l.new, &paint.new),
            };
            let at = paint.at[li];
            let mut spans = theme.spans(&side.within(at..at + l.text.len()), 0);
            if let Some(word) = match l.kind {
                LineKind::Added => Some((p.diff.add_word, p.diff.add_word_text)),
                LineKind::Removed => Some((p.diff.del_word, p.diff.del_word_text)),
                _ => None,
            } {
                spans.extend(l.changes.iter().map(|r| Span::new(r.start, r.end).bg(word.0, 0.0).color(word.1)));
            }
            let (shown, spans) = expand_tabs(&l.text, spans);
            let mut line = row()
                .key(format!("l-{hi}-{li}"))
                .w_full()
                .min_h(ROW)
                .items_start()
                .child(
                    row().key("n").w(numbers).h(ROW).justify_end().center_y()
                        .child(text(number.map(|n| n.to_string()).unwrap_or_default()).font(font).color(ink)),
                )
                // Changed words round at Pierre's 3 px (`[data-diff-span] { border-radius: 3px }`).
                .child(readable(p, text(shown).font(font).color(p.syntax.parameter).spans(spans).span_radius(3.0).wrap_anywhere()).grow(1.0).min_w(0.0).pl(2.0 * ch).pr(ch));
            // Pierre lights an unchanged line under the pointer (`lineHoverHighlight: "both"`); a
            // changed one it mixes toward its own fill, which leaves it as it is.
            line = match fill {
                Some(f) => line.fill(f),
                None => line.hover(p.diff.hover).hover_motion(INSTANT, INSTANT),
            };
            if let Some(b) = bar {
                line = line.child(el().key("bar").absolute(0.0, 0.0).w(2.0).h_full().fill(b));
            }
            out = out.child(line);
        }
    }
    out
}

/// The theme a file's code colours by: Desktop's `syntax-*` tokens in every theme, as Pierre's
/// `OpenCodeTheme` paints them.
fn syntax(p: &Pal) -> SyntaxTheme {
    p.syntax.syntax(SyntaxTheme::from_kit(crate::palette::kit(p)), p.diff_add, p.diff_del)
}


/// The highlighter's name for `fd`'s language, from its file name as Desktop's diffs pick it.
fn language(fd: &FileDiff) -> Option<String> {
    fd.language().map(str::to_owned)
}

/// One side of a hunk coloured as a whole: its highlights over the side's lines joined by `\n`.
struct Side(Rc<[Highlight]>);

impl Side {
    /// The highlights inside `range` of the joined side, moved to start at 0.
    fn within(&self, range: Range<usize>) -> Vec<Highlight> {
        self.0
            .iter()
            .filter(|(r, _)| r.end > range.start && r.start < range.end)
            .map(|(r, paint)| (r.start.max(range.start) - range.start..r.end.min(range.end) - range.start, *paint))
            .collect()
    }
}

/// A hunk's two sides, each coloured once, and where each line starts in its side.
struct HunkPaint {
    old: Side,
    new: Side,
    at: Vec<usize>,
}

impl HunkPaint {
    fn new(h: &Hunk, lang: Option<&str>) -> Self {
        let (mut old, mut new) = (String::new(), String::new());
        let mut at = Vec::with_capacity(h.lines.len());
        for l in &h.lines {
            let side = if l.kind == LineKind::Removed { &mut old } else { &mut new };
            at.push(side.len());
            side.push_str(&l.text);
            side.push('\n');
            if l.kind == LineKind::Context {
                old.push_str(&l.text);
                old.push('\n');
            }
        }
        let paint = |code: &str| Side(rune_kit::syntax::highlight(lang, code));
        HunkPaint { old: paint(&old), new: paint(&new), at }
    }
}

/// `line` with each tab two spaces wide, and `spans` moved to match.
fn expand_tabs(line: &str, spans: Vec<Span>) -> (String, Vec<Span>) {
    if !line.contains('\t') {
        return (line.to_owned(), spans);
    }
    let tabs: Vec<usize> = line.match_indices('\t').map(|(i, _)| i).collect();
    let shift = |at: u32| at + (TAB.len() as u32 - 1) * tabs.iter().filter(|&&t| (t as u32) < at).count() as u32;
    let spans = spans.into_iter().map(|s| Span { start: shift(s.start), end: shift(s.end), ..s }).collect();
    (line.replace('\t', TAB), spans)
}

/// `content`'s lines numbered, coloured as `lang`, as Pierre's file view shows a text in a dialog:
/// numbers 12 pt in, right-aligned in `digits` characters, with 4 pt above and below.
pub fn code(p: &Pal, content: &str, lang: Option<&str>, digits: usize) -> El {
    let theme = syntax(p);
    let font = CODE.lh(ROW);
    let ch = rune::text::measure("0", &[], font, None).0;
    let numbers = 12.0 + ch * digits.max(1) as f32;
    let paint = Side(rune_kit::syntax::highlight(lang, content));
    let mut out = col().key("code").w_full().py(4.0).fill(p.code_bg);
    let mut at = 0;
    for (n, l) in content.split('\n').enumerate() {
        let spans = theme.spans(&paint.within(at..at + l.len()), 0);
        at += l.len() + 1;
        let (shown, spans) = expand_tabs(l, spans);
        out = out.child(
            row().key(format!("l-{n}")).w_full().min_h(ROW).items_start()
                .hover(p.diff.hover)
                .hover_motion(INSTANT, INSTANT)
                .child(row().key("n").w(numbers).h(ROW).justify_end().center_y().child(text((n + 1).to_string()).font(font).color(p.faint)))
                .child(readable(p, text(shown).font(font).color(p.syntax.parameter).spans(spans).wrap_anywhere()).grow(1.0).min_w(0.0).pl(2.0 * ch).pr(ch)),
        );
    }
    out
}
