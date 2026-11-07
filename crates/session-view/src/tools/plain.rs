//! A tool's text as Desktop shows it: ANSI escapes stripped (`tools/ansi.ts`), line ends made
//! `\n`, URLs made links (`LinkedText`), and a one-line subtitle's whitespace collapsed.

use std::borrow::Cow;
use std::ops::Range;

use rune::{El, Link, LinkPaint, Span, Tx, text};

use crate::TimelineHost;
use crate::palette::Pal;

/// `text` without ANSI escapes, as Desktop's `stripAnsi` (ansi-regex 6.3) removes them: an OSC
/// sequence through its BEL, `ESC \` or 0x9c terminator, and CSI and related escapes. An `ESC ]`
/// whose terminator hasn't streamed in yet is only an escape, so the text after it stays.
pub fn strip_ansi(text: &str) -> Cow<'_, str> {
    if !text.contains(['\u{1b}', '\u{9b}']) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['\u{1b}', '\u{9b}']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        match escape_len(tail) {
            Some(n) => rest = &tail[n..],
            None => {
                let ch = tail.chars().next().map_or(1, char::len_utf8);
                out.push_str(&tail[..ch]);
                rest = &tail[ch..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The byte length of the escape `s` starts with, if it is one.
fn escape_len(s: &str) -> Option<usize> {
    if let Some(payload) = s.strip_prefix("\u{1b}]") {
        let stop = payload.find(['\u{7}', '\u{1b}', '\u{9c}']);
        let end = stop.and_then(|i| {
            let t = &payload[i..];
            if t.starts_with('\u{7}') {
                Some(i + 1)
            } else if t.starts_with("\u{1b}\\") {
                Some(i + 2)
            } else if t.starts_with('\u{9c}') {
                Some(i + '\u{9c}'.len_utf8())
            } else {
                None
            }
        });
        if let Some(end) = end {
            return Some(2 + end);
        }
    }
    let lead = s.chars().next()?.len_utf8();
    let b = &s.as_bytes()[lead..];
    let mut i = b.iter().take_while(|c| b"[]()#;?".contains(c)).count();
    let start = i;
    // Parameters: up to four digits, then `;` or `:` and up to four digits, repeated.
    let digits = |from: usize| b[from..].iter().take(4).take_while(|c| c.is_ascii_digit()).count();
    let n = digits(i);
    if n > 0 {
        i += n;
        while i < b.len() && (b[i] == b';' || b[i] == b':') {
            i += 1;
            i += digits(i);
        }
    }
    let fin = |c: u8| c.is_ascii_digit() || matches!(c, b'A'..=b'P' | b'R'..=b'T' | b'Z' | b'c' | b'f'..=b'n' | b'q'..=b'u' | b'y' | b'=' | b'>' | b'<' | b'~');
    if b.get(i).is_some_and(|&c| fin(c)) {
        return Some(lead + i + 1);
    }
    // The pattern backtracks: a parameter's last digit can be the final byte.
    (i > start && b[i - 1].is_ascii_digit()).then_some(lead + i)
}

/// A shell's output as Desktop shows it: no ANSI escapes, `\n` line ends. Borrowed when there
/// is nothing to change.
pub fn clean(out: &str) -> Cow<'_, str> {
    let stripped = strip_ansi(out);
    if !stripped.contains('\r') {
        return stripped;
    }
    Cow::Owned(stripped.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `s` on one line, its runs of whitespace one space each, as a `nowrap` subtitle shows it.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The byte ranges of the http(s) URLs in `text`, without the sentence punctuation or closing
/// brackets after them (Desktop's `linkSegments`).
pub fn urls(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("http").map(|i| from + i) {
        let tail = &text[at..];
        let scheme = if tail.starts_with("https://") { 8 } else if tail.starts_with("http://") { 7 } else { 0 };
        if scheme == 0 {
            from = at + 4;
            continue;
        }
        let end = tail.find(|c: char| c.is_whitespace() || "<>\"'`".contains(c)).unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches(|c: char| "),.;:!?]".contains(c));
        if url.len() > scheme {
            out.push(at..at + url.len());
        }
        from = at + end.max(scheme);
    }
    out
}

/// `body` in `font` and `ink`, wrapping anywhere, its URLs underlined links that open in the
/// browser (`linked-text-link`: an underline at 40% of the text's colour, the base text colour
/// and a full underline under the pointer).
pub fn linked<S: TimelineHost>(p: &Pal, body: &str, font: rune::TextStyle, ink: rune::Color) -> El {
    let ranges = urls(body);
    let mut t = text(body.to_owned()).font(font).color(ink).wrap_anywhere();
    if ranges.is_empty() {
        return t;
    }
    let spans: Vec<Span> = ranges.iter().map(|r| Span::new(r.start, r.end).underline(ink.alpha(0.4))).collect();
    t = t.spans(spans);
    for r in &ranges {
        let url = &body[r.clone()];
        t = t.link(Link::new(r.clone(), url).hover(LinkPaint::new().color(p.text).underline(p.text)).help(url));
    }
    t.on_link(|s: &mut S, url: &str, tx: &mut Tx| {
        tx.instant();
        s.open_link(url, tx);
    })
}
