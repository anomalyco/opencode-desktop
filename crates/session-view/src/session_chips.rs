//! Session ids in a reply's Markdown as chips: `ses_` and its letters, written as plain text
//! or alone in inline code, read as the session's title in the session-mention chip's colours.
//! A press opens it, a right-click offers Copy Session ID, and resting on it says where it sits
//! and what it's doing. Code blocks, links and ids the host doesn't know stay as written.

use std::hash::{Hash, Hasher};
use std::ops::Range;

use rune::{LinkPaint, Span};

use crate::TimelineHost;
use crate::palette::Pal;
use crate::user_parts::{mention_neutral, session_link, session_of_link, short_session_id};

/// How many letters follow `ses_` in a full session id: 26 from a server, fewer in fixtures.
const ID_BODY: std::ops::RangeInclusive<usize> = 20..=32;

/// What a session id in a reply stands for, as the host knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SessionRef {
    /// Its title; empty when the session has none yet.
    pub title: String,
    /// The title of the session that started it, for a subagent (`Some("")` when that one isn't loaded).
    pub parent: Option<String>,
    pub busy: bool,
    /// When it last changed, in milliseconds (0: unknown).
    pub updated: i64,
}

impl SessionRef {
    pub fn child(&self) -> bool {
        self.parent.is_some()
    }

    /// The chip's words: its title, or its short id while it has none.
    pub fn label<'a>(&'a self, id: &'a str) -> &'a str {
        if self.title.trim().is_empty() { short_session_id(id) } else { self.title.trim() }
    }
}

/// Whether `c` may sit right before an id: anything but a word, a path, a URL or a mention.
fn opens(c: Option<char>) -> bool {
    c.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '/' | ':' | '@' | '-' | '.' | '=' | '#' | '\\')))
}

/// The full session id starting at byte `at` of `s`, when one does and nothing joins it on.
fn id_at(s: &str, at: usize) -> Option<Range<usize>> {
    let rest = s.get(at..)?.strip_prefix("ses_")?;
    let body = rest.bytes().take_while(u8::is_ascii_alphanumeric).count();
    let after = rest[body..].chars().next();
    (ID_BODY.contains(&body) && after.is_none_or(|c| c != '_' && c != '-' && c != '/') && opens(s[..at].chars().next_back())).then(|| at..at + 4 + body)
}

/// Every session id in Markdown `src` that becomes a chip: `(bytes to replace, id)`. A bare id
/// replaces itself; an inline code span holding only an id replaces the span, backticks and all.
/// Ids in fenced code, in a link's words or address, or joined to a path or a word stay.
pub fn find(src: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    if !src.contains("ses_") {
        return out;
    }
    let mut fence: Option<(char, usize)> = None;
    let mut line_start = 0;
    for line in src.split_inclusive('\n') {
        let at = line_start;
        line_start += line.len();
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let run = |c: char| trimmed.chars().take_while(|&x| x == c).count();
        if let Some((c, n)) = fence {
            if indent < 4 && run(c) >= n && trimmed[run(c)..].trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if indent < 4 && (run('`') >= 3 || run('~') >= 3) {
            let c = if run('`') >= 3 { '`' } else { '~' };
            fence = Some((c, run(c)));
            continue;
        }
        inline(src, at, at + line.len(), &mut out);
    }
    out
}

/// [`find`] within one line, `src[from..to]`, skipping code spans other than lone ids and links.
fn inline<'a>(src: &'a str, from: usize, to: usize, out: &mut Vec<(Range<usize>, &'a str)>) {
    let b = src.as_bytes();
    let (mut i, mut brackets) = (from, 0usize);
    while i < to {
        match b[i] {
            b'\\' => i += 2,
            b'`' => {
                let n = b[i..to].iter().take_while(|&&c| c == b'`').count();
                let ticks = &src[i..i + n];
                let Some(close) = src[i + n..to].find(ticks).map(|c| i + n + c) else {
                    i += n;
                    continue;
                };
                let inner = &src[i + n..close];
                let lone = inner.trim();
                if brackets == 0 && id_at(lone, 0).is_some_and(|r| r.end == lone.len()) && opens(src[..i].chars().next_back()) {
                    let lead = i + n + inner.len() - inner.trim_start().len();
                    out.push((i..close + n, &src[lead..lead + lone.len()]));
                }
                i = close + n;
            }
            b'[' => {
                brackets += 1;
                i += 1;
            }
            b']' => {
                brackets = brackets.saturating_sub(1);
                i += 1;
                // A link's address: `](…)` up to its closing parenthesis.
                if b.get(i) == Some(&b'(') {
                    i = src[i..to].find(')').map_or(to, |c| i + c + 1);
                }
            }
            b'<' if src[i..to].starts_with("<http") => i = src[i..to].find('>').map_or(to, |c| i + c + 1),
            b's' if brackets == 0 => match id_at(src, i).filter(|r| r.end <= to) {
                Some(r) => {
                    out.push((r.clone(), &src[r.clone()]));
                    i = r.end;
                }
                None => i += 1,
            },
            _ => i += 1,
        }
    }
}

/// Every full session id in plain text `src` (a prompt), by [`find`]'s rules for a bare id.
pub fn plain_ids(src: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find("ses_") {
        let at = from + i;
        match id_at(src, at) {
            Some(r) => {
                from = r.end;
                out.push((r.clone(), &src[r]));
            }
            None => from = at + 4,
        }
    }
    out
}

/// `t` with Markdown's punctuation escaped, to sit inside a link's words.
fn escape(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    for c in t.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '~' | '|' | '!' | '(' | ')') {
            out.push('\\');
        }
        out.push(if c == '\n' { ' ' } else { c });
    }
    out
}

