//! A file's change as Desktop's timeline draws it with Pierre (`session-ui/src/pierre/index.ts`):
//! one column, 13 px mono on 24 px rows over `background-stronger`; a changed line tinted green
//! or red with a 2 pt bar at its left edge and its number in the same colour, the words that
//! changed in it on a stronger tint; no `+`/`-` column; a quiet 24 px row between hunks
//! (`hunkSeparators: "simple"`, as a tool's partial patch has). Each hunk's sides are coloured
//! as whole chunks, so a comment or string that spans lines keeps its colour.

use std::ops::Range;
use std::rc::Rc;

use rune::{El, Span, col, el, row, text};
use rune_kit::diff::{FileDiff, Hunk, Line, LineKind};
use rune_kit::syntax::{Highlight, SyntaxTheme};

use crate::palette::Pal;
use crate::style::{CODE, INSTANT, readable};

/// Pierre's row height (`--diffs-line-height`).
const ROW: f32 = 24.0;
/// Pierre's tab size (`--diffs-tab-size`).
const TAB: &str = "  ";

/// Above this many rows a change draws its lines as virtual rows: a patch of thousands of lines
/// builds, highlights and lays out the ones in view, not all of them.
const VIRTUAL: usize = 120;

/// What every line of a change draws with.
#[derive(Clone)]
struct Look {
    p: Pal,
    theme: SyntaxTheme,
    font: rune::TextStyle,
    ch: f32,
    numbers: f32,
}

impl Look {
    fn new(p: &Pal, fd: &FileDiff) -> Look {
        let font = CODE.lh(ROW);
        let ch = rune::text::measure("0", &[], font, None).0;
        let most = fd.hunks.iter().map(|h| (h.old_start + h.old_count).max(h.new_start + h.new_count)).max().unwrap_or(1);
        // Numbers sit 2ch in (room for the bar), right-aligned in at least 3ch; code starts 2ch on.
        let numbers = ch * (2.0 + most.to_string().len().max(3) as f32);
        Look { p: *p, theme: syntax(p), font, ch, numbers }
    }

    /// Line `li` of hunk `hi`, `highlights` its syntax colours from its line's start.
    fn line(&self, l: &Line, hi: usize, li: usize, highlights: &[Highlight]) -> El {
        let p = &self.p;
        let (fill, ink, bar, number) = match l.kind {
            LineKind::Added => (Some(p.diff.add_line), p.diff.add_bar, Some(p.diff.add_bar), l.new),
            LineKind::Removed => (Some(p.diff.del_line), p.diff.del_bar, Some(p.diff.del_bar), l.old),
            _ => (None, p.faint, None, l.new),
        };
        let mut spans = self.theme.spans(highlights, 0);
        if let Some(word) = match l.kind {
            LineKind::Added => Some((p.diff.add_word, p.diff.add_word_text)),
            LineKind::Removed => Some((p.diff.del_word, p.diff.del_word_text)),
            _ => None,
        } {
            spans.extend(l.changes.iter().map(|r| Span::new(r.start, r.end).bg(word.0, 0.0).color(word.1)));
        }
        let (shown, spans) = expand_tabs(&l.text, spans);
        let (font, ch) = (self.font, self.ch);
        let mut line = row()
            .key(format!("l-{hi}-{li}"))
            .w_full()
            .min_h(ROW)
            .items_start()
            .child(
                row().key("n").w(self.numbers).h(ROW).justify_end().center_y()
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
        line
    }

    fn separator(&self, hi: usize) -> El {
        el().key(format!("sep-{hi}")).w_full().h(ROW).fill(self.p.diff.separator)
    }
}

/// How tall `fd` lays out with no line wrapped: a row per line, and one between hunks.
pub(super) fn height(fd: &FileDiff) -> f32 {
    (fd.hunks.iter().map(|h| h.lines.len()).sum::<usize>() + fd.hunks.len().saturating_sub(1)) as f32 * ROW
}

/// `fd`'s hunks, each line on its own row.
pub fn view<S: 'static>(p: &Pal, fd: FileDiff) -> El {
    let look = Look::new(p, &fd);
    let lang = language(&fd);
    // Code reads left to right in any window: numbers and bars at the left.
    let out = col().key("diff").dir(rune::Direction::Ltr).w_full().fill(p.code_bg);
    let rows = fd.hunks.iter().map(|h| h.lines.len()).sum::<usize>() + fd.hunks.len().saturating_sub(1);
    if rows > VIRTUAL {
        return virtual_view::<S>(look, lang, fd, out);
    }
    let mut out = out;
    for (hi, h) in fd.hunks.iter().enumerate() {
        if hi > 0 {
            out = out.child(look.separator(hi));
        }
        let paint = HunkPaint::new(h, lang.as_deref());
        for (li, l) in h.lines.iter().enumerate() {
            let side = if l.kind == LineKind::Removed { &paint.old } else { &paint.new };
            let at = paint.at[li];
            out = out.child(look.line(l, hi, li, &side.within(at..at + l.text.len())));
        }
    }
    out
}

