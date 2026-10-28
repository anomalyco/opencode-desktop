//! OpenCode's data as this app sees it: projects, sessions and transcripts, parsed leniently
//! from the server's JSON (`/api/project`, `/api/session`, `/api/session/:id/message`) and
//! kept current by the same events `/api/event` streams.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

mod compaction;
#[cfg(test)]
mod compaction_tests;

pub use compaction::{Compaction, CompactionStatus, background_compaction};

#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// The project's root folder.
    pub directory: String,
    pub active: i64,
    pub icon: ProjectIcon,
}

/// The project's `icon`: a discovered image (`url`, usually a data URL), a chosen one
/// (`override`) or a chosen avatar colour. Empty strings when unset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectIcon {
    pub url: String,
    pub override_url: String,
    pub color: String,
}

impl ProjectIcon {
    pub fn parse(v: &Value) -> ProjectIcon {
        ProjectIcon { url: s(&v["url"]), override_url: s(&v["override"]), color: s(&v["color"]) }
    }

    /// The image to show, as Desktop picks it (`getProjectAvatarSource`): a chosen image, else
    /// none when a colour is chosen, else the discovered one.
    pub fn source(&self) -> Option<&str> {
        if !self.override_url.is_empty() {
            return Some(&self.override_url);
        }
        if !self.color.is_empty() || self.url.is_empty() {
            return None;
        }
        Some(&self.url)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
}

impl Tokens {
    fn parse(v: &Value) -> Tokens {
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        Tokens {
            input: n(&v["input"]),
            output: n(&v["output"]),
            cache_read: n(&v["cache"]["read"]),
            cache_write: n(&v["cache"]["write"]),
            reasoning: n(&v["reasoning"]),
        }
    }
    /// Tokens in the context window for this step.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.output
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModelRef {
    pub id: String,
    pub provider: String,
    pub variant: String,
}

impl ModelRef {
    fn parse(v: &Value) -> ModelRef {
        ModelRef {
            id: s(&v["id"]),
            provider: s(&v["providerID"]),
            variant: s(&v["variant"]),
        }
    }

