//! Generative UI in the timeline: a reply's ```` ```ui ```` outlines draw as parts
//! (`rune_parts`), and everything around them (prose, code, Mermaid) as the Markdown it was.
//!
//! A reply without an outline never comes here. One with an outline is cut into runs of
//! Markdown and outlines: the first run's Markdown is the root, so its blocks keep the paths
//! they had before the fence opened, and each outline and later run joins it as a block.
//!
//! While a reply streams, an outline grows by whole lines only (`rune_parts::parse_note`), and a
//! half-written fence line (```` ``` ````, ```` ```u ````) is held back, so the source never
//! flashes as a code block before the outline takes its place.
//!
//! Find (⌘F) matches what an outline shows in words (`inline::pieces`), and paints them in
//! place. A selection runs through an outline's words like the prose around it, and copying a
//! whole outline copies it as plain indented lines (`rune_parts::to_plain`).

pub mod inline;

use std::ops::Range;

use rune::{El, col};
use rune_kit::markdown::{self, Block, Case};
use rune_parts::Node;
use rune_parts::view::PartsTheme;

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{CONTENT_W, markdown_mark, markdown_style, markdown_styled};

/// A slice of a reply: Markdown as written, or an outline.
#[derive(Debug, PartialEq)]
pub enum Seg {
    Md(Range<usize>),
    Ui {
        /// The fence lines and the outline between them.
        at: Range<usize>,
        /// Where the outline's first line starts.
        src_at: usize,
        /// The fence's line in the reply (1-based), its key.
        line: usize,
        nodes: Vec<Node>,
    },
}

/// Space around an outline, as Markdown spaces a code block (rune-kit's `margins`).
const UI_EDGES: (f32, f32) = (12.0, 24.0);

/// Byte offsets where each line starts.
fn line_starts(s: &str) -> Vec<usize> {
    std::iter::once(0).chain(s.match_indices('\n').map(|(i, _)| i + 1)).collect()
}

/// Whether `line` opens a fence that may yet be a ```` ```ui ```` one.
fn may_open_ui(line: &str) -> bool {
    let t = line.trim_start_matches(' ');
    let Some(c) = t.chars().next().filter(|c| matches!(c, '`' | '~')) else { return false };
    let n = t.chars().take_while(|&x| x == c).count();
    n >= 3 && "ui".starts_with(t[n..].trim_end())
}

/// Cuts `body` into Markdown runs and outlines, or None when it has no outline (nor the start
/// of one held back while streaming), so it draws as it always has.
pub fn split(body: &str, streaming: bool) -> Option<Vec<Seg>> {
    let tail_at = if streaming && !body.ends_with('\n') { body.rfind('\n').map_or(0, |i| i + 1) } else { body.len() };
    let tail_opens = may_open_ui(&body[tail_at..]);
    if !tail_opens && !body.contains("```ui") && !body.contains("~~~ui") {
        return None;
    }
    let blocks = rune_parts::parse_note(body, streaming);
    let in_fence = matches!(blocks.last(), Some(rune_parts::Block::Ui { open: true, .. } | rune_parts::Block::Fence { open: true, .. }));
    let held = tail_opens && !in_fence;
    if !held && !blocks.iter().any(|b| matches!(b, rune_parts::Block::Ui { .. })) {
        return None;
    }
    let starts = line_starts(body);
    let start = |line: usize| starts.get(line).copied().unwrap_or(body.len());
    let end = if held { tail_at } else { body.len() };
    let mut out = Vec::new();
    let mut at = 0;
    for b in blocks {
        let rune_parts::Block::Ui { line, src, nodes, open } = b else { continue };
        let fence = start(line - 1);
        if !body[at..fence].trim().is_empty() {
            out.push(Seg::Md(at..fence));
        }
        let rows = src.split_inclusive('\n').count();
        let close = if open { body.len() } else { start(line + rows + 1) };
        out.push(Seg::Ui { at: fence..close, src_at: start(line), line, nodes });
        at = close;
    }
    if at < end && !body[at..end].trim().is_empty() {
        out.push(Seg::Md(at..end));
    }
    Some(out)
}

/// Matches of `query` (lowercase) in a reply's text as drawn: source byte ranges per match, as
/// `markdown::find` gives them, with an outline's matches in the words it shows.
pub fn find(body: &str, query: &str) -> Vec<Vec<Range<usize>>> {
    let Some(segs) = split(body, false) else { return markdown::find(body, query, Case::Insensitive) };
    let mut out = Vec::new();
    for seg in segs {
        match seg {
            Seg::Md(r) => out.extend(markdown::find(&body[r.clone()], query, Case::Insensitive).into_iter().map(|m| shift(m, r.start))),
            Seg::Ui { src_at, nodes, .. } => {
                for (at, piece) in inline::pieces(body, src_at, &nodes) {
                    out.extend(inline::find(piece, query).into_iter().map(|m| shift(m, at)));
                }
            }
        }
    }
    out
}

fn shift(m: Vec<Range<usize>>, by: usize) -> Vec<Range<usize>> {
    m.into_iter().map(|r| r.start + by..r.end + by).collect()
}

