//! Shared virtualized code & diff rendering primitives for the Review panel, inline tool-step
//! diffs, and the Files / `⌘P` viewer.
//!
//! Uses `rune_kit::syntax` in 128-line chunks so 50,000-line diffs and source files stay smooth
//! at 120 Hz, `.truncate_fade()` on clipped paths (never `"…"`), and `Anim::INSTANT` on hovers.

pub mod data;
pub mod diff;
pub mod file;
pub mod find_bar;
pub mod fold;
pub mod notes;
pub mod prepared;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::LazyLock;

use rune::{Color, KeyEvent, Keybind, Span, TextStyle, Tx};
use rune_kit::syntax::{Highlight, Paint, Syntax, SyntaxTheme, highlight};
use opencode_ui::palette::{Pal, kit};

pub use diff::{DiffDocState, DiffFind, DiffViewer, PreparedDiffDoc, default_file_header};
pub use file::{
    CodeDocState, CodeViewer, Content, ContentEffect, ContentEvent, DocFind, DocFindEvent,
    FileLineSelection,
};
pub use find_bar::{FindBar, find_bar, over};
pub use fold::{ExpandMode, Fold, FoldEffect, FoldEvent, GapFacts, Reveal};
pub use notes::{Handoff, LineNotes, LineRange, diff_range};
pub use prepared::{BAND_LINES, Prepared, Prior, band_keys, drop_off_ui, ensure_band_keys, fresh_line_ids};

/// Desktop's code row: Pierre's line at session-ui's `--diffs-line-height: 24px`
/// (`session-ui/src/pierre/index.ts:325`), in its 13 px mono (`--font-size-small`, `:324`).
pub const LINE_H: f32 = 24.0;
pub const HEADER_H: f32 = 33.0;
/// Desktop's fold separator row (session-ui `[data-separator] { height: 24px }`).
pub const GAP_H: f32 = 24.0;
pub const NOTE_H: f32 = 36.0;
/// A changed line's bar, over the left edge of its number cell (session-ui's
/// `[data-indicators='bars'] [data-column-number]::before { width: 2px }`, `index.ts:119-122`).
pub const BAR_W: f32 = 2.0;
/// Pierre's tab stops (`--diffs-tab-size: 2`, `index.ts:326`).
pub const TAB: u32 = 2;
/// How far in, in mono columns, a line's number starts (`padding-left: 2ch`).
const NUMBER_INSET: f32 = 2.0;
/// Width of the fade where long lines run under the no-wrap viewers' edges.
pub const EDGE_FADE: f32 = 24.0;
pub const CHUNK_LINES: u32 = 128;

/// Keybind matcher for the two host shortcuts code and diff views listen for:
/// Find (`⌘F` / `Ctrl+F`) and Copy selection (`⌘C` / `Ctrl+C`).
pub trait KeyMatch {
    fn is_find(&self, ev: &KeyEvent, tx: &Tx) -> bool;
    fn is_copy_selection(&self, ev: &KeyEvent, tx: &Tx) -> bool;
}

/// Default platform keybindings: `mod+f` for Find and `mod+c` for Copy selection.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultKeys;

impl KeyMatch for DefaultKeys {
    fn is_find(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        static FIND: LazyLock<Keybind> = LazyLock::new(|| Keybind::new("mod+f"));
        FIND.matches_tx(ev, tx)
    }

    fn is_copy_selection(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        static COPY: LazyLock<Keybind> = LazyLock::new(|| Keybind::new("mod+c"));
        COPY.matches_tx(ev, tx)
    }
}

/// Optional instrumentation hooks called when a [`CodeViewer`] builds its view and rows.
#[derive(Clone, Copy)]
pub struct TraceHooks {
    pub on: fn() -> bool,
    pub view: fn(),
    pub row: fn(&str, u32),
}

/// Desktop's code views (and the narrow Changes list) scroll in a plain `overflow-y: auto` box
/// (`[data-scrollable]`), so `scrollbar.css` styles their bar: while the rows overflow it takes an 8 px gutter from their
/// width (`::-webkit-scrollbar { width: 8px }`), and its 4 px pill shows only while the pointer
/// is over the box (`:hover::-webkit-scrollbar-thumb`). The end padding the rows leave for it,
/// from the viewport's and the rows' last laid-out heights.
pub fn scroll_gutter(bar: &rune_kit::ScrollBarStyle, viewport: &str, rows: &str) -> f32 {
    let Some(view) = rune::memo::layout_with(viewport, |r| r[3]) else { return 0.0 };
    if rune::memo::layout_with(rows, move |r| r[3] > view + 0.5) == Some(true) {
        bar.strip()
    } else {
        0.0
    }
}