    /// "claude-opus-5-5" → "Claude Opus 5.5".
    pub fn display(&self) -> String {
        let mut out = Vec::new();
        let parts: Vec<&str> = self.id.split('-').collect();
        let mut i = 0;
        while i < parts.len() {
            let p = parts[i];
            if p.chars().all(|c| c.is_ascii_digit()) {
                let mut num = p.to_owned();
                while i + 1 < parts.len() && parts[i + 1].chars().all(|c| c.is_ascii_digit()) && parts[i + 1].len() <= 2 {
                    num.push('.');
                    num.push_str(parts[i + 1]);
                    i += 1;
                }
                out.push(num);
            } else if p.eq_ignore_ascii_case("gpt") {
                out.push("GPT".into());
            } else {
                out.push(capitalize(p));
            }
            i += 1;
        }
        if out.is_empty() { "Default model".into() } else { out.join(" ") }
    }
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
}

#[derive(Clone, Debug, Default)]
pub struct SessionInfo {
    pub id: String,
    pub parent: Option<String>,
    pub project: String,
    pub title: String,
    pub directory: String,
    pub updated: i64,
    pub model: ModelRef,
    pub cost: f64,
    /// When the session last went idle and when someone last viewed it (`time.idle`,
    /// `time.viewed`): unread while it went idle after the last view.
    pub idle: Option<i64>,
    pub viewed: Option<i64>,
    /// The session's agent, when it was created (`time.created`), and how its last run ended
    /// (`outcome`: "succeeded", "failed", "interrupted").
    pub agent: String,
    pub created: i64,
    pub outcome: Option<String>,
    /// The message a staged revert rewinds to (`revert.messageID`): it and everything after it is hidden.
    pub revert: Option<String>,
}

impl SessionInfo {
    pub fn unread(&self) -> bool {
        self.idle.is_some_and(|i| self.viewed.is_none_or(|v| i > v))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolStatus {
    Pending,
    Running,
    Completed,
    Error,
}

#[derive(Clone, Debug)]
pub enum Part {
    Text { text: String },
    Reasoning { text: String, started: i64, completed: Option<i64> },
    /// `files`: what the tool returned besides text, such as the image a `read` loaded.
    /// `metadata`: the tool's own state (a subagent call's `sessionID`).
    /// A failed call keeps its error there too: see [`tool_error`].
    Tool { id: String, name: String, status: ToolStatus, title: String, input: Value, files: Vec<Attachment>, metadata: Value },
}

/// Where a failed tool call's error message is kept in its `metadata`.
const TOOL_ERROR: &str = "opencode-kit.error";

/// A failed tool call's error message, from its `metadata`.
pub fn tool_error(metadata: &Value) -> Option<&str> {
    metadata[TOOL_ERROR].as_str()
}

/// Keeps `error`'s message (an `{ message }` object or a string) in `metadata`.
fn keep_tool_error(metadata: &mut Value, error: &Value) {
    let Some(message) = error["message"].as_str().or(error.as_str()) else { return };
    if !metadata.is_object() {
        *metadata = Value::Object(Default::default());
    }
    metadata[TOOL_ERROR] = message.into();
}

/// The child session a subagent call runs in, once the call names it.
pub fn subagent_session<'a>(input: &'a Value, metadata: &'a Value) -> Option<&'a str> {
    metadata["sessionID"].as_str().or_else(|| input["sessionID"].as_str()).filter(|s| !s.is_empty())
}

/// A file on a user message or a tool result. The server inlines its bytes, or, for a client
/// that opts in (see [`crate::client::MEDIA_CAPABILITY`]) on a session with
/// `experimental.media_blob` on, sends a `media:<hash>` ref: then `media` is the hash, `bytes`
/// is empty, and `GET /api/experimental/media/:hash` has the bytes.
#[derive(Clone, Default, PartialEq)]
pub struct Attachment {
    pub mime: String,
    pub name: String,
    pub bytes: Arc<[u8]>,
    pub media: Option<String>,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.media {
            Some(hash) => write!(f, "Attachment({}, {}, media:{hash})", self.mime, self.name),
            None => write!(f, "Attachment({}, {}, {} bytes)", self.mime, self.name, self.bytes.len()),
        }
    }
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    /// The server's `[media unavailable: <name> (<mime>)]`, which stands in for a ref whose
    /// stored bytes are gone.
    pub fn unavailable(&self) -> Option<&str> {
        if self.mime != "text/plain" || self.media.is_some() {
            return None;
        }
        std::str::from_utf8(&self.bytes).ok().filter(|t| t.starts_with("[media unavailable: ") && t.ends_with(']'))
    }
}

/// The blob hash in a `media:<hash>` ref (64 lowercase hex digits), the only form the server
/// sends stored media in.
pub fn media_hash(uri: &str) -> Option<&str> {
    uri.strip_prefix("media:").filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Standard base64, padding optional; None on anything else.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.chunks(4) {
        let mut n = 0u32;
        for (k, &c) in chunk.iter().enumerate() {
            n |= (val(c)? as u32) << (18 - 6 * k);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        match chunk.len() {
            4 => out.extend_from_slice(&bytes),
            3 => out.extend_from_slice(&bytes[..2]),
            2 => out.push(bytes[0]),
            _ => return None,
        }
    }
    Some(out)
}

/// One file: `{data, mime, name}` (base64), `{uri: "data:…;base64,…", mime, name}` or a
/// `{uri: "media:<hash>", mime, name, bytes}` ref. Other URIs (`file://`) are skipped.
pub fn parse_attachment(f: &Value) -> Option<Attachment> {
    if let Some(hash) = f["uri"].as_str().and_then(media_hash) {
        return Some(Attachment { mime: s(&f["mime"]), name: s(&f["name"]), media: Some(hash.to_owned()), ..Default::default() });
    }
    let (mime, bytes) = match (f["data"].as_str(), f["uri"].as_str()) {
        (Some(data), _) => (s(&f["mime"]), base64_decode(data)?),
        (None, Some(uri)) => {
            let (head, data) = uri.strip_prefix("data:")?.split_once(',')?;
            let head_mime = head.split(';').next().unwrap_or("");
            if !head.ends_with(";base64") {
                return None;
            }
            let mime = f["mime"].as_str().unwrap_or(head_mime).to_owned();
            (mime, base64_decode(data)?)
        }
        _ => return None,
    };
    Some(Attachment { mime, name: s(&f["name"]), bytes: bytes.into(), media: None })
}

fn parse_attachments(list: &Value) -> Vec<Attachment> {
    list.as_array().into_iter().flatten().filter(|c| c["type"].as_str().is_none_or(|t| t == "file")).filter_map(parse_attachment).collect()
}

/// User messages' files, by message id. `Message::User` has no field for them, so apps that
/// build one as a literal keep compiling; `parse_message` fills this and apps read it.
fn user_file_store() -> &'static Mutex<HashMap<String, Arc<[Attachment]>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Arc<[Attachment]>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// The files on user message `id` (empty when it has none).
pub fn user_files(id: &str) -> Arc<[Attachment]> {
    user_file_store().lock().unwrap().get(id).cloned().unwrap_or_else(|| Arc::from([]))
}

/// Records the files on user message `id`, as a local echo of a sent prompt has them.
pub fn set_user_files(id: &str, files: Vec<Attachment>) {
    let mut store = user_file_store().lock().unwrap();
    // A cached page and its refresh describe the same immutable prompt. Preserve the bytes'
    // identity so clients can keep already-decoded images while the refresh is parsed.
    if store.get(id).is_some_and(|old| old.as_ref() == files.as_slice()) { return; }
    if files.is_empty() {
        store.remove(id);
    } else {
        store.insert(id.to_owned(), files.into());
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    User { id: String, text: String },
    Assistant(Assistant),
    /// Shell commands and notices: shown as a quiet one-line row.
    Note { id: String, text: String },
    Compaction(Compaction),
}

#[derive(Clone, Debug, Default)]
pub struct Assistant {
    pub id: String,
    pub completed: Option<i64>,
    pub parts: Vec<Part>,
    pub cost: f64,
    pub tokens: Tokens,
    pub error: Option<String>,
}

impl Message {
    pub fn id(&self) -> &str {
        match self {
            Message::User { id, .. } | Message::Note { id, .. } => id,
            Message::Assistant(a) => &a.id,
            Message::Compaction(c) => &c.id,
        }
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_owned()
}

fn i(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(0)
}

pub fn parse_projects(v: &Value) -> Vec<Project> {
    let list = v.as_array().or_else(|| v["data"].as_array());
    list.into_iter()
        .flatten()
        .filter_map(|p| {
            let dir = s(&p["canonical"]);
            if dir.is_empty() {
                return None;
            }
            let name = dir.rsplit('/').next().unwrap_or(&dir).to_owned();
            Some(Project {
                id: s(&p["id"]),
                name,
                directory: dir.clone(),
                active: i(&p["time"]["active"]).max(i(&p["time"]["updated"])),
                icon: ProjectIcon::parse(&p["icon"]),
            })
        })
        .collect()
}

pub fn parse_session(p: &Value) -> SessionInfo {
    SessionInfo {
        id: s(&p["id"]),
        parent: p["parentID"].as_str().map(str::to_owned),
        project: s(&p["projectID"]),
        title: s(&p["title"]),
        directory: s(&p["location"]["directory"]),
        updated: i(&p["time"]["updated"]),
        model: ModelRef::parse(&p["model"]),
        cost: p["cost"].as_f64().unwrap_or(0.0),
        idle: p["time"]["idle"].as_i64(),
        viewed: p["time"]["viewed"].as_i64(),
        agent: s(&p["agent"]),
        created: i(&p["time"]["created"]),
        outcome: p["outcome"].as_str().map(str::to_owned),
        revert: p["revert"]["messageID"].as_str().map(str::to_owned),
    }
}

pub fn parse_sessions(v: &Value) -> Vec<SessionInfo> {
    v["data"].as_array().or_else(|| v.as_array()).into_iter().flatten().map(parse_session).collect()
}

fn tool_title(name: &str, input: &Value) -> String {
    let get = |k: &str| input[k].as_str().map(str::to_owned);
    let short = |p: String| {
        let parts: Vec<&str> = p.split('/').collect();
        if parts.len() > 3 { parts[parts.len() - 2..].join("/") } else { p }
    };
    let t = match name {
        "shell" | "bash" => get("description").or_else(|| get("command")),
        "read" | "write" | "edit" => get("filePath").or_else(|| get("path")).map(short),
        "grep" | "glob" => get("pattern"),
        "task" | "subagent" => get("description").or_else(|| get("prompt")),
        "webfetch" => get("url"),
        "skill" => get("name").or_else(|| get("id")),
        _ => None,
    };
    let t = t.unwrap_or_default();
    let line = t.lines().next().unwrap_or("").trim().to_owned();
    if line.chars().count() > 120 { line.chars().take(119).collect::<String>() + "…" } else { line }
}

fn tool_status(v: &Value) -> ToolStatus {
    match v.as_str().unwrap_or("") {
        "completed" => ToolStatus::Completed,
        "error" | "failed" => ToolStatus::Error,
        "running" => ToolStatus::Running,
        _ => ToolStatus::Pending,
    }
}

fn parse_part(c: &Value) -> Option<Part> {
    Some(match c["type"].as_str()? {
        "text" => Part::Text { text: s(&c["text"]) },
        "reasoning" => Part::Reasoning {
            text: s(&c["text"]),
            started: i(&c["time"]["created"]),
            completed: c["time"]["completed"].as_i64(),
        },
        "tool" => {
            let name = s(&c["name"]);
            let input = c["state"]["input"].clone();
            let mut metadata = c["state"]["metadata"].clone();
            keep_tool_error(&mut metadata, &c["state"]["error"]);
            Part::Tool {
                id: s(&c["id"]),
                title: tool_title(&name, &input),
                name,
                status: tool_status(&c["state"]["status"]),
                input,
                files: parse_attachments(&c["state"]["content"]),
                metadata,
            }
        }
        _ => return None,
    })
}

pub fn parse_message(m: &Value) -> Option<Message> {
    let id = s(&m["id"]);
    Some(match m["type"].as_str()? {
        "user" => {
            let files = parse_attachments(&m["files"]);
            if !files.is_empty() {
                set_user_files(&id, files);
            }
            let text = s(&m["text"]);
            crate::mention::record(&id, &text, m);
            Message::User { id, text }
        }
        "assistant" => Message::Assistant(Assistant {
            id,
            completed: m["time"]["completed"].as_i64(),
            parts: m["content"].as_array().into_iter().flatten().filter_map(parse_part).collect(),
            cost: m["cost"].as_f64().unwrap_or(0.0),
            tokens: Tokens::parse(&m["tokens"]),
            error: m["error"]["message"].as_str().map(str::to_owned),
        }),
        "shell" => Message::Note { id, text: format!("$ {}", s(&m["command"])) },
        "compaction" => Message::Compaction(Compaction::parse(m)),
        _ => return None,
    })
}

/// Messages in chronological order from one `desc` page.
pub fn parse_messages(v: &Value) -> Vec<Message> {
    let mut list: Vec<&Value> = v["data"].as_array().into_iter().flatten().collect();
    // By `time.created`, then id. A steered prompt keeps the id it was sent with but takes its
    // delivery time, so it sorts after the work it waited behind, as the server orders it.
    list.sort_by(|a, b| (i(&a["time"]["created"]), a["id"].as_str()).cmp(&(i(&b["time"]["created"]), b["id"].as_str())));
    list.into_iter().filter_map(parse_message).collect()
}

/// How the server delivers a prompt sent while the session is busy: `Steer` joins the running
/// turn at its next step boundary; `Queue` waits for the turn (and every steer) to finish. The
/// server steers when a prompt names neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delivery {
    #[default]
    Steer,
    Queue,
}

impl Delivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Delivery::Steer => "steer",
            Delivery::Queue => "queue",
        }
    }