/// One row of a long change: a hunk's line, or the quiet row between two hunks.
#[derive(Clone, Copy)]
enum Item {
    Line(usize, usize),
    Separator(usize),
}

/// [`view`]'s rows built by index: each hunk coloured as a whole the first time one of its
/// lines shows, and its colours split by line once.
fn virtual_view<S: 'static>(look: Look, lang: Option<String>, fd: FileDiff, out: El) -> El {
    let mut items = Vec::new();
    for (hi, h) in fd.hunks.iter().enumerate() {
        if hi > 0 {
            items.push(Item::Separator(hi));
        }
        items.extend((0..h.lines.len()).map(|li| Item::Line(hi, li)));
    }
    let painted: Rc<[std::cell::OnceCell<Vec<Vec<Highlight>>>]> = fd.hunks.iter().map(|_| std::cell::OnceCell::new()).collect();
    let version = (fd.old_path.clone(), fd.new_path.clone(), items.len());
    let (fd, items) = (Rc::new(fd), Rc::new(items));
    let keys = items.clone();
    let key = move |i: usize| match keys[i] {
        Item::Line(hi, li) => format!("l-{hi}-{li}"),
        Item::Separator(hi) => format!("sep-{hi}"),
    };
    out.growth(rune::GrowthSpec::default())
        .virtual_rows(items.len(), ROW, move |_: &S, i| key(i).into(), move |_: &S, i| match items[i] {
            Item::Separator(hi) => look.separator(hi),
            Item::Line(hi, li) => {
                let h = &fd.hunks[hi];
                let lines = painted[hi].get_or_init(|| HunkPaint::new(h, lang.as_deref()).by_line(h));
                look.line(&h.lines[li], hi, li, &lines[li])
            }
        })
        .virtual_rows_version(version)
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

    /// Each of `h`'s lines' colours, from its own start, as [`Side::within`] cuts them: each
    /// colour found once among the lines its side shows, not once per line.
    fn by_line(&self, h: &Hunk) -> Vec<Vec<Highlight>> {
        let mut out = vec![Vec::new(); h.lines.len()];
        // The old side shows the removed lines; the new one the rest.
        for (side, removed) in [(&self.old, true), (&self.new, false)] {
            let lines: Vec<usize> = (0..h.lines.len()).filter(|&li| (h.lines[li].kind == LineKind::Removed) == removed).collect();
            for (r, paint) in side.0.iter() {
                let first = lines.partition_point(|&li| self.at[li] + h.lines[li].text.len() <= r.start);
                for &li in &lines[first..] {
                    let (at, end) = (self.at[li], self.at[li] + h.lines[li].text.len());
                    if at >= r.end && at > r.start {
                        break;
                    }
                    if r.end > at && r.start < end {
                        out[li].push((r.start.max(at) - at..r.end.min(end) - at, *paint));
                    }
                }
            }
        }
        out
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
    let mut out = col().key("code").dir(rune::Direction::Ltr).w_full().py(4.0).fill(p.code_bg);
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