/// The bar [`scroll_gutter`] leaves room for, drawn over the viewport's end edge.
pub fn scroll_bar(p: &Pal) -> rune_kit::ScrollBarStyle {
    rune_kit::ScrollBarStyle::native(p.border_muted, p.q.border_strong).inside()
}

/// A changed line's colours (`session-ui/src/pierre/index.ts:39-65,119-157`).
#[derive(Clone, Copy)]
pub struct DiffTints {
    /// The row and its number cell, laid over the diff's background.
    pub line: Color,
    /// A changed word's fill and text.
    pub word: Color,
    pub word_text: Color,
    pub number: Color,
    pub bar: Color,
}

#[derive(Clone)]
pub struct CodeLook {
    pub dark: bool,
    pub mono: TextStyle,
    pub mono_small: TextStyle,
    pub label: TextStyle,
    pub label_med: TextStyle,
    pub adv: f32,
    pub bg: Color,
    pub header_bg: Color,
    pub text: Color,
    /// Code no token colours (Pierre's `editor.foreground`, the v1 `text-base`): a Markdown or
    /// plain text file's words, and a fold separator's button under the pointer.
    pub plain: Color,
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub border_muted: Color,
    /// An unchanged row under the pointer (Pierre's `lineHoverHighlight: "both"`).
    pub hover: Color,
    pub add: DiffTints,
    pub del: DiffTints,
    pub empty_bg: Color,
    /// A selected row and its number cell, and the number's colour.
    pub selected: Color,
    pub selected_number: Color,
    /// Selected text.
    pub selection: Color,
    pub match_bg: Color,
    pub active_match_bg: Color,
    pub accent: Color,
    pub syntax: SyntaxTheme,
    pub version: u64,
}

fn tint(base: Color, hue: Color, t: f32) -> Color {
    let [a, b] = [base.to_oklab(), hue.to_oklab()];
    Color::from_oklab([0, 1, 2, 3].map(|i| if i == 3 { a[3] } else { a[i] + (b[i] - a[i]) * t }))
}

impl CodeLook {
    pub fn new(p: &Pal) -> Self {
        let k = kit(p);
        // Desktop draws code on `background-stronger` (session-ui's `--diffs-bg`, `index.ts:33`).
        let bg = p.code_bg;
        let dark = p.dark;
        let d = &p.diff;
        // Pierre's number cell is the computed weight 440 of the page; IBM Plex Mono's Text cut
        // draws every weight below 600 (`app/src/index.css`).
        let mono = TextStyle::new(13.0, 440.0).mono().lh(LINE_H);
        let mono_small = TextStyle::new(11.5, 400.0).mono().lh(16.0);
        let label = TextStyle::new(12.5, 450.0).lh(18.0).tracking(-0.04);
        let label_med = TextStyle::new(12.5, 530.0).lh(18.0).tracking(-0.04);
        let mut h = DefaultHasher::new();
        dark.hash(&mut h);
        for c in [bg, p.text, p.diff_add, p.diff_del, p.info, p.layer1] {
            [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut h);
        }
        (p.theme, p.generation, opencode_i18n::locale().id()).hash(&mut h);
        mono.size.to_bits().hash(&mut h);

        Self {
            dark,
            mono,
            mono_small,
            label,
            label_med,
            adv: 0.6 * mono.size,
            bg,
            header_bg: p.layer1,
            text: p.text,
            plain: p.syntax.parameter,
            muted: p.muted,
            faint: p.faint,
            border: p.border,
            border_muted: p.border_muted,
            hover: d.hover,
            add: DiffTints { line: d.add_row, word: d.add_word, word_text: d.add_word_text, number: d.add_bar, bar: d.add_bar },
            del: DiffTints { line: d.del_row, word: d.del_word, word_text: d.del_word_text, number: d.del_bar, bar: d.del_bar },
            empty_bg: d.empty,
            selected: d.selected,
            selected_number: p.accent,
            // `[data-diff]::selection`: `--timeline-quote-background` (session-ui `pierre/index.ts:76-79`).
            selection: p.timeline_selection,
            match_bg: tint(bg, p.warning, if dark { 0.28 } else { 0.22 }),
            active_match_bg: tint(bg, p.warning, if dark { 0.48 } else { 0.38 }),
            accent: p.info,
            // Desktop colours code with its own theme's `syntax-*` tokens in every theme
            // (`OpenCodeTheme`), the default's included.
            syntax: p.syntax.syntax(SyntaxTheme::from_kit(k), p.diff_add, p.diff_del),
            version: h.finish(),
        }
    }

