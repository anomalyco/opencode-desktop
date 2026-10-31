//! A whole note or chat reply, cut into prose, ```` ```ui ```` outlines and other fences.
//!
//! While a reply streams, `streaming` keeps the half-written last line out of fences, so an
//! outline only grows by whole lines and an unclosed trailing fence already draws as its part.

use crate::outline::{Node, parse_outline};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Markdown between fences, as written.
    Prose { line: usize, text: String },
    /// `src` is the outline as written, `nodes` its parse.
    Ui { line: usize, src: String, nodes: Vec<Node>, open: bool },
    /// Any other fence (code, `interface`, `mermaid`…), left to the surface.
    Fence { line: usize, lang: String, body: String, open: bool },
}

impl Block {
    /// The note line the block starts on (the fence line for fences), 1-based.
    pub fn line(&self) -> usize {
        match self {
            Block::Prose { line, .. } | Block::Ui { line, .. } | Block::Fence { line, .. } => *line,
        }
    }
}

/// ```` ``` ```` or `~~~` (three or more) at most three spaces in: the marker and the info string.
pub(crate) fn fence_open(line: &str) -> Option<(char, usize, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let c = t.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let n = t.chars().take_while(|&x| x == c).count();
    let info = t[n..].trim();
    (n >= 3 && !(c == '`' && info.contains('`'))).then_some((c, n, info))
}

fn fence_close(line: &str, c: char, n: usize) -> bool {
    let t = line.trim();
    line.len() - line.trim_start_matches(' ').len() <= 3 && t.len() >= n && t.chars().all(|x| x == c)
}

pub fn parse_note(src: &str, streaming: bool) -> Vec<Block> {
    let mut lines: Vec<&str> = src.split_inclusive('\n').collect();
    let partial = streaming && lines.last().is_some_and(|l| !l.ends_with('\n'));
    let tail = if partial { lines.pop() } else { None };

    let mut out = Vec::new();
    let mut prose = String::new();
    let mut prose_line = 1;
    let mut i = 0;
    while i < lines.len() {
        let Some((c, n, info)) = fence_open(lines[i].trim_end_matches(['\n', '\r'])) else {
            if prose.is_empty() {
                prose_line = i + 1;
            }
            prose.push_str(lines[i]);
            i += 1;
            continue;
        };
        if !prose.trim().is_empty() {
            out.push(Block::Prose { line: prose_line, text: std::mem::take(&mut prose) });
        }
        let start = i + 1;
        let lang = info.split_whitespace().next().unwrap_or("").to_owned();
        let mut body = String::new();
        i += 1;
        let mut open = true;
        while i < lines.len() {
            if fence_close(lines[i].trim_end_matches(['\n', '\r']), c, n) {
                open = false;
                i += 1;
                break;
            }
            body.push_str(lines[i]);
            i += 1;
        }
        out.push(if lang == "ui" {
            Block::Ui { line: start, nodes: parse_outline(&body), src: body, open }
        } else {
            Block::Fence { line: start, lang, body, open }
        });
    }
    let in_fence = matches!(out.last(), Some(Block::Ui { open: true, .. } | Block::Fence { open: true, .. })) && prose.is_empty();
    if let Some(t) = tail.filter(|t| !in_fence && fence_open(t).is_none()) {
        if prose.is_empty() {
            prose_line = lines.len() + 1;
        }
        prose.push_str(t);
    }
    if !prose.trim().is_empty() {
        out.push(Block::Prose { line: prose_line, text: prose });
    }
    out
}