    pub fn parse(v: &Value) -> Option<Delivery> {
        match v.as_str()? {
            "steer" => Some(Delivery::Steer),
            "queue" => Some(Delivery::Queue),
            _ => None,
        }
    }
}

/// A prompt the server admitted but hasn't delivered yet (`GET /api/session/:id/inbox`). Its id
/// becomes the user message's id once delivered.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxItem {
    pub id: String,
    pub text: String,
    pub delivery: Delivery,
    pub files: Vec<Attachment>,
}

/// One user item, from the inbox list (`{id, type, payload, delivery}`) or a
/// `session.inbox.enqueued` event (`{inboxID, item: {type, payload, delivery}}`). Synthetic,
/// compaction and move items are None.
pub fn parse_inbox_item(v: &Value) -> Option<InboxItem> {
    let (id, item) = if v["item"].is_object() { (&v["inboxID"], &v["item"]) } else { (&v["id"], v) };
    if item["type"] != "user" {
        return None;
    }
    let payload = &item["payload"];
    let text = payload["metadata"]["displayText"].as_str().filter(|t| !t.is_empty()).map_or_else(|| s(&payload["text"]), str::to_owned);
    Some(InboxItem { id: s(id), text, delivery: Delivery::parse(&item["delivery"])?, files: parse_attachments(&payload["files"]) })
}

/// What [`Inbox::apply`] did.
#[derive(Debug, PartialEq)]
pub enum InboxChange {
    None,
    Changed,
    /// The item left the inbox for the transcript.
    Delivered(InboxItem),
}

/// One session's undelivered prompts, in admission order, kept current from
/// `session.inbox.*` events.
#[derive(Clone, Debug, Default)]
pub struct Inbox {
    pub items: Vec<InboxItem>,
    /// The id of a compaction the server admitted but hasn't started.
    pub compaction: Option<String>,
}

impl Inbox {
    pub fn queued(&self) -> impl Iterator<Item = &InboxItem> {
        self.items.iter().filter(|i| i.delivery == Delivery::Queue)
    }