    /// A line-number cell: Pierre's `[data-column-number]`, 2ch in (`padding-left: 2ch`, room for
    /// the bar), the number right-aligned in at least 3ch (session-ui's
    /// `--diffs-min-number-column-width: 3ch`, `index.ts:331`), then 1ch (`padding-inline: 1ch`).
    pub fn number_w(&self, digits: u32) -> f32 {
        (NUMBER_INSET + digits.max(3) as f32 + 1.0) * self.adv
    }

    /// `highlights` coloured, with `bgs` (word changes, find matches) under them: a Markdown
    /// heading in Desktop's bold mono (its highlight theme's `title`, weight 700). A changed
    /// word (`word`, its fill and text colour) takes that colour over its syntax colour, as
    /// session-ui's `[data-diff-span] { color: … !important }` does (`index.ts:148-157`).
    pub fn spans(&self, len: usize, highlights: &[(Range<usize>, Paint)], bgs: &[(Range<usize>, Color)], word: Option<(Color, Color)>) -> Vec<Span> {
        let syntax: Vec<(Range<usize>, Color)> = highlights.iter().map(|(r, p)| (r.clone(), self.syntax.paint(*p))).collect();
        let mut spans = layer_spans(len, &syntax, bgs);
        for (r, _) in highlights.iter().filter(|(_, p)| *p == Paint::Kind(Syntax::Heading)) {
            for sp in spans.iter_mut().filter(|sp| r.start <= sp.start as usize && sp.end as usize <= r.end) {
                sp.weight = Some(700.0);
            }
        }
        if let Some((fill, ink)) = word {
            for sp in spans.iter_mut().filter(|sp| sp.bg == Some(fill)) {
                sp.color = Some(ink);
            }
        }
        spans
    }
}

/// The language Desktop's diffs and file views colour `path` as (Pierre's file-name map), when
/// Highlights has a lexer for it.
pub fn lang_for_path(path: &str) -> Option<&'static str> {
    rune_kit::syntax::lang_for_path(path)
}

/// Split source lines (`1`-based) with lazy 128-line `rune_kit::syntax` highlight chunks.
#[derive(Clone, Debug, Default)]
pub struct SourceLines {
    // Shared, not copied, with the worker that folds them for Find (`file::state::Slab`).
    text: std::sync::Arc<String>,
    lines: std::sync::Arc<[(u32, u32)]>,
    columns: u32,
}

impl SourceLines {
    pub fn new(src: &str) -> Self {
        Self::from_string(src.to_owned())
    }

    /// `new` for text read on a worker, kept rather than copied.
    pub fn from_string(src: String) -> Self {
        let mut lines = Vec::new();
        let mut max_cols = 0u32;
        let mut start = 0usize;
        for (i, _) in src.match_indices('\n') {
            let end = if src[..i].ends_with('\r') { i - 1 } else { i };
            max_cols = max_cols.max(visual_columns(&src[start..end]));
            lines.push((start as u32, end as u32));
            start = i + 1;
        }
        if start < src.len() {
            let end = if src.ends_with('\r') { src.len() - 1 } else { src.len() };
            max_cols = max_cols.max(visual_columns(&src[start..end]));
            lines.push((start as u32, end as u32));
        }
        Self {
            text: std::sync::Arc::new(src),
            lines: lines.into(),
            columns: max_cols,
        }
    }

