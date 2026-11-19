//! Session ids in a reply's Markdown as chips: `ses_` and its letters, written as plain text
//! or alone in inline code, read as the session's title in the session-mention chip's colours.
//! A press opens it, a right-click offers Copy Session ID, and resting on it says where it sits
//! and what it's doing. Code blocks, links and ids the host doesn't know stay as written.

use std::hash::{Hash, Hasher};

pub use opencode_kit::mention::{SessionRef, find_session_ids as find, plain_session_ids as plain_ids};
use rune::{LinkPaint, Span};

use crate::TimelineHost;
use crate::palette::Pal;
use crate::user_parts::{mention_neutral, session_link, session_of_link};

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
        lines.push(opencode_i18n::t!("rune.session.chip.active", ago = ago(now, info.updated)));
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

/// "5m ago"-style age from milliseconds, in the interface's language, as Desktop's short
/// relative times (`common.time.*.short`).
fn ago(now: i64, then: i64) -> String {
    let s = ((now - then) / 1000).max(0);
    match s {
        0..60 => opencode_i18n::t!("common.time.justNow").to_owned(),
        60..3600 => opencode_i18n::t!("common.time.minutesAgo.short", count = s / 60),
        3600..86_400 => opencode_i18n::t!("common.time.hoursAgo.short", count = s / 3600),
        _ => opencode_i18n::t!("common.time.daysAgo.short", count = s / 86_400),
    }
}
