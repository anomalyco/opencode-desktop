//! Where a user message's `@` mentions sit in its text: the `agents`, `skills` and `files`
//! the server keeps with each a `mention {start, end, text}` in UTF-16 units, as byte ranges.
//! Session mentions are `@session:<id>` in the text itself ([`session_tokens`]).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde_json::Value;

use crate::prompt::PathAttachment;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MentionKind {
    Agent,
    Skill,
    File,
    /// A file mention whose mime is `application/x-directory`.
    Directory,
    Session,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMention {
    /// Bytes of the message's text.
    pub range: Range<usize>,
    pub kind: MentionKind,
}

fn store() -> &'static Mutex<HashMap<String, Arc<[UserMention]>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Arc<[UserMention]>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Keeps only the user messages `keep` says are still held (`model::retain_user_messages`).
pub(crate) fn retain_user_messages(keep: &dyn Fn(&str) -> bool) {
    store().lock().unwrap_or_else(PoisonError::into_inner).retain(|id, _| keep(id));
    created_store().lock().unwrap_or_else(PoisonError::into_inner).retain(|id, _| keep(id));
    path_store().lock().unwrap_or_else(PoisonError::into_inner).retain(|id, _| keep(id));
}

/// The mentions on user message `id`, sorted and apart (empty when it has none).
pub fn user_mentions(id: &str) -> Arc<[UserMention]> {
    store().lock().unwrap_or_else(PoisonError::into_inner).get(id).cloned().unwrap_or_else(|| Arc::from([]))
}

/// Records the mentions on user message `id`, as a local echo of a sent prompt has them.
pub fn set_user_mentions(id: &str, mut mentions: Vec<UserMention>) {
    mentions.sort_by_key(|m| m.range.start);
    let mut last = 0;
    mentions.retain(|m| {
        let keep = m.range.start >= last && m.range.start < m.range.end;
        if keep {
            last = m.range.end;
        }
        keep
    });
    let mut store = store().lock().unwrap_or_else(PoisonError::into_inner);
    if mentions.is_empty() {
        store.remove(id);
    } else {
        store.insert(id.to_owned(), mentions.into());
    }
}

/// A UTF-16 offset into `text` as a byte offset, if it falls on a char boundary.
fn byte_at(text: &str, utf16: usize) -> Option<usize> {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units == utf16 {
            return Some(i);
        }
        if units > utf16 {
            return None;
        }
        units += c.len_utf16();
    }
    (units == utf16).then_some(text.len())
}

/// The mentions in a user message's (or inbox payload's) `agents`, `skills` and `files`, over
/// `text`. A mention whose offsets don't land on its text is dropped.
pub fn parse(text: &str, m: &Value) -> Vec<UserMention> {
    let list = |k: &str| m[k].as_array().cloned().unwrap_or_default();
    let kinds = [("agents", MentionKind::Agent), ("skills", MentionKind::Skill), ("files", MentionKind::File)];
    let mut out = Vec::new();
    for (key, kind) in kinds {
        for item in list(key) {
            let at = &item["mention"];
            let (Some(start), Some(end)) = (at["start"].as_u64(), at["end"].as_u64()) else { continue };
            let (Some(a), Some(b)) = (byte_at(text, start as usize), byte_at(text, end as usize)) else { continue };
            if a >= b || at["text"].as_str().is_some_and(|t| t != &text[a..b]) {
                continue;
            }
            let kind = if kind == MentionKind::File && item["mime"].as_str() == Some("application/x-directory") { MentionKind::Directory } else { kind };
            out.push(UserMention { range: a..b, kind });
        }
    }
    out
}

fn created_store() -> &'static Mutex<HashMap<String, i64>> {
    static STORE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// When user message `id` was created (`time.created`, in Unix milliseconds), if known.
pub fn user_created(id: &str) -> Option<i64> {
    created_store().lock().unwrap_or_else(PoisonError::into_inner).get(id).copied()
}

/// Records when user message `id` was created (`time.created`, in Unix milliseconds).
pub fn set_user_created(id: &str, created: i64) {
    if created > 0 {
        created_store().lock().unwrap_or_else(PoisonError::into_inner).insert(id.to_owned(), created);
    }
}

fn path_store() -> &'static Mutex<HashMap<String, Arc<[PathAttachment]>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Arc<[PathAttachment]>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Path-referenced attachments (`metadata.attachments`) on user message `id`.
pub fn user_path_attachments(id: &str) -> Arc<[PathAttachment]> {
    path_store().lock().unwrap_or_else(PoisonError::into_inner).get(id).cloned().unwrap_or_else(|| Arc::from([]))
}