    pub fn steering(&self) -> impl Iterator<Item = &InboxItem> {
        self.items.iter().filter(|i| i.delivery == Delivery::Steer)
    }

    /// Adds `item`, or replaces the one with its id in place.
    pub fn upsert(&mut self, item: InboxItem) {
        match self.items.iter_mut().find(|i| i.id == item.id) {
            Some(slot) => *slot = item,
            None => self.items.push(item),
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<InboxItem> {
        let at = self.items.iter().position(|i| i.id == id)?;
        Some(self.items.remove(at))
    }

    pub fn set_delivery(&mut self, id: &str, delivery: Delivery) -> bool {
        self.items.iter_mut().find(|i| i.id == id).is_some_and(|i| std::mem::replace(&mut i.delivery, delivery) != delivery)
    }

    /// Applies one `session.inbox.*` event, or `session.inbox.listed` (see
    /// [`crate::client::Request::Inbox`]), which replaces the list.
    pub fn apply(&mut self, ty: &str, d: &Value) -> InboxChange {
        let id = d["inboxID"].as_str().unwrap_or("");
        let compaction = match ty {
            "session.inbox.listed" => {
                d["items"].as_array().into_iter().flatten().find(|i| i["type"] == "compaction").map(|i| s(&i["id"]))
            }
            "session.inbox.enqueued" if d["item"]["type"] == "compaction" => Some(id.to_owned()),
            "session.inbox.cancelled" | "session.inbox.delivered" if self.compaction.as_deref() == Some(id) => None,
            _ => self.compaction.clone(),
        };
        let moved = compaction != self.compaction;
        self.compaction = compaction;
        let changed = match ty {
            "session.inbox.listed" => {
                let items: Vec<InboxItem> = d["items"].as_array().into_iter().flatten().filter_map(parse_inbox_item).collect();
                let same = items == self.items;
                self.items = items;
                !same
            }
            "session.inbox.enqueued" => match parse_inbox_item(d) {
                Some(item) => {
                    let same = self.items.iter().any(|i| *i == item);
                    self.upsert(item);
                    !same
                }
                None => false,
            },
            "session.inbox.delivery.changed" => Delivery::parse(&d["delivery"]).is_some_and(|to| self.set_delivery(id, to)),
            "session.inbox.cancelled" => self.remove(id).is_some(),
            "session.inbox.delivered" => match self.remove(id) {
                Some(item) => return InboxChange::Delivered(item),
                None => false,
            },
            _ => false,
        } || moved;
        if changed { InboxChange::Changed } else { InboxChange::None }
    }
}

/// One open transcript, kept current from events.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub session: String,
    pub messages: Vec<Message>,
}

impl Transcript {
    fn assistant(&mut self, id: &str) -> Option<&mut Assistant> {
        self.messages.iter_mut().rev().find_map(|m| match m {
            Message::Assistant(a) if a.id == id => Some(a),
            _ => None,
        })
    }

    /// The assistant step still producing output, if any.
    pub fn busy(&self) -> bool {
        matches!(self.messages.last(), Some(Message::Assistant(a)) if a.completed.is_none() && a.error.is_none())
            || matches!(self.messages.last(), Some(Message::User { .. }))
    }

    pub fn merge(&mut self, fresh: Vec<Message>) {
        let mut prev: Option<String> = None;
        for m in fresh {
            let id = m.id().to_owned();
            let after = prev.replace(id).and_then(|p| self.messages.iter().position(|o| o.id() == p));
            if let Some(slot) = self.messages.iter_mut().find(|o| o.id() == m.id()) {
                // Keep live-streamed text when the fetched copy is behind.
                if let (Message::Assistant(old), Message::Assistant(new)) = (&*slot, &m)
                    && new.completed.is_none()
                    && old.parts.len() > new.parts.len()
                {
                    continue;
                }
                *slot = m;
            } else {
                // After the page's previous message, so a steered prompt stays where the server put it.
                let at = after.map_or_else(|| self.messages.partition_point(|o| o.id() < m.id()), |a| a + 1);
                self.messages.insert(at, m);
            }
        }
    }

