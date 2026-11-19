//! What a prompt says about things other than its words, as Desktop sends it (`PromptComment`,
//! `composer/comment-note.ts`): text quoted from the conversation, and comments on lines of a
//! file. Each is lines in the prompt's `text` and an entry in `metadata.comments`; a file
//! comment also attaches its lines as a file.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde_json::Value;

/// One comment a prompt carries (Desktop's `PromptComment` less notes).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PromptComment {
    Quote(Quote),
    File(FileComment),
}

impl From<Quote> for PromptComment {
    fn from(q: Quote) -> Self {
        PromptComment::Quote(q)
    }
}

impl From<FileComment> for PromptComment {
    fn from(f: FileComment) -> Self {
        PromptComment::File(f)
    }
}

impl PromptComment {
    /// What the model reads (`formatQuoteComment`, `formatCommentNote`).
    pub fn prompt_text(&self) -> String {
        match self {
            PromptComment::Quote(q) => q.prompt_text(),
            PromptComment::File(f) => f.prompt_text(),
        }
    }

    /// The trimmed comment, when there's one.
    pub fn comment(&self) -> Option<&str> {
        match self {
            PromptComment::Quote(q) => q.comment(),
            PromptComment::File(f) => Some(f.comment.trim()).filter(|c| !c.is_empty()),
        }
    }

    pub fn json(&self) -> Value {
        match self {
            PromptComment::Quote(q) => q.json(),
            PromptComment::File(f) => f.json(),
        }
    }

    /// A `metadata.comments` entry (`decodePromptComment`): a quote, or a file comment; notes
    /// and anything else are skipped.
    pub fn parse(v: &Value) -> Option<PromptComment> {
        Quote::parse(v).map(PromptComment::Quote).or_else(|| FileComment::parse(v).map(PromptComment::File))
    }

    pub fn quote(&self) -> Option<&Quote> {
        match self {
            PromptComment::Quote(q) => Some(q),
            PromptComment::File(_) => None,
        }
    }

    pub fn quote_mut(&mut self) -> Option<&mut Quote> {
        match self {
            PromptComment::Quote(q) => Some(q),
            PromptComment::File(_) => None,
        }
    }

    pub fn file(&self) -> Option<&FileComment> {
        match self {
            PromptComment::File(f) => Some(f),
            PromptComment::Quote(_) => None,
        }
    }
}

/// Which view a file comment was written in (`commentOrigin`), whose plugin reveals it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentOrigin {
    Review,
    File,
}

/// A comment on lines of a file (Desktop's `PromptFileComment`, `comment-note.ts:8-15`): `path`
/// as the workspace names it, its 1-based lines, the comment, and the first two of the lines
/// (`previewSelectedLines`). `id` is the line comment it came from, which the composer's card
/// opens and which never goes on the wire.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FileComment {
    pub path: String,
    pub lines: Option<(u32, u32)>,
    pub comment: String,
    pub preview: Option<String>,
    pub origin: Option<CommentOrigin>,
    #[serde(default)]
    pub id: Option<String>,
}

impl FileComment {
    /// `formatCommentNote`: the lines in order, or "this file".
    pub fn prompt_text(&self) -> String {
        let range = match self.lines.map(|(a, b)| (a.min(b), a.max(b))) {
            None => "this file".to_owned(),
            Some((a, b)) if a == b => format!("line {a}"),
            Some((a, b)) => format!("lines {a} through {b}"),
        };
        format!("The user made the following comment regarding {range} of {}: {}", self.path, self.comment.trim())
    }

    /// Its `metadata.comments` entry, as Desktop's `request.ts` builds it (no `type`).
    pub fn json(&self) -> Value {
        let mut v = serde_json::json!({ "path": self.path });
        if let Some((a, b)) = self.lines {
            v["selection"] = serde_json::json!({ "startLine": a, "endLine": b, "startChar": 0, "endChar": 0 });
        }
        v["comment"] = self.comment.trim().into();
        if let Some(p) = &self.preview {
            v["preview"] = p.clone().into();
        }
        if let Some(o) = self.origin {
            v["origin"] = (if o == CommentOrigin::Review { "review" } else { "file" }).into();
        }
        v
    }

    pub fn parse(v: &Value) -> Option<FileComment> {
        if !v["type"].is_null() && v["type"] != "file" {
            return None;
        }
        let path = v["path"].as_str()?.to_owned();
        let comment = v["comment"].as_str()?.to_owned();
        let line = |k: &str| v["selection"][k].as_u64().and_then(|n| u32::try_from(n).ok());
        let lines = line("startLine").zip(line("endLine"));
        let origin = match v["origin"].as_str() {
            Some("review") => Some(CommentOrigin::Review),
            Some("file") => Some(CommentOrigin::File),
            _ => None,
        };
        Some(FileComment { path, lines, comment, preview: v["preview"].as_str().map(str::to_owned), origin, id: None })
    }

