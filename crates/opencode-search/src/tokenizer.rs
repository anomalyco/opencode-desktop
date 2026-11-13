//! Document and query analysis shared by the in-memory and SQLite clients.

use std::ops::Range;

use crate::SearchFold;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Document,
    Query,
}

#[derive(Debug)]
pub struct Token {
    pub text: String,
    pub bytes: Range<usize>,
    /// Alternative spelling at the previous token's position, not another word.
    pub colocated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Separator,
    Word,
    Ideographic,
    SoutheastAsian,
}

fn class(c: char) -> Class {
    match c as u32 {
        0x3040..=0x312f
        | 0x31a0..=0x31bf
        | 0x3400..=0x4dbf
        | 0x4e00..=0x9fff
        | 0xac00..=0xd7af
        | 0xf900..=0xfaff
        | 0xff66..=0xff9f
        | 0x20000..=0x323af => Class::Ideographic,
        0xe00..=0xeff | 0x1000..=0x109f | 0x1780..=0x17ff if c.is_alphabetic() => {
            Class::SoutheastAsian
        }
        _ if c.is_alphanumeric() => Class::Word,
        _ => Class::Separator,
    }
}

pub fn tokenize(text: &str, mode: Mode, mut emit: impl FnMut(Token)) {
    let mut clusters = text.grapheme_indices(true).peekable();
    let fold = SearchFold::default();
    while let Some((start, g)) = clusters.next() {
        let first = g.chars().next().unwrap_or(' ');
        let kind = class(first);
        if kind == Class::Separator {
            continue;
        }
        let mut run = vec![(start, first)];
        while let Some(&(p, cluster)) = clusters.peek() {
            let c = cluster.chars().next().unwrap_or(' ');
            if class(c) != kind {
                break;
            }
            clusters.next();
            run.push((p, c));
        }
        let end = clusters.peek().map_or(text.len(), |r| r.0);
        let mut token = |bytes: Range<usize>, colocated| {
            let value = fold.fold(&text[bytes.clone()]);
            if !value.is_empty() {
                emit(Token {
                    bytes,
                    text: value,
                    colocated,
                });
            }
        };
        if kind != Class::Word {
            for (i, (p, _)) in run.iter().enumerate() {
                if i + 1 < run.len() || mode == Mode::Document || run.len() == 1 {
                    token(*p..run.get(i + 2).map_or(end, |r| r.0), false);
                }
            }
            continue;
        }
        let mut part = start;
        for i in 1..run.len() {
            let (p, c) = run[i];
            let prev = run[i - 1].1;
            if prev.is_numeric() != c.is_numeric()
                || c.is_uppercase()
                    && (prev.is_lowercase()
                        || run.get(i + 1).is_some_and(|r| r.1.is_lowercase())
                            && prev.is_uppercase())
            {
                token(part..p, false);
                if part == start && mode == Mode::Document {
                    token(start..end, true);
                }
                part = p;
            }
        }
        token(part..end, false);
    }
}

/// Quote the user's words rather than parse user input as FTS syntax. Keep the
/// original script runs intact so the FTS tokenizer sees a phrase of bigrams,
/// not a conjunction of disconnected overlapping tokens. Prefix only the unfinished word.
pub fn fts_query(raw: &str) -> Option<String> {
    let words: Vec<_> = raw
        .split_whitespace()
        .filter(|w| {
            let mut any = false;
            tokenize(w, Mode::Query, |_| any = true);
            any
        })
        .collect();
    if words.is_empty() {
        return None;
    }
    let mut out = words
        .iter()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ");
    if !raw.ends_with(char::is_whitespace) {
        out.push('*');
    }
    Some(out)
}