    /// How many messages show under a staged revert at `boundary`: those before it (ids sort by time).
    pub fn visible_len(&self, boundary: Option<&str>) -> usize {
        boundary.map_or(self.messages.len(), |b| self.messages.partition_point(|m| m.id() < b))
    }

    /// Applies one `/api/event` payload for this session; returns whether anything changed.
    pub fn apply(&mut self, ty: &str, d: &Value, now: i64) -> bool {
        if d["sessionID"].as_str() != Some(self.session.as_str()) {
            return false;
        }
        let mid = s(&d["assistantMessageID"]);
        let ordinal = d["ordinal"].as_u64().map(|o| o as usize);
        match ty {
            "session.prompted" | "session.user.message" => {
                let id = s(&d["messageID"]);
                if !id.is_empty() && !self.messages.iter().any(|m| m.id() == id) {
                    let text = s(&d["text"]);
                    crate::mention::record(&id, &text, d);
                    self.messages.push(Message::User { id, text });
                }
            }
            "session.step.started" => {
                if self.assistant(&mid).is_none() {
                    self.messages.push(Message::Assistant(Assistant { id: mid, ..Default::default() }));
                }
            }
            "session.text.started" | "session.reasoning.started" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let part = if ty == "session.text.started" {
                    Part::Text { text: String::new() }
                } else {
                    Part::Reasoning { text: String::new(), started: now, completed: None }
                };
                place(&mut a.parts, ordinal, part);
            }
            "session.text.delta" | "session.reasoning.delta" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let delta = d["delta"].as_str().unwrap_or("");
                let want_text = ty == "session.text.delta";
                let at = ordinal.filter(|&o| o < a.parts.len()).or(a.parts.len().checked_sub(1));
                match at.map(|k| &mut a.parts[k]) {
                    Some(Part::Text { text }) if want_text => text.push_str(delta),
                    Some(Part::Reasoning { text, .. }) if !want_text => text.push_str(delta),
                    _ => {
                        let part = if want_text {
                            Part::Text { text: delta.to_owned() }
                        } else {
                            Part::Reasoning { text: delta.to_owned(), started: now, completed: None }
                        };
                        place(&mut a.parts, ordinal, part);
                    }
                }
            }
            "session.text.ended" | "session.reasoning.ended" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let full = d["text"].as_str().map(str::to_owned);
                match ordinal.and_then(|o| a.parts.get_mut(o)) {
                    Some(Part::Text { text }) => {
                        if let Some(f) = full {
                            *text = f;
                        }
                    }
                    Some(Part::Reasoning { text, completed, .. }) => {
                        if let Some(f) = full {
                            *text = f;
                        }
                        *completed = Some(now);
                    }
                    _ => return false,
                }
            }
            "session.tool.input.started" | "session.tool.called" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let id = s(&d["id"]);
                let input = d["input"].clone();
                let found = a.parts.iter_mut().find_map(|p| match p {
                    Part::Tool { id: tid, name, status, title, input: inp, .. } if *tid == id => {
                        Some((name, status, title, inp))
                    }
                    _ => None,
                });
                match found {
                    Some((name, status, title, inp)) => {
                        if ty == "session.tool.called" {
                            *status = ToolStatus::Running;
                            *title = tool_title(name, &input);
                            *inp = input;
                        }
                    }
                    None => {
                        let name = s(&d["name"]);
                        a.parts.push(Part::Tool {
                            id,
                            title: tool_title(&name, &input),
                            name,
                            status: if ty == "session.tool.called" { ToolStatus::Running } else { ToolStatus::Pending },
                            input,
                            files: Vec::new(),
                            metadata: Value::Null,
                        });
                    }
                }
            }
            "session.tool.progress" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let id = s(&d["id"]);
                let mut hit = false;
                for p in &mut a.parts {
                    if let Part::Tool { id: tid, metadata, .. } = p
                        && *tid == id
                        && d["metadata"].is_object()
                    {
                        *metadata = d["metadata"].clone();
                        hit = true;
                    }
                }
                return hit;
            }
            "session.tool.success" | "session.tool.failed" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let id = s(&d["id"]);
                for p in &mut a.parts {
                    if let Part::Tool { id: tid, status, files, metadata, .. } = p
                        && *tid == id
                    {
                        *status = if ty == "session.tool.success" { ToolStatus::Completed } else { ToolStatus::Error };
                        if d["metadata"].is_object() {
                            *metadata = d["metadata"].clone();
                        }
                        if ty == "session.tool.failed" {
                            keep_tool_error(metadata, &d["error"]);
                        }
                        let got = parse_attachments(&d["content"]);
                        if !got.is_empty() {
                            *files = got;
                        }
                    }
                }
            }
            "session.step.ended" | "session.step.failed" => {
                let Some(a) = self.assistant(&mid) else { return false };
                a.completed = Some(now);
                a.cost = d["cost"].as_f64().unwrap_or(a.cost);
                if d["tokens"].is_object() {
                    a.tokens = Tokens::parse(&d["tokens"]);
                }
                if ty == "session.step.failed" {
                    a.error = Some(d["error"]["message"].as_str().unwrap_or("The step failed").to_owned());
                }
            }
            "session.compaction.started" | "session.compaction.delta" | "session.compaction.ended" | "session.compaction.failed" => {
                return compaction::apply(&mut self.messages, ty, d, now);
            }
            "session.revert.committed" => {
                let at = self.visible_len(d["to"].as_str());
                if at == self.messages.len() {
                    return false;
                }
                self.messages.truncate(at);
            }
            _ => return false,
        }
        true
    }
}

fn place(parts: &mut Vec<Part>, ordinal: Option<usize>, part: Part) {
    match ordinal {
        Some(o) if o < parts.len() => parts[o] = part,
        _ => parts.push(part),
    }
}