/// A row's Find matches: its source ranges, and the active one.
pub type Hits<'a> = Option<(&'a [Vec<Range<usize>>], Option<usize>)>;

/// Matches lying wholly in `r`, moved to start from it: `(index in the row, ranges, active)`.
fn hits_in(hits: Hits<'_>, r: &Range<usize>) -> Vec<(usize, Vec<Range<usize>>, bool)> {
    let Some((ranges, active)) = hits else { return Vec::new() };
    ranges
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.is_empty() && m.iter().all(|x| x.start >= r.start && x.end <= r.end))
        .map(|(i, m)| (i, m.iter().map(|x| x.start - r.start..x.end - r.start).collect(), active == Some(i)))
        .collect()
}

/// A reply with outlines in it, or None to draw it as plain Markdown.
pub fn reply<S: TimelineHost>(s: &S, p: &Pal, src: &str, fade: f32, streaming: bool, hits: Hits<'_>) -> Option<El> {
    let segs = split(src, streaming)?;
    let width = width(s);
    let mut root: Option<El> = None;
    let mut below = 0.0f32;
    let count = segs.len();
    for (i, seg) in segs.into_iter().enumerate() {
        let (el, (top, bottom)) = match seg {
            Seg::Md(r) => {
                let marks = hits_in(hits, &r).into_iter().map(|(i, m, on)| (m, markdown_mark(p, i, on))).collect();
                let el = markdown_styled::<S, _>(p, &src[r.clone()], fade, streaming && i + 1 == count, markdown_style(p, p.text, width), marks, crate::copied::current(s), Some(crate::images::md_media(s)));
                // The first run is the root and keeps its blocks' keys from before any fence.
                let el = if i == 0 { el } else { el.key(format!("md-{}", r.start)) };
                (el, md_edges(&src[r]))
            }
            Seg::Ui { at, src_at, line, nodes } => {
                let marks: Vec<inline::Mark> = hits_in(hits, &at).into_iter().map(|(i, m, on)| (i, shift(m, at.start), on)).collect();
                let lines: Vec<(usize, u64)> = nodes_lines(&nodes);
                let ver = (lines, src_at, width.to_bits(), fade.to_bits(), p.dark, (p.theme, p.generation), &marks);
                let outline = rune::memo(format!("outline-{line}"), ver, || inline::outline::<S>(p, src, src_at, &nodes, width, fade, marks.clone()));
                // A pane that gains a sibling while it streams springs to its new width.
                (col().key(format!("ui-{line}")).w_full().animate_layout().child(outline), UI_EDGES)
            }
        };
        root = Some(match root {
            None => el,
            Some(r) => r.child(el.mt(top.max(below))),
        });
        below = bottom;
    }
    Some(root.unwrap_or_else(|| col().w_full()))
}

/// Every line of an outline and its hash, what it draws from.
pub fn nodes_lines(nodes: &[Node]) -> Vec<(usize, u64)> {
    let mut out = Vec::new();
    let mut stack: Vec<&Node> = nodes.iter().rev().collect();
    while let Some(n) = stack.pop() {
        out.push((n.line, n.hash));
        stack.extend(n.kids.iter().rev());
    }
    out
}

/// The space Markdown keeps above a run's first block and below its last (rune-kit's `margins`).
fn md_edges(src: &str) -> (f32, f32) {
    let edge = |b: Option<&Block>| match b {
        Some(Block::Heading(1, _)) => (28.0, 12.0),
        Some(Block::Heading(2, _)) => (24.0, 10.0),
        Some(Block::Heading(..)) => (20.0, 8.0),
        Some(Block::Code(_) | Block::Diff(_) | Block::Mermaid(_)) => (12.0, 24.0),
        Some(Block::List { .. }) => (8.0, 12.0),
        Some(Block::Quote(_)) => (16.0, 16.0),
        Some(Block::Rule) => (32.0, 32.0),
        Some(Block::Table(_)) => (12.0, 16.0),
        Some(Block::Image { .. }) => (24.0, 24.0),
        Some(Block::Para(_)) | None => (0.0, 12.0),
    };
    let blocks = markdown::parse_cached(src);
    (edge(blocks.first()).0, edge(blocks.last()).1)
}

/// The width an outline lays out in: the timeline's content column, narrower in a narrow window.
/// Layout rects are in the zoomed shell's points already.
pub fn width<S: TimelineHost>(s: &S) -> f32 {
    let Some(id) = s.active_session_id() else { return CONTENT_W };
    rune::memo::layout_with(&format!("scroll-{id}"), |r| (r[2] - 48.0).clamp(0.0, CONTENT_W)).unwrap_or(CONTENT_W)
}

/// Parts in the timeline's colours: status tones from the theme's diff and state colours, frames
/// with the plain ring panels use, and your bubble as the prompt's.
pub fn theme(p: &Pal) -> PartsTheme {
    PartsTheme {
        text: p.text,
        muted: p.muted,
        faint: p.faint,
        ring: p.ring,
        bg: p.bg_base,
        bubble: p.layer2,
        done: p.diff_add,
        risk: p.diff_del,
        proposal: p.warning,
        working: p.info,
        accent: p.accent,
    }
}