/// `src` with each id [`find`] finds and `known` knows as a link to `session:<id>` reading its
/// [`SessionRef::label`]; `None` when nothing changes.
pub fn rewrite(src: &str, known: impl Fn(&str) -> Option<SessionRef>) -> Option<String> {
    let found = find(src);
    let mut out = String::new();
    let mut at = 0;
    for (r, id) in found {
        let Some(info) = known(id) else { continue };
        out.push_str(&src[at..r.start]);
        out.push_str(&format!("[{}]({})", escape(info.label(id)), session_link(id)));
        at = r.end;
    }
    if at == 0 {
        return None;
    }
    out.push_str(&src[at..]);
    Some(out)
}

/// What the ids in `src` read as now, for a row's memo: it changes when one is looked up, renamed,
/// starts or stops, and each minute while any is known, so the hover's "5m ago" stays true.
pub fn version<S: TimelineHost>(s: &S, src: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut any = false;
    for (_, id) in find(src) {
        let r = s.session_ref(id);
        any |= r.is_some();
        (id, r).hash(&mut h);
    }
    if any {
        (s.now_ms() / 60_000).hash(&mut h);
    }
    h.finish()
}

/// Rewrites `src`'s ids for drawing, when the host knows them.
pub fn chips<S: TimelineHost>(s: &S, src: &str) -> Option<String> {
    rewrite(src, |id| s.session_ref(id))
}

/// A chip's resting look and its look under the pointer: the session mention's neutral chip.
pub fn look(p: &Pal, url: &str) -> Option<(Span, LinkPaint)> {
    session_of_link(url)?;
    let tint = mention_neutral(p.dark);
    let rest = Span::new(0, 0).color(p.text).bg(p.text.alpha(tint), 3.0);
    Some((rest, LinkPaint::new().color(p.text).bg(p.text.alpha(tint * 2.0 + 0.03))))
}

/// What resting on the chip for `id` says: its title, the session that started it, and whether
/// it runs or when it last did.
pub fn tip(info: &SessionRef, id: &str, now: i64) -> String {
    let mut lines = vec![info.label(id).to_owned()];
    match info.parent.as_deref() {
        Some("") => lines.push(opencode_i18n::t!("rune.session.chip.subagent").to_owned()),
        Some(parent) => lines.push(opencode_i18n::t!("rune.session.chip.subagentOf", parent = parent)),
        None => {}
    }
    if info.busy {
        lines.push(opencode_i18n::t!("rune.session.chip.running").to_owned());
    } else if info.updated > 0 {
        lines.push(opencode_i18n::t!("rune.session.chip.active", ago = opencode_kit::model::ago(now, info.updated)));
    }
    lines.push(id.to_owned());
    lines.join("\n")
}

/// The tooltips for the chips [`chips`] drew from `src`, by link.
pub fn tips<S: TimelineHost>(s: &S, src: &str) -> Vec<(String, String)> {
    find(src)
        .into_iter()
        .filter_map(|(_, id)| Some((session_link(id), tip(&s.session_ref(id)?, id, s.now_ms()))))
        .collect()
}

#[cfg(test)]
#[path = "session_chips_tests.rs"]
mod tests;
