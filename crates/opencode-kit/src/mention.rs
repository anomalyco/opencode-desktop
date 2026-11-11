//! Where a user message's `@` mentions sit in its text: the `agents`, `skills` and `files`
//! the server keeps with each a `mention {start, end, text}` in UTF-16 units, as byte ranges.
//! Session mentions are `@session:<id>` in the text itself ([`session_tokens`]).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

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

/// The mentions on user message `id`, sorted and apart (empty when it has none).
pub fn user_mentions(id: &str) -> Arc<[UserMention]> {
    store().lock().unwrap().get(id).cloned().unwrap_or_else(|| Arc::from([]))
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
    let mut store = store().lock().unwrap();
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
    created_store().lock().unwrap().get(id).copied()
}

/// Records when user message `id` was created (`time.created`, in Unix milliseconds).
pub fn set_user_created(id: &str, created: i64) {
    if created > 0 {
        created_store().lock().unwrap().insert(id.to_owned(), created);
    }
}

fn path_store() -> &'static Mutex<HashMap<String, Arc<[PathAttachment]>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Arc<[PathAttachment]>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Path-referenced attachments (`metadata.attachments`) on user message `id`.
pub fn user_path_attachments(id: &str) -> Arc<[PathAttachment]> {
    path_store().lock().unwrap().get(id).cloned().unwrap_or_else(|| Arc::from([]))
}

/// Records path-referenced attachments (`metadata.attachments`) on user message `id`.
pub fn set_user_path_attachments(id: &str, items: Vec<PathAttachment>) {
    let mut store = path_store().lock().unwrap();
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

