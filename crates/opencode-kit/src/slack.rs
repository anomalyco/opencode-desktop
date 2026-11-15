//! Slack messages a prompt links to: what `GET /api/experimental/console/slack/message?url=`
//! says of a permalink. The server asks Console with its own Console sign-in, and Console reads
//! the message with the member's own Slack user token, so it answers only what that member could
//! read in Slack themselves; anything else is `unavailable`.

use std::ops::Range;

use serde::Deserialize;
use serde_json::Value;

use crate::client::{Server, encode_component};
use crate::wire_error::{WireError, WireKind};

/// A message the reader can read in Slack.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub permalink: String,
    pub channel: Channel,
    pub author: Author,
    /// Plain text, mentions and links resolved to readable text, at most 500 characters.
    pub text: String,
    /// Slack's own timestamp, seconds since the epoch: `1794780254.412659`.
    pub ts: String,
    /// The thread root's timestamp; `None` for a message outside a thread.
    #[serde(default)]
    pub thread_ts: Option<String>,
    /// Replies to the message's thread root (0 for none).
    #[serde(default)]
    pub reply_count: u64,
}

impl Message {
    /// When it was sent, in milliseconds since the epoch.
    pub fn sent_ms(&self) -> Option<i64> {
        let seconds = self.ts.split('.').next()?.parse::<i64>().ok()?;
        Some(seconds * 1000)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Channel {
    pub id: String,
    /// `None` for a direct or group message.
    #[serde(default)]
    pub name: Option<String>,
    pub kind: ChannelKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind {
    Public,
    Private,
    Dm,
    Group,
    External,
    /// A kind this client doesn't know yet.
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Author {
    pub name: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// Why a permalink has no preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The member hasn't signed in to Slack in Console.
    NoSlackConnection,
    /// The member can't read the channel.
    NoAccess,
    NotFound,
    InvalidUrl,
    /// The server isn't signed in to Console.
    NoConsole,
    /// A reason this client doesn't know yet.
    Other(String),
    /// No answer: the server doesn't have the route (an older one, or the feature's flag off),
    /// refused, or couldn't be reached.
    Failed(String),
}

impl Reason {
    fn named(reason: &str) -> Reason {
        match reason {
            "no_slack_connection" => Reason::NoSlackConnection,
            "no_access" => Reason::NoAccess,
            "not_found" => Reason::NotFound,
            "invalid_url" => Reason::InvalidUrl,
            "no_console" => Reason::NoConsole,
            other => Reason::Other(other.to_owned()),
        }
    }
}

/// What the server says of a permalink.
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    Found(Box<Message>),
    Unavailable(Reason),
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Body {
    Found { message: Message },
    Unavailable { reason: String },
}

/// The route's answer, `{ "status": "found", "message": … }` or `{ "status": "unavailable",
/// "reason": … }`; `None` for anything else.
pub fn lookup(v: &Value) -> Option<Lookup> {
    match Body::deserialize(v).ok()? {
        Body::Found { message } => Some(Lookup::Found(Box::new(message))),
        Body::Unavailable { reason } => Some(Lookup::Unavailable(Reason::named(&reason))),
    }
}

impl Server {
    /// What Slack permalink `url` points at, as the reader may see it
    /// (`GET /api/experimental/console/slack/message?url=`).
    pub fn slack_message(&self, url: &str) -> Result<Lookup, WireError> {
        let path = format!("/api/experimental/console/slack/message?url={}", encode_component(url));
        let v = self.get(&path)?;
        lookup(&v).ok_or_else(|| WireError::new("GET", &path, WireKind::Shape("not a Slack message lookup".into())))
    }
}

/// Each Slack message permalink in `text`, by its bytes: a run of characters from `https://` to
/// the next space or `<`, `>`, `"`, `'` (Desktop's `slackPermalinks`), less the sentence
/// punctuation that ends it.
pub fn permalinks(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("https://") {
        let start = from + at;
        let len = text[start..].find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'')).unwrap_or(text.len() - start);
        let url = text[start..start + len].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);
        if is_permalink(url) {
            out.push(start..start + url.len());
        }
        from = start + len;
    }
    out
}

/// Whether `url` is a Slack message permalink (`isSlackPermalink`):
/// `https://<team>.slack.com/archives/<C|D|G…>/p<16 digits>`, then any query or fragment.
pub fn is_permalink(url: &str) -> bool {
    let Some((host, path)) = url.strip_prefix("https://").and_then(|rest| rest.split_once('/')) else { return false };
    let team = host.strip_suffix(".slack.com").unwrap_or_default();
    if team.is_empty() || !team.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return false;
    }
    let Some((channel, message)) = path.strip_prefix("archives/").and_then(|rest| rest.split_once('/')) else { return false };
    let message = message.split(['?', '#']).next().unwrap_or_default();
    let channel_ok = channel.len() > 1 && matches!(channel.as_bytes()[0], b'C' | b'D' | b'G') && channel.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    channel_ok && message.strip_prefix('p').is_some_and(|ts| ts.len() == 16 && ts.bytes().all(|b| b.is_ascii_digit()))
}
