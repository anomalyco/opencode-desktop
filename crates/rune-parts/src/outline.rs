//! The outline inside a ```` ```ui ```` fence: one part per line, kids indented under it.
//!
//! The parser never fails. A line it can't place still becomes a node, so a half-written
//! outline draws while it streams; [`crate::check`] is where mistakes become errors.

use crate::catalog::{PartSpec, spec};

/// One line's part: its name and the words after it, split the way the name asks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    /// The first word, as written (an unknown name stays as typed).
    pub name: String,
    /// `{word}` after the name: a tone, `say`'s how, `toggle`'s on or off.
    pub tone: Option<String>,
    /// `kv`'s label or `say`'s speaker.
    pub key: String,
    pub text: String,
    /// What follows ` · `, drawn quietly.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub part: Part,
    /// 1-based line in the outline, so keys stay put as lines stream in below.
    pub line: usize,
    /// Leading spaces.
    pub indent: usize,
    /// The line's own hash: a view may reuse what it drew for an unchanged `(line, hash)`.
    pub hash: u64,
    pub kids: Vec<Node>,
}

impl Node {
    pub fn spec(&self) -> Option<&'static PartSpec> {
        spec(&self.part.name)
    }

    /// A stable key within its outline: `L{line}`.
    pub fn key(&self) -> String {
        format!("L{}", self.line)
    }

    /// Every word the line shows, for counting and for plain text.
    pub fn words(&self) -> String {
        let p = &self.part;
        let mut out = match (p.key.is_empty(), p.text.is_empty()) {
            (false, false) => format!("{}: {}", p.key, p.text),
            (false, true) => p.key.clone(),
            _ => p.text.clone(),
        };
        if !p.detail.is_empty() {
            out.push_str(" · ");
            out.push_str(&p.detail);
        }
        out
    }
}

/// `{word} rest` → (`word`, `rest`).
pub(crate) fn tone_prefix(s: &str) -> (Option<String>, &str) {
    let Some(rest) = s.strip_prefix('{') else { return (None, s) };
    match rest.split_once('}') {
        Some((w, after)) if !w.trim().is_empty() && w.len() <= 32 && !w.contains('{') => (Some(w.trim().to_owned()), after.trim_start()),
        _ => (None, s),
    }
}

fn split_detail(s: &str) -> (String, String) {
    match s.split_once(" · ") {
        Some((t, d)) => (t.trim().to_owned(), d.trim().to_owned()),
        None => (s.trim().to_owned(), String::new()),
    }
}

pub(crate) fn parse_part(line: &str) -> Part {
    let line = line.trim();
    let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    let mut p = Part { name: name.to_owned(), ..Part::default() };
    match name {
        "say" => {
            let (head, words) = match rest.split_once(": ") {
                Some((h, w)) => (h, w),
                None => match rest.strip_suffix(':') {
                    Some(h) => (h, ""),
                    None => ("", rest),
                },
            };
            let (who, how) = match head.split_once('{') {
                Some((who, how)) => (who.trim(), Some(how.trim_end().trim_end_matches('}').trim().to_owned())),
                None => (head.trim(), None),
            };
            p.key = who.to_owned();
            p.tone = how.filter(|h| !h.is_empty());
            p.text = words.trim().to_owned();
        }
        "kv" => {
            let (label, value) = rest.split_once(':').unwrap_or((rest, ""));
            let (tone, value) = tone_prefix(value.trim());
            p.key = label.trim().to_owned();
            p.tone = tone;
            (p.text, p.detail) = split_detail(value);
        }
        "chips" | "tools" => p.text = rest.to_owned(),
        _ => {
            let (tone, rest) = tone_prefix(rest);
            p.tone = tone;
            (p.text, p.detail) = split_detail(rest);
        }
    }
    p
}

/// FNV-1a: the same on every target, so a cache keyed by it survives Wasm and native alike.
pub(crate) fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Leading width, a tab counting as two spaces.
pub(crate) fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 2 } else { 1 }).sum()
}

pub fn parse_outline(src: &str) -> Vec<Node> {
    let lines: Vec<(usize, usize, &str)> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i + 1, indent_of(l), l))
        .collect();
    fn take(lines: &[(usize, usize, &str)], at: &mut usize, depth: usize) -> Vec<Node> {
        let mut out = Vec::new();
        while let Some(&(line, indent, raw)) = lines.get(*at).filter(|l| l.1 >= depth) {
            *at += 1;
            let kids = match lines.get(*at) {
                Some(&(_, next, _)) if next > indent => take(lines, at, next),
                _ => Vec::new(),
            };
            out.push(Node { part: parse_part(raw), line, indent, hash: hash(raw), kids });
        }
        out
    }
    take(&lines, &mut 0, 0)
}

/// The outline as indented plain lines: what a screen reader, a notification or a word count sees.
pub fn to_plain(nodes: &[Node]) -> String {
    fn walk(out: &mut String, nodes: &[Node], depth: usize) {
        for n in nodes {
            let w = n.words();
            if !w.is_empty() {
                out.push_str(&"  ".repeat(depth));
                out.push_str(&w);
                out.push('\n');
            }
            walk(out, &n.kids, depth + usize::from(!w.is_empty()));
        }
    }
    let mut out = String::new();
    walk(&mut out, nodes, 0);
    out
}
