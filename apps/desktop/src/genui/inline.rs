//! An outline drawn in the timeline: its words in the timeline's Markdown look (inline code,
//! links), selectable like the prose around it, with Find's matches painted in place.

use std::ops::Range;

use rune::{Color, El, TextStyle, col, text};
use rune_kit::markdown::{self, Case};
use rune_parts::Node;
use rune_parts::view::{PartsResolver, outline as draw};

use crate::find::{markdown_mark, text_mark};
use crate::palette::Pal;
use crate::view::{markdown_style, markdown_styled, readable};

/// Parts whose words draw through the resolver, so Find can paint them; the rest of an
/// outline's words (labels, details, buttons) are drawn by `rune_parts` itself.
const WORDS: &[&str] = &["title", "text", "item", "kv", "toggle", "say"];

/// The parts whose words draw through the resolver, in order, each with where its words start
/// in `body`. `src_at` is where the outline's first line starts.
fn located<'n>(body: &str, src_at: usize, nodes: &'n [Node]) -> Vec<(&'n Node, usize)> {
    fn walk<'n>(body: &str, starts: &[usize], nodes: &'n [Node], out: &mut Vec<(&'n Node, usize)>) {
        for n in nodes {
            let words = &n.part.text;
            if WORDS.contains(&n.part.name.as_str()) && !words.is_empty()
                && let Some(&at) = starts.get(n.line - 1)
            {
                let end = body[at..].find('\n').map_or(body.len(), |i| at + i);
                let from = at + words_from(&body[at..end], &n.part.name);
                if let Some(i) = body[from..end].find(words.as_str()) {
                    out.push((n, from + i));
                }
            }
            walk(body, starts, &n.kids, out);
        }
    }
    let starts: Vec<usize> = super::line_starts(&body[src_at..]).into_iter().map(|i| i + src_at).collect();
    let mut out = Vec::new();
    walk(body, &starts, nodes, &mut out);
    out
}

/// Where a line's words can start: past its name, a `kv` label or `say` speaker, and a `{tone}`.
fn words_from(line: &str, name: &str) -> usize {
    let mut at = line.find(name).map_or(0, |i| i + name.len());
    if matches!(name, "kv" | "say") {
        at += line[at..].find(':').map_or(0, |i| i + 1);
    }
    let rest = &line[at..];
    let lead = rest.len() - rest.trim_start().len();
    if rest.trim_start().starts_with('{') && let Some(i) = rest.find('}') {
        return at + i + 1;
    }
    at + lead
}

/// The words an outline shows through the resolver, in order: `(offset in body, words)`.
pub(super) fn pieces<'a>(body: &'a str, src_at: usize, nodes: &[Node]) -> Vec<(usize, &'a str)> {
    located(body, src_at, nodes).into_iter().map(|(n, at)| (at, &body[at..at + n.part.text.len()])).collect()
}

/// Matches of `query` in one part's words, as they draw.
pub(super) fn find(words: &str, query: &str) -> Vec<Vec<Range<usize>>> {
    markdown::find(words, query, Case::Insensitive)
}

/// Words that need Markdown to draw as written; the rest draw as plain text, which is cheaper.
fn rich(words: &str) -> bool {
    words.contains(['`', '*', '_', '[', '<', '&', '\\', '~']) || words.contains("://") || words.contains("www.")
}

/// A Find match in an outline: `(index in the row, source ranges in the reply, active)`.
pub(super) type Mark = (usize, Vec<Range<usize>>, bool);

struct Resolver<'a> {
    p: &'a Pal,
    fade: f32,
    width: f32,
    /// Each drawn piece of words by where its `String` lives: `(ptr, len, offset in the reply)`.
    spans: Vec<(*const u8, usize, usize)>,
    marks: Vec<Mark>,
}

impl Resolver<'_> {
    /// The matches inside the words at `src`, moved to start from them.
    fn marks_for(&self, src: &str) -> Vec<Mark> {
        let Some(&(_, len, at)) = self.spans.iter().find(|(ptr, len, _)| *ptr == src.as_ptr() && *len == src.len()) else {
            return Vec::new();
        };
        self.marks
            .iter()
            .filter(|(_, m, _)| m.iter().all(|r| r.start >= at && r.end <= at + len))
            .map(|(i, m, on)| (*i, m.iter().map(|r| r.start - at..r.end - at).collect(), *on))
            .collect()
    }
}

impl PartsResolver for Resolver<'_> {
    fn inline(&self, src: &str, font: TextStyle, color: Color) -> El {
        let p = self.p;
        let marks = self.marks_for(src);
        if rich(src) {
            let marks: Vec<_> = marks.into_iter().map(|(i, m, on)| (m, markdown_mark(p, i, on))).collect();
            return col().w_full().child(markdown_styled(p, src, self.fade, false, markdown_style(p, color, self.width).body_font(font), marks));
        }
        let hl = marks.into_iter().filter_map(|(i, m, on)| Some(text_mark(p, m.first()?.clone(), i, on)));
        readable(p, text(src.to_owned()).font(font).color(color).wrap().reveal(self.fade).text_highlights(hl))
    }
}

/// The outline `nodes` (from `src`, its first line at `src_at`) `width` points wide.
pub(super) fn outline(p: &Pal, src: &str, src_at: usize, nodes: &[Node], width: f32, fade: f32, marks: Vec<Mark>) -> El {
    let spans = located(src, src_at, nodes).into_iter().map(|(n, at)| (n.part.text.as_ptr(), n.part.text.len(), at)).collect();
    let r = Resolver { p, fade, width, spans, marks };
    let plain = rune_parts::to_plain(nodes);
    col().w_full().selection_group(false)
        .selection_copy(move |sel| sel.texts().iter().all(|t| t.is_whole()).then(|| plain.clone()))
        .child(draw(nodes, width, &super::theme(p), &r))
}