/// Records path-referenced attachments (`metadata.attachments`) on user message `id`.
pub fn set_user_path_attachments(id: &str, items: Vec<PathAttachment>) {
    let mut store = path_store().lock().unwrap_or_else(PoisonError::into_inner);
    if store.get(id).map_or(items.is_empty(), |old| old.as_ref() == items.as_slice()) { return; }
    crate::model::extras_changed();
    if items.is_empty() {
        store.remove(id);
    } else {
        store.insert(id.to_owned(), items.into());
    }
}

/// Records `m`'s mentions, creation time and path attachments for message `id` when present; a
/// payload without them leaves what's recorded alone.
pub fn record(id: &str, text: &str, m: &Value) {
    if ["agents", "skills", "files"].iter().any(|k| m[k].is_array()) {
        set_user_mentions(id, parse(text, m));
    }
    if let Some(created) = m["time"]["created"].as_i64().or_else(|| m["time"]["created"].as_f64().map(|f| f as i64)) {
        set_user_created(id, created);
    }
    if let Some(items) = path_attachments(&m["metadata"]) {
        set_user_path_attachments(id, items);
    }
}

/// The files a prompt's `metadata.attachments` names by path (`PromptAttachmentReference`),
/// skipping malformed ones; None without the list.
pub fn path_attachments(metadata: &Value) -> Option<Vec<PathAttachment>> {
    let list = metadata["attachments"].as_array()?;
    Some(
        list.iter()
            .filter_map(|a| {
                let path = a["path"].as_str()?.to_owned();
                if path.is_empty() {
                    return None;
                }
                Some(PathAttachment {
                    name: a["name"].as_str().unwrap_or("").to_owned(),
                    mime: a["mime"].as_str().unwrap_or("text/plain").to_owned(),
                    path,
                })
            })
            .collect(),
    )
}

/// `@session:<id>` tokens in `text`: each token's bytes and the session id.
pub fn session_tokens(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("@session:ses_") {
        let start = from + i;
        let id_start = start + "@session:".len();
        let len = text[id_start + 4..].find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(text.len() - id_start - 4);
        let end = id_start + 4 + len;
        let boundary = text[..start].chars().next_back().is_none_or(char::is_whitespace);
        if len > 0 && boundary {
            out.push((start..end, &text[id_start..end]));
        }
        from = end.max(start + 1);
    }
    out
}

/// The kind of a composer pill by its id (`agent:`, `skill:`, `file:`, `reference:`, `session:`).
pub fn mention_kind_of(id: &str) -> Option<MentionKind> {
    Some(match id.split_once(':')?.0 {
        "agent" => MentionKind::Agent,
        "skill" => MentionKind::Skill,
        "file" => MentionKind::File,
        "reference" => MentionKind::Directory,
        "session" => MentionKind::Session,
        _ => return None,
    })
}

/// A session id short enough for a chip: `ses_` and its first eight characters.
pub fn short_session_id(id: &str) -> &str {
    let end = id.char_indices().nth(12).map_or(id.len(), |(i, _)| i);
    &id[..end]
}

/// The `on_link` id a session badge reports, and back.
pub fn session_link(id: &str) -> String {
    format!("session:{id}")
}

pub fn session_of_link(link: &str) -> Option<&str> {
    link.strip_prefix("session:")
}

/// The `on_link` id a plain/unknown session ID reports for its right-click reference menu.
pub fn session_raw_link(id: &str) -> String {
    format!("session-raw:{id}")
}

pub fn session_of_raw_link(link: &str) -> Option<&str> {
    link.strip_prefix("session-raw:")
}

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

/// How many letters follow `ses_` in a full session id: 26 from a server, fewer in fixtures.
const ID_BODY: std::ops::RangeInclusive<usize> = 20..=32;

/// Whether `c` may sit right before an id: anything but a word, a path, a URL or a mention.
fn opens_session_id(c: Option<char>) -> bool {
    c.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '/' | ':' | '@' | '-' | '.' | '=' | '#' | '\\')))
}

/// The full session id starting at byte `at` of `s`, when one does and nothing joins it on.
fn session_id_at(s: &str, at: usize) -> Option<Range<usize>> {
    let rest = s.get(at..)?.strip_prefix("ses_")?;
    let body = rest.bytes().take_while(u8::is_ascii_alphanumeric).count();
    let after = rest[body..].chars().next();
    (ID_BODY.contains(&body) && after.is_none_or(|c| c != '_' && c != '-' && c != '/') && opens_session_id(s[..at].chars().next_back())).then(|| at..at + 4 + body)
}