    pub fn len(&self) -> u32 {
        self.lines.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn columns(&self) -> u32 {
        self.columns
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// 1-based line slice without its trailing newline.
    pub fn line(&self, n: u32) -> Option<&str> {
        let &(a, b) = self.lines.get((n as usize).checked_sub(1)?)?;
        Some(&self.text[a as usize..b as usize])
    }
}

pub fn visual_columns(line: &str) -> u32 {
    let mut col = 0u32;
    for ch in line.chars() {
        if ch == '\t' {
            col += TAB - (col % TAB);
        } else {
            col += 1;
        }
    }
    col
}

/// Cached 128-line highlight chunk for either a `SourceLines` or a slice of lines.
pub struct HighlightChunk {
    starts: Vec<u32>,
    hl: std::sync::Arc<[Highlight]>,
    line_hashes: Box<[u64]>,
}

/// Bytes of the lines before a chunk that its highlighting reads, so a comment or string opened
/// above it keeps its colour, as Desktop colours a whole file at once. Within `rune_kit::syntax`'s
/// 128 KiB, so every chunk of a file up to that size is coloured exactly as the whole file is.
const CONTEXT_BYTES: usize = 120 * 1024;

fn hash_paint(paint: Paint, h: &mut impl Hasher) {
    match paint {
        Paint::Kind(k) => {
            0u8.hash(h);
            k.hash(h);
        }
        Paint::Color(c) => {
            1u8.hash(h);
            [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(h);
        }
    }
}

impl HighlightChunk {
    /// Lines `first..=last` of a text whose 1-based lines `line` gives, coloured after up to
    /// [`CONTEXT_BYTES`] of the lines before them. Consecutive chunks extend one source, which
    /// `rune_kit::syntax` lexes incrementally.
    pub fn lines<'a>(lang: Option<&str>, first: u32, last: u32, line: impl Fn(u32) -> &'a str) -> std::sync::Arc<Self> {
        Self::build_with(first, last, line, |buf, base| {
            highlight(lang, buf)
                .iter()
                .filter(|(r, _)| r.end > base)
                .map(|(r, p)| (r.start.max(base) - base..r.end - base, *p))
                .collect()
        })
    }

    /// [`Self::lines`] for a background read worker, using `builtin` directly so short-lived
    /// pool threads do not hold cached 128 KiB sources in thread-local storage.
    pub fn lines_worker<'a>(lang: Option<&str>, first: u32, last: u32, line: impl Fn(u32) -> &'a str) -> std::sync::Arc<Self> {
        Self::build_with(first, last, line, |buf, base| {
            rune_kit::syntax::builtin(lang, buf)
                .into_iter()
                .filter(|(r, _)| r.end > base)
                .map(|(r, p)| (r.start.max(base) - base..r.end - base, p))
                .collect()
        })
    }

    fn build_with<'a>(
        first: u32,
        last: u32,
        line: impl Fn(u32) -> &'a str,
        lex: impl FnOnce(&str, usize) -> Vec<Highlight>,
    ) -> std::sync::Arc<Self> {
        let mut from = first;
        let mut context = 0;
        while from > 1 && context + line(from - 1).len() < CONTEXT_BYTES {
            from -= 1;
            context += line(from).len() + 1;
        }
        let mut buf = String::with_capacity(context);
        for m in from..first {
            buf.push_str(line(m));
            buf.push('\n');
        }
        let base = buf.len();
        let mut starts = Vec::with_capacity(CHUNK_LINES as usize + 1);
        for m in first..=last {
            starts.push((buf.len() - base) as u32);
            buf.push_str(line(m));
            buf.push('\n');
        }
        starts.push((buf.len() - base) as u32);
        let hl: std::sync::Arc<[Highlight]> = lex(&buf, base).into();
        let line_count = starts.len().saturating_sub(1);
        let mut line_hashes = Vec::with_capacity(line_count);
        let mut idx = 0usize;
        for offset in 0..line_count {
            let a = starts[offset] as usize;
            let b = starts[offset + 1].saturating_sub(1) as usize;
            let mut h = DefaultHasher::new();
            if a < b {
                while idx < hl.len() && hl[idx].0.end <= a {
                    idx += 1;
                }
                for (r, paint) in &hl[idx..] {
                    if r.start >= b {
                        break;
                    }
                    let s = r.start.max(a) - a;
                    let e = r.end.min(b) - a;
                    if s < e {
                        (s, e).hash(&mut h);
                        hash_paint(*paint, &mut h);
                    }
                }
            }
            line_hashes.push(h.finish());
        }
        std::sync::Arc::new(Self {
            starts,
            hl,
            line_hashes: line_hashes.into_boxed_slice(),
        })
    }

    pub fn line_hash(&self, offset: usize) -> u64 {
        self.line_hashes.get(offset).copied().unwrap_or(0)
    }