/// Something a session is waiting on a person for.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    /// A tool wants to run: `action` on `resources`.
    Permission { id: String, session: String, action: String, resources: Vec<String>, message: String },
    /// A form, such as the question tool's: one field per question.
    Form { id: String, session: String, title: String, fields: Vec<Field> },
}

impl Pending {
    pub fn id(&self) -> &str {
        match self {
            Pending::Permission { id, .. } | Pending::Form { id, .. } => id,
        }
    }
    pub fn session(&self) -> &str {
        match self {
            Pending::Permission { session, .. } | Pending::Form { session, .. } => session,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    /// Free text, or one of `options` (plus free text when `custom`).
    Text,
    Number,
    Boolean,
    /// Any number of `options`.
    Multi,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub title: String,
    pub description: String,
    pub kind: FieldKind,
    pub options: Vec<FieldOption>,
    pub custom: bool,
    pub required: bool,
}

/// `permission.asked`'s payload or one entry of `GET /session/:id/permission`.
pub fn parse_permission(v: &Value) -> Option<Pending> {
    let id = s(&v["id"]);
    if id.is_empty() {
        return None;
    }
    Some(Pending::Permission {
        id,
        session: s(&v["sessionID"]),
        action: s(&v["action"]),
        resources: v["resources"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(str::to_owned)).collect(),
        message: s(&v["message"]),
    })
}

/// `form.created`'s `form` or one entry of `GET /session/:id/form`.
pub fn parse_form(v: &Value) -> Option<Pending> {
    let v = if v["form"].is_object() { &v["form"] } else { v };
    let id = s(&v["id"]);
    if id.is_empty() || v["state"]["status"].as_str().is_some_and(|st| st != "pending") {
        return None;
    }
    let fields = v["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["hidden"].as_bool() != Some(true) && f["type"] != "external")
        .map(|f| Field {
            key: s(&f["key"]),
            title: s(&f["title"]),
            description: s(&f["description"]),
            kind: match f["type"].as_str().unwrap_or("") {
                "boolean" => FieldKind::Boolean,
                "number" | "integer" => FieldKind::Number,
                "multiselect" => FieldKind::Multi,
                _ => FieldKind::Text,
            },
            options: f["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| FieldOption { value: s(&o["value"]), label: s(&o["label"]), description: s(&o["description"]) })
                .collect(),
            custom: f["custom"].as_bool().unwrap_or(false),
            required: f["required"].as_bool().unwrap_or(false),
        })
        .collect();
    Some(Pending::Form { id, session: s(&v["sessionID"]), title: s(&v["title"]), fields })
}

/// "3m ago"-style age from milliseconds, as Desktop's short relative times.
pub fn ago(now: i64, then: i64) -> String {
    let s = ((now - then) / 1000).max(0);
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

pub fn tokens_short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names() {
        let m = |id: &str| ModelRef { id: id.into(), ..Default::default() }.display();
        assert_eq!(m("claude-opus-5-5"), "Claude Opus 5.5");
        assert_eq!(m("gpt-6-astra"), "GPT 6 Astra");
        assert_eq!(m("gemini-4-argon"), "Gemini 4 Argon");
    }

    #[test]
    fn parses_the_question_form() {
        let ev = serde_json::json!({"form": {"id": "frm_1", "sessionID": "ses_1", "title": "Questions", "fields": [
            {"key": "q0", "title": "Base", "description": "Which branch?", "type": "string", "custom": true,
             "options": [{"value": "dev", "label": "dev", "description": "default"}]},
            {"key": "q1", "type": "multiselect", "options": [{"value": "a", "label": "A"}]},
            {"key": "hidden", "type": "string", "hidden": true}
        ]}});
        let Some(Pending::Form { id, session, fields, .. }) = parse_form(&ev) else { panic!() };
        assert_eq!((id.as_str(), session.as_str(), fields.len()), ("frm_1", "ses_1", 2));
        assert_eq!(fields[0].kind, FieldKind::Text);
        assert!(fields[0].custom);
        assert_eq!(fields[0].options[0].description, "default");
        assert_eq!(fields[1].kind, FieldKind::Multi);
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(base64_decode("b3BlbmNvZGU6cHc=").unwrap(), b"opencode:pw");
        assert_eq!(base64_decode("YWI=").unwrap(), b"ab");
        assert_eq!(base64_decode("YWJj").unwrap(), b"abc");
        assert!(base64_decode("a$==").is_none());
    }

    #[test]
    fn parses_user_and_tool_images() {
        // As `GET /api/session/:id/message` returns them.
        let v = serde_json::json!({"data": [
            {"id": "msg_u1", "type": "user", "text": "look", "files": [
                {"data": "iVBORw==", "mime": "image/png", "source": {"type": "inline"}, "name": "shot.png"}
            ]},
            {"id": "msg_a1", "type": "assistant", "content": [
                {"type": "tool", "id": "t1", "name": "read", "state": {"status": "completed", "input": {"path": "/x.jpg"},
                 "content": [{"type": "text", "text": "Image read successfully"},
                             {"type": "file", "uri": "data:image/jpeg;base64,/9j/", "mime": "image/jpeg", "name": "/x.jpg"}]}}
            ]}
        ]});
        let msgs = parse_messages(&v);
        let files = user_files("msg_u1");
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].mime.as_str(), files[0].name.as_str(), &files[0].bytes[..]), ("image/png", "shot.png", &[0x89, b'P', b'N', b'G'][..]));
        let Some(Message::Assistant(a)) = msgs.iter().find(|m| matches!(m, Message::Assistant(_))) else { panic!() };
        let Part::Tool { files, .. } = &a.parts[0] else { panic!() };
        assert_eq!((files.len(), files[0].mime.as_str(), &files[0].bytes[..]), (1, "image/jpeg", &[0xff, 0xd8, 0xff][..]));
        assert!(user_files("msg_none").is_empty());
    }

    #[test]
    fn identical_user_file_refresh_preserves_byte_identity() {
        let mut value = serde_json::json!({"data":[{"id":"msg_file_refresh", "type":"user", "files":[{"mime":"image/png", "name":"shot.png", "uri":"data:image/png;base64,iVBORw=="}]}]});
        parse_messages(&value);
        let original = user_files("msg_file_refresh")[0].bytes.clone();
        parse_messages(&value);
        assert!(Arc::ptr_eq(&original, &user_files("msg_file_refresh")[0].bytes));
        value["data"][0]["files"][0]["uri"] = "data:image/png;base64,iVBORwE=".into();
        parse_messages(&value);
        assert!(!Arc::ptr_eq(&original, &user_files("msg_file_refresh")[0].bytes));
    }

    #[test]
    fn parses_media_refs_and_the_unavailable_placeholder() {
        let hash = "a".repeat(64);
        // As an opted-in read returns them: a ref, an inline file, and a ref whose blob is gone.
        let v = serde_json::json!({"data": [
            {"id": "msg_m1", "type": "user", "text": "", "files": [
                {"uri": format!("media:{hash}"), "mime": "image/png", "source": {"type": "inline"}, "name": "shot.png", "bytes": 1234},
                {"data": "iVBORw==", "mime": "image/png", "source": {"type": "inline"}, "name": "inline.png"},
                {"data": "W21lZGlhIHVuYXZhaWxhYmxlOiBnb25lLnBuZyAoaW1hZ2UvcG5nKV0=", "mime": "text/plain", "source": {"type": "inline"}, "name": "gone.png"},
                {"uri": "media:deadbeef", "mime": "image/png", "name": "bad.png"}
            ]},
            {"id": "msg_m2", "type": "assistant", "content": [
                {"type": "tool", "id": "t1", "name": "read", "state": {"status": "completed", "input": {},
                 "content": [{"type": "file", "uri": format!("media:{hash}"), "mime": "image/png", "name": "shot.png"}]}}
            ]}
        ]});
        let msgs = parse_messages(&v);
        let files = user_files("msg_m1");
        assert_eq!(files.len(), 3, "a malformed ref is skipped: {files:?}");
        assert_eq!((files[0].media.as_deref(), files[0].bytes.len(), files[0].mime.as_str()), (Some(hash.as_str()), 0, "image/png"));
        assert!(files[0].is_image() && files[0].unavailable().is_none());
        assert_eq!((files[1].media.as_deref(), files[1].bytes.len()), (None, 4));
        assert_eq!(files[2].unavailable(), Some("[media unavailable: gone.png (image/png)]"));
        let Some(Message::Assistant(a)) = msgs.iter().find(|m| matches!(m, Message::Assistant(_))) else { panic!() };
        let Part::Tool { files, .. } = &a.parts[0] else { panic!() };
        assert_eq!(files[0].media.as_deref(), Some(hash.as_str()));
        let item = parse_inbox_item(&serde_json::json!({"id": "msg_q", "type": "user", "delivery": "queue",
            "payload": {"text": "again", "files": [{"uri": format!("media:{hash}"), "mime": "image/png", "name": "again.png", "bytes": 3}]}}))
        .unwrap();
        assert_eq!(item.files[0].media.as_deref(), Some(hash.as_str()));
    }

    #[test]
    fn a_delivered_steer_sorts_after_the_work_it_waited_behind() {
        // As the server returns them: the steer keeps its send-time id, takes its delivery time.
        let v = serde_json::json!({"data": [
            {"id": "msg_3", "type": "assistant", "time": {"created": 40}, "content": []},
            {"id": "msg_2", "type": "user", "text": "also check X", "time": {"created": 30}},
            {"id": "msg_4", "type": "assistant", "time": {"created": 20}, "content": []},
            {"id": "msg_1", "type": "user", "text": "go", "time": {"created": 10}},
        ]});
        let ids = |m: &[Message]| m.iter().map(|m| m.id().to_owned()).collect::<Vec<_>>();
        let fresh = parse_messages(&v);
        assert_eq!(ids(&fresh), ["msg_1", "msg_4", "msg_2", "msg_3"]);
        // Merged into a transcript that already had the first turn, it lands in the same place.
        let mut t = Transcript { session: "s".into(), messages: fresh[..2].to_vec() };
        t.merge(fresh.clone());
        assert_eq!(ids(&t.messages), ["msg_1", "msg_4", "msg_2", "msg_3"]);
    }

    #[test]
    fn follows_the_inbox() {
        let mut inbox = Inbox::default();
        // As `GET /api/session/:id/inbox` lists them.
        let listed = serde_json::json!({"sessionID": "s", "items": [
            {"id": "msg_1", "type": "user", "delivery": "steer", "payload": {"text": "use tabs"}},
            {"id": "msg_c", "type": "compaction", "delivery": "steer", "payload": {}},
            {"id": "msg_2", "type": "user", "delivery": "queue", "payload": {"text": "full", "metadata": {"displayText": "then ship it"}}},
        ]});
        assert_eq!(inbox.apply("session.inbox.listed", &listed), InboxChange::Changed);
        assert_eq!(inbox.steering().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["use tabs"]);
        assert_eq!(inbox.queued().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["then ship it"]);

        let enqueued = serde_json::json!({"sessionID": "s", "inboxID": "msg_3",
            "item": {"type": "user", "delivery": "queue", "payload": {"text": "and docs"}}});
        assert_eq!(inbox.apply("session.inbox.enqueued", &enqueued), InboxChange::Changed);
        assert_eq!(inbox.apply("session.inbox.enqueued", &enqueued), InboxChange::None);
        let moved = serde_json::json!({"sessionID": "s", "inboxID": "msg_2", "delivery": "steer"});
        assert_eq!(inbox.apply("session.inbox.delivery.changed", &moved), InboxChange::Changed);
        assert_eq!(inbox.steering().count(), 2);
        let gone = serde_json::json!({"sessionID": "s", "inboxID": "msg_3"});
        assert_eq!(inbox.apply("session.inbox.cancelled", &gone), InboxChange::Changed);
        let InboxChange::Delivered(item) = inbox.apply("session.inbox.delivered", &serde_json::json!({"inboxID": "msg_1"})) else { panic!() };
        assert_eq!((item.id.as_str(), item.text.as_str()), ("msg_1", "use tabs"));
        assert_eq!(inbox.items.len(), 1);
    }

    #[test]
    fn streams_text_by_ordinal() {
        let mut t = Transcript { session: "s".into(), messages: vec![] };
        let ev = |ty: &str, v: Value| (ty.to_owned(), v);
        let evs = [
            ev("session.step.started", serde_json::json!({"sessionID":"s","assistantMessageID":"m"})),
            ev("session.text.started", serde_json::json!({"sessionID":"s","assistantMessageID":"m","ordinal":0})),
            ev("session.text.delta", serde_json::json!({"sessionID":"s","assistantMessageID":"m","ordinal":0,"delta":"Hel"})),
            ev("session.text.delta", serde_json::json!({"sessionID":"s","assistantMessageID":"m","ordinal":0,"delta":"lo"})),
        ];
        for (ty, d) in &evs {
            assert!(t.apply(ty, d, 0));
        }
        let Message::Assistant(a) = &t.messages[0] else { panic!() };
        assert!(matches!(&a.parts[0], Part::Text { text } if text == "Hello"));
        assert!(t.busy());
    }

    #[test]
    fn a_subagent_call_names_its_child_session() {
        let mut t = Transcript { session: "s".into(), messages: vec![] };
        let d = |v: Value| {
            let mut base = serde_json::json!({"sessionID": "s", "assistantMessageID": "m", "id": "t1"});
            base.as_object_mut().unwrap().extend(v.as_object().unwrap().clone());
            base
        };
        t.apply("session.step.started", &d(serde_json::json!({})), 0);
        t.apply("session.tool.called", &d(serde_json::json!({"name": "subagent", "input": {"agent": "general", "description": "Look"}})), 0);
        let child = |t: &Transcript| match &t.messages[0] {
            Message::Assistant(a) => match &a.parts[0] {
                Part::Tool { input, metadata, .. } => subagent_session(input, metadata).map(str::to_owned),
                _ => None,
            },
            _ => None,
        };
        assert_eq!(child(&t), None);
        assert!(t.apply("session.tool.progress", &d(serde_json::json!({"metadata": {"sessionID": "ses_child"}})), 0));
        assert_eq!(child(&t).as_deref(), Some("ses_child"));
        // A background call finishes at once; its metadata still names the child.
        t.apply("session.tool.success", &d(serde_json::json!({"metadata": {"sessionID": "ses_child", "status": "running"}})), 0);
        assert_eq!(child(&t).as_deref(), Some("ses_child"));
        let info = parse_session(&serde_json::json!({"id": "ses_child", "parentID": "s", "agent": "general", "outcome": "failed", "time": {"created": 5}}));
        assert_eq!((info.agent.as_str(), info.created, info.outcome.as_deref()), ("general", 5, Some("failed")));
    }

    #[test]
    fn a_failed_tool_keeps_its_error() {
        let mut t = Transcript { session: "s".into(), messages: vec![] };
        let d = |v: Value| {
            let mut base = serde_json::json!({"sessionID": "s", "assistantMessageID": "m", "id": "q1"});
            base.as_object_mut().unwrap().extend(v.as_object().unwrap().clone());
            base
        };
        t.apply("session.step.started", &d(serde_json::json!({})), 0);
        t.apply("session.tool.called", &d(serde_json::json!({"name": "question", "input": {"questions": []}})), 0);
        t.apply("session.tool.failed", &d(serde_json::json!({"error": {"type": "unknown", "message": "The user dismissed this question"}})), 0);
        let Message::Assistant(a) = &t.messages[0] else { panic!() };
        let Part::Tool { metadata, status, .. } = &a.parts[0] else { panic!() };
        assert_eq!((status, tool_error(metadata)), (&ToolStatus::Error, Some("The user dismissed this question")));
        // Stored, the same.
        let m = parse_message(&serde_json::json!({"id": "m", "type": "assistant", "content": [
            {"type": "tool", "id": "q1", "name": "question", "state": {"status": "error", "input": {}, "error": {"type": "unknown", "message": "boom"}}},
            {"type": "tool", "id": "q2", "name": "question", "state": {"status": "completed", "input": {}, "metadata": {"answers": [["A"]]}}},
        ]}));
        let Some(Message::Assistant(a)) = m else { panic!() };
        let errors: Vec<_> = a.parts.iter().map(|p| match p { Part::Tool { metadata, .. } => tool_error(metadata), _ => None }).collect();
        assert_eq!(errors, [Some("boom"), None]);
    }
}
