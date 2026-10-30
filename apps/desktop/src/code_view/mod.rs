//! Shared virtualized code & diff rendering primitives for the Review panel, inline tool-step
//! diffs, and the Files / `⌘P` viewer.
//!
//! Uses `rune_kit::syntax` in 128-line chunks so 50,000-line diffs and source files stay smooth
//! at 120 Hz, `.truncate_fade()` on clipped paths (never `"…"`), and `Anim::INSTANT` on hovers.

pub mod diff_nav;
pub mod diff_state;
pub mod diff_view;
pub mod file_state;
pub mod file_view;
#[cfg(test)]
mod edge_fade_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use rune::{Color, Span, TextStyle};
use rune_kit::syntax::{Highlight, SyntaxTheme, highlight};

use crate::palette::Pal;

pub use diff_state::{DiffDocState, ExpandMode};
pub use diff_view::DiffViewer;
pub use file_state::{CodeDocState, FileLineSelection};
pub use file_view::CodeViewer;

pub const LINE_H: f32 = 20.0;
pub const HEADER_H: f32 = 38.0;
pub const GAP_H: f32 = 28.0;
pub const NOTE_H: f32 = 36.0;
pub const BAR_W: f32 = 3.0;
pub const CODE_PAD: (f32, f32) = (10.0, 20.0);
/// Width of the fade where long lines run under the no-wrap viewers' edges.
pub const EDGE_FADE: f32 = 24.0;
pub const CHUNK_LINES: u32 = 128;

#[derive(Clone, Copy)]
pub struct DiffTints {
    pub line: Color,
    pub gutter: Color,
    pub word: Color,
    pub number: Color,
    pub bar: Color,
    pub count: Color,
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
    pub muted: Color,
    pub faint: Color,
    pub border: Color,
    pub border_muted: Color,
    pub hover: Color,
    pub add: DiffTints,
    pub del: DiffTints,
    pub gap_bg: Color,
    pub empty_bg: Color,
    pub selected: Color,
    pub selected_gutter: Color,
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
        let kit = crate::palette::kit(p);
        let bg = p.bg_base;
        let dark = p.dark;
        let mk_tints = |hue: Color| DiffTints {
            line: tint(bg, hue, if dark { 0.13 } else { 0.095 }),
            gutter: tint(bg, hue, if dark { 0.20 } else { 0.16 }),
            word: tint(bg, hue, if dark { 0.32 } else { 0.26 }),
            number: tint(p.muted, hue, 0.7),
            bar: hue,
            count: hue,
        };
        let mono = TextStyle::new(12.5, 400.0).mono().lh(LINE_H);
        let mono_small = TextStyle::new(11.5, 400.0).mono().lh(16.0);
        let label = TextStyle::new(12.5, 450.0).lh(18.0).tracking(-0.04);
        let label_med = TextStyle::new(12.5, 530.0).lh(18.0).tracking(-0.04);
        let mut h = DefaultHasher::new();
        dark.hash(&mut h);
        for c in [bg, p.text, p.diff_add, p.diff_del, p.info, p.layer1] {
            [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut h);
        }
        p.theme.hash(&mut h);
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
            muted: p.muted,
            faint: p.faint,
            border: p.border,
            border_muted: p.border_muted,
            hover: p.hover,
            add: mk_tints(p.diff_add),
            del: mk_tints(p.diff_del),
            gap_bg: tint(bg, p.info, if dark { 0.09 } else { 0.055 }),
            empty_bg: tint(bg, p.text, if dark { 0.03 } else { 0.025 }),
            selected: tint(bg, p.info, if dark { 0.20 } else { 0.14 }),
            selected_gutter: tint(bg, p.info, if dark { 0.32 } else { 0.25 }),
            selection: p.selection,
            match_bg: tint(bg, p.warning, if dark { 0.28 } else { 0.22 }),
            active_match_bg: tint(bg, p.warning, if dark { 0.48 } else { 0.38 }),
            accent: p.info,
            syntax: match p.code {
                Some(code) => code.syntax(SyntaxTheme::from_kit(kit), p.diff_add, p.diff_del),
                None => SyntaxTheme::from_kit(kit),
            },
            version: h.finish(),
        }
    }

    pub fn number_w(&self, digits: u32) -> f32 {
        (digits.max(3) as f32 * self.adv + 14.0).ceil()
    }
}

/// Maps a file path to a `rune_kit::syntax` language tag, when supported.
pub fn lang_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name {
        "Dockerfile" | "Makefile" | "Justfile" | ".zshrc" | ".bashrc" => return Some("sh"),
        _ => {}
    }
    let (_, ext) = name.rsplit_once('.')?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "rs" => "rs",
        "ts" | "mts" | "cts" => "ts",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" | "jsx" => "js",
        "json" | "jsonc" | "json5" => "json",
        "py" | "pyi" => "py",
        "go" => "go",
        "swift" => "swift",
        "sh" | "bash" | "zsh" => "sh",
        "sql" => "sql",
        "html" | "htm" | "xhtml" | "xml" | "svg" => "html",
        "css" | "scss" => "css",
        "yaml" | "yml" => "yaml",
        "diff" | "patch" => "diff",
        _ => return None,
    })
}

/// Split source lines (`1`-based) with lazy 128-line `rune_kit::syntax` highlight chunks.
#[derive(Clone, Debug, Default)]
pub struct SourceLines {
    text: Rc<String>,
    lines: Rc<[(u32, u32)]>,
    columns: u32,
}

impl SourceLines {
    pub fn new(src: &str) -> Self {
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
            text: Rc::new(src.to_owned()),
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
            col += 4 - (col % 4);
        } else {
            col += 1;
        }
    }
    col
}

/// Cached 128-line highlight chunk for either a `SourceLines` or a slice of lines.
pub struct HighlightChunk {
    starts: Vec<u32>,
    hl: Rc<[Highlight]>,
}

impl HighlightChunk {
    pub fn from_lines<'a>(lang: Option<&str>, iter: impl IntoIterator<Item = &'a str>) -> Rc<Self> {
        let mut buf = String::new();
        let mut starts = Vec::with_capacity(CHUNK_LINES as usize + 1);
        for line in iter {
            starts.push(buf.len() as u32);
            buf.push_str(line);
            buf.push('\n');
        }
        starts.push(buf.len() as u32);
        let hl = highlight(lang, &buf);
        Rc::new(Self { starts, hl })
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

pub type ChunkMap<K> = HashMap<K, Rc<HighlightChunk>>;

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

/// Expands `\t` into 1..=4 spaces and shifts `spans` accordingly.
pub fn expand_tabs(line: &str, spans: Vec<Span>) -> (String, Vec<Span>) {
    if !line.contains('\t') {
        return (line.to_owned(), spans);
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut map = vec![0usize; line.len() + 1];
    let mut col = 0usize;
    for (byte_i, ch) in line.char_indices() {
        map[byte_i] = out.len();
        if ch == '\t' {
            let n = 4 - (col % 4);
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