/// Every session id in Markdown `src` that becomes a chip: `(bytes to replace, id)`. A bare id
/// replaces itself; an inline code span holding only an id replaces the span, backticks and all.
/// Ids in fenced code, in a link's words or address, or joined to a path or a word stay.
pub fn find_session_ids(src: &str) -> Vec<(Range<usize>, &str)> {
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
        inline_session_ids(src, at, at + line.len(), &mut out);
    }
    out
}

/// [`find_session_ids`] within one line, `src[from..to]`, skipping code spans other than lone ids and links.
fn inline_session_ids<'a>(src: &'a str, from: usize, to: usize, out: &mut Vec<(Range<usize>, &'a str)>) {
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
                if brackets == 0 && session_id_at(lone, 0).is_some_and(|r| r.end == lone.len()) && opens_session_id(src[..i].chars().next_back()) {
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
            b's' if brackets == 0 => match session_id_at(src, i).filter(|r| r.end <= to) {
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

/// Every full session id in plain text `src` (a prompt), by [`find_session_ids`]'s rules for a bare id.
pub fn plain_session_ids(src: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find("ses_") {
        let at = from + i;
        match session_id_at(src, at) {
            Some(r) => {
                from = r.end;
                out.push((r.clone(), &src[r]));
            }
            None => from = at + 4,
        }
    }
    out
}

/// The recorded mentions still on an `@` in `body`, minus any over an `@session:` token, and
/// recorded session mentions once the text has tokens. Those come from a local echo of the
/// text as typed (a session pill reading `@WORK`), so on the server's copy their offsets can
/// land inside a token and cut it (p279); the token is the truth.
pub fn recorded_mentions<'a>(body: &'a str, mentions: &'a [UserMention]) -> impl Iterator<Item = &'a UserMention> + 'a {
    let tokens = session_tokens(body);
    mentions.iter().filter(move |m| {
        let stale = !tokens.is_empty() && m.kind == MentionKind::Session;
        let cuts = tokens.iter().any(|(r, _)| m.range.start < r.end && r.start < m.range.end);
        !stale && !cuts && body.get(m.range.clone()).is_some_and(|t| t.starts_with('@'))
    })
}

/// One badge in a bubble's shown text; `session` is the id a session mention opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BubbleMark {
    pub range: Range<usize>,
    pub kind: MentionKind,
    pub session: Option<String>,
    /// The session's title, when known.
    pub title: Option<String>,
}

type RawMark<'a> = (Range<usize>, MentionKind, Option<(&'a str, Option<String>)>);

/// A bubble's text and badges: the message's [`recorded_mentions`], and each `@session:<id>`
/// token as one badge reading the session's title, or its short id when `title` (which names a
/// session by id) doesn't know it.
pub fn bubble_marks(body: &str, mentions: &[UserMention], title: impl Fn(&str) -> Option<String>) -> (String, Vec<BubbleMark>) {
    let mut marks: Vec<RawMark<'_>> =
        recorded_mentions(body, mentions).map(|m| (m.range.clone(), m.kind, None)).collect();
    for (r, id) in session_tokens(body) {
        marks.push((r, MentionKind::Session, Some((id, title(id).filter(|t| !t.trim().is_empty())))));
    }
    // A session id pasted as it is reads as its badge too, once its title is known.
    for (r, id) in plain_session_ids(body) {
        if let Some(t) = title(id).filter(|t| !t.trim().is_empty()) {
            marks.push((r, MentionKind::Session, Some((id, Some(t)))));
        }
    }
    marks.sort_by_key(|(r, _, _)| r.start);
    let (mut out, mut ranges, mut at) = (String::with_capacity(body.len()), Vec::new(), 0);
    for (r, kind, session) in marks {
        if r.start < at {
            continue;
        }
        out.push_str(&body[at..r.start]);
        let start = out.len();
        match &session {
            Some((id, t)) => out.push_str(t.as_deref().unwrap_or_else(|| short_session_id(id))),
            None => out.push_str(&body[r.clone()]),
        }
        let (session, title) = session.map_or((None, None), |(id, t)| (Some(id.to_owned()), t));
        ranges.push(BubbleMark { range: start..out.len(), kind, session, title });
        at = r.end;
    }
    out.push_str(&body[at..]);
    (out, ranges)
}

/// [`bubble_marks`] as the shown text and each badge's bytes and kind.
pub fn mention_bubble(body: &str, mentions: &[UserMention], title: impl Fn(&str) -> Option<String>) -> (String, Vec<(Range<usize>, MentionKind)>) {
    let (text, marks) = bubble_marks(body, mentions, title);
    (text, marks.into_iter().map(|m| (m.range, m.kind)).collect())
}