    /// The card's `:3` or `:3-5` (`CommentCard`).
    pub fn lines_label(&self) -> String {
        match self.lines {
            None => String::new(),
            Some((a, b)) if a == b => format!(":{a}"),
            Some((a, b)) => format!(":{a}-{b}"),
        }
    }
}

/// One quote (Desktop's `QuoteComment`), with the comment the user wrote about it, if any.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Quote {
    pub text: String,
    pub comment: Option<String>,
}

impl Quote {
    /// What the model reads (Desktop's `formatQuoteComment`). Every quoted line is prefixed,
    /// so quoted text can't end the quote early and read as the user's own words.
    pub fn prompt_text(&self) -> String {
        let lines: Vec<String> = self.text.split('\n').map(|l| format!("> {l}")).collect();
        let quote = format!("The user quoted the following from earlier in the conversation:\n{}", lines.join("\n"));
        match self.comment() {
            Some(c) => format!("{quote}\n\nThe user made the following comment regarding this quote: {c}"),
            None => quote,
        }
    }

    /// The trimmed comment, when there's one to send.
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref().map(str::trim).filter(|c| !c.is_empty())
    }

    /// Its `metadata.comments` entry: the comment travels only when the user wrote one.
    pub fn json(&self) -> Value {
        let mut v = serde_json::json!({ "type": "quote", "text": self.text });
        if let Some(c) = self.comment() {
            v["comment"] = c.into();
        }
        v
    }

    pub fn parse(v: &Value) -> Option<Quote> {
        if v["type"] != "quote" {
            return None;
        }
        let text = v["text"].as_str()?.to_owned();
        Some(Quote { text, comment: v["comment"].as_str().map(str::to_owned) })
    }
}

/// The paths a comment names with `@`, as written (Desktop's `parseCommentMentions`,
/// `composer/request.ts:67-77`): an `@` at the start or after a space or an opening `([{"'`,
/// up to the next space, less trailing punctuation. A session mention (`@session:ses_…`) isn't
/// a path.
pub fn comment_mentions(comment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    let mut at = 0;
    while let Some(c) = comment[at..].chars().next() {
        let next = at + c.len_utf8();
        let word = (c == '@' && prev.is_none_or(|p| p.is_whitespace() || "([{\"'".contains(p))).then(|| {
            let rest = &comment[next..];
            &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())]
        });
        let Some(word) = word.filter(|w| !w.is_empty()) else {
            prev = Some(c);
            at = next;
            continue;
        };
        let path = word.trim_end_matches(['.', ',', '!', '?', ';', ':', ')', '}', ']', '"', '\'']);
        if !path.is_empty() && !session_mention(path) {
            out.push(path.to_owned());
        }
        prev = word.chars().next_back();
        at = next + word.len();
    }
    out
}

/// What `parseSessionMention` takes `@<path>` for: `session:` and a whole generated session id.
fn session_mention(path: &str) -> bool {
    path.strip_prefix("session:ses_").is_some_and(|id| id.len() == 26 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// The paths the comments mention, in order (Desktop's `commentMentions`).
pub fn mentions(comments: &[PromptComment]) -> Vec<String> {
    comments.iter().filter_map(PromptComment::comment).flat_map(comment_mentions).collect()
}

/// The comments in a user message's `metadata`, read only beside a `displayText`, as Desktop's
/// `readPromptPresentation` does (prompts from other clients have no comments list).
pub fn parse_metadata(metadata: &Value) -> Option<(String, Vec<PromptComment>)> {
    let display = metadata["displayText"].as_str()?;
    let comments = metadata["comments"].as_array()?;
    Some((display.to_owned(), comments.iter().filter_map(PromptComment::parse).collect()))
}

/// User messages' comments, by message id, kept beside `Message::User` like its files.
fn store() -> &'static Mutex<HashMap<String, Arc<[PromptComment]>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Arc<[PromptComment]>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Keeps only the user messages `keep` says are still held (`model::retain_user_messages`).
pub(crate) fn retain_user_comments(keep: &dyn Fn(&str) -> bool) {
    store().lock().unwrap_or_else(PoisonError::into_inner).retain(|id, _| keep(id));
}

/// The comments on user message `id` (empty when it has none).
pub fn user_comments(id: &str) -> Arc<[PromptComment]> {
    store().lock().unwrap_or_else(PoisonError::into_inner).get(id).cloned().unwrap_or_else(|| Arc::from([]))
}

/// Records the comments on user message `id`, as a local echo of a sent prompt has them.
pub fn set_user_comments(id: &str, comments: Vec<PromptComment>) {
    let mut store = store().lock().unwrap_or_else(PoisonError::into_inner);
    if store.get(id).map_or(comments.is_empty(), |old| old.as_ref() == comments.as_slice()) { return; }
    crate::model::extras_changed();
    if comments.is_empty() {
        store.remove(id);
    } else {
        store.insert(id.to_owned(), comments.into());
    }
}