    /// Token highlights within `offset` (`0..CHUNK_LINES`), shifted to line-local byte offsets.
    pub fn line_highlights(&self, offset: usize) -> Vec<Highlight> {
        let Some(&a) = self.starts.get(offset) else {
            return Vec::new();
        };
        let b = self.starts.get(offset + 1).copied().unwrap_or(a).saturating_sub(1);
        if a >= b {
            return Vec::new();
        }
        let (a, b) = (a as usize, b as usize);
        let start_idx = self.hl.partition_point(|(r, _)| r.end <= a);
        let mut out = Vec::new();
        for (r, paint) in &self.hl[start_idx..] {
            if r.start >= b {
                break;
            }
            let s = r.start.max(a) - a;
            let e = r.end.min(b) - a;
            if s < e {
                out.push((s..e, *paint));
            }
        }
        out
    }
}

pub type ChunkMap<K> = HashMap<K, std::sync::Arc<HighlightChunk>>;

/// The rows' laid-out heights by row key, as their layouts report them: what a drag down the
/// line numbers walks, since a wrapped line or one with comments under it is taller than its
/// estimate.
#[derive(Debug, Default)]
pub struct RowHeights(std::cell::RefCell<HashMap<String, f32>>);

impl RowHeights {
    pub fn get(&self, key: &str) -> Option<f32> {
        self.0.borrow().get(key).copied()
    }

    /// Records row `key`'s height; whether a height it had changed (a row seen for the first
    /// time, as one scrolls in, isn't a change).
    pub fn record(&self, key: &str, h: f32) -> bool {
        let mut map = self.0.borrow_mut();
        match map.insert(key.to_owned(), h) {
            Some(old) => (old - h).abs() >= 0.25,
            None => false,
        }
    }

    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    /// Keeps laid-out heights for `cl1..=cl{prefix}` across an edit below `prefix`.
    pub fn retain_prefix(&self, prefix: u32) {
        self.0.borrow_mut().retain(|k, _| {
            k.strip_prefix("cl")
                .and_then(|n| n.parse::<u32>().ok())
                .is_some_and(|n| n <= prefix)
        });
    }
}

/// Reports a row's height to `record` (`RowHeights::record`), running no view unless a height
/// changed (so scrolling, which moves every row and mounts new ones, costs no commit).
pub fn report_height<S: 'static>(row: rune::El, record: impl Fn(&mut S, f32) -> bool + 'static) -> rune::El {
    row.on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
        if !record(s, r[3]) {
            tx.live_only();
        }
    })
}

/// Combines syntax foreground colors with background highlights (word diffs or search matches).
pub fn layer_spans(
    len: usize,
    syntax: &[(Range<usize>, Color)],
    bgs: &[(Range<usize>, Color)],
) -> Vec<Span> {
    if syntax.is_empty() && bgs.is_empty() {
        return Vec::new();
    }
    let mut cuts: Vec<usize> = syntax
        .iter()
        .flat_map(|(r, _)| [r.start.min(len), r.end.min(len)])
        .chain(bgs.iter().flat_map(|(r, _)| [r.start.min(len), r.end.min(len)]))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a >= b {
            continue;
        }
        let fg = syntax
            .iter()
            .find(|(r, _)| r.start <= a && b <= r.end)
            .map(|(_, c)| *c);
        let bg = bgs
            .iter()
            .rfind(|(r, _)| r.start <= a && b <= r.end)
            .map(|(_, c)| *c);
        if fg.is_some() || bg.is_some() {
            out.push(Span {
                color: fg,
                bg,
                ..Span::new(a, b)
            });
        }
    }
    out
}

/// Expands `\t` to the next [`TAB`] stop and shifts `spans` accordingly.
pub fn expand_tabs(line: &str, spans: Vec<Span>) -> (String, Vec<Span>) {
    if !line.contains('\t') {
        return (line.to_owned(), spans);
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut map = vec![0usize; line.len() + 1];
    let mut col = 0usize;
    let tab = TAB as usize;
    for (byte_i, ch) in line.char_indices() {
        map[byte_i] = out.len();
        if ch == '\t' {
            let n = tab - (col % tab);
            for _ in 0..n {
                out.push(' ');
            }
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    map[line.len()] = out.len();
    let shifted = spans
        .into_iter()
        .filter_map(|mut sp| {
            let s = *map.get(sp.start as usize)?;
            let e = *map.get(sp.end as usize)?;
            (s < e).then(|| {
                sp.start = s as u32;
                sp.end = e as u32;
                sp
            })
        })
        .collect();
    (out, shifted)
}
