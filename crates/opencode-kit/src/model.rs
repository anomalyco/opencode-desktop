//! OpenCode's data as this app sees it: projects, sessions and transcripts, parsed leniently
//! from the server's JSON (`/api/project`, `/api/session`, `/api/session/:id/message`) and
//! kept current by the same events `/api/event` streams.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

mod attachment;
mod compaction;
mod inbox;
mod notice;
mod pending;
mod tool;
#[cfg(test)]
mod compaction_tests;

pub use attachment::{Attachment, base64_decode, media_hash, parse_attachment, set_user_files, user_files};
use attachment::parse_attachments;
pub use compaction::{Compaction, CompactionStatus, background_compaction};
pub use inbox::{Delivery, Inbox, InboxChange, InboxItem, NoticeEffect, NoticeEvent, NoticeState, parse_inbox_item, step_notice};
pub use pending::{Field, FieldKind, FieldOption, Pending, parse_form, parse_permission};
pub use notice::{Finished, Notice, NoticeKind, Outcome, Retry, StepError, is_notice_event, parse_inbox_notice};
pub use tool::{shell_message, tool_error, tool_error_metadata, tool_output, tool_ran};
use tool::{keep_tool_error, keep_tool_output, keep_tool_ran, replace_tool_metadata};
pub use crate::project::{LocationInfo, ProjectMeta, parse_location, parse_project, parse_worktrees, project_git, project_meta, project_sandboxes, set_project_sandboxes};
pub use crate::quote::{Quote, set_user_quotes, user_quotes};

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

#[derive(Clone, Debug, Default, PartialEq)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// When its latest run started (`time.busy`).
    pub busy: Option<i64>,
    /// The session's agent, when it was created (`time.created`), and how its last run ended
    /// (`outcome`: "succeeded", "failed", "interrupted").
    pub agent: String,
    pub created: i64,
    pub outcome: Option<String>,
    /// The message a staged revert rewinds to (`revert.messageID`): it and everything after it is hidden.
    pub revert: Option<String>,
    /// When it was archived (`time.archived`).
    pub archived: Option<i64>,
    /// The session's `metadata`, which the server copies onto every child it starts.
    pub metadata: Value,
}

impl SessionInfo {
    /// `metadata.seerProject`: the project session this one is, or was started under, and when
    /// it was marked.
    pub fn seer_project(&self) -> Option<(&str, i64)> {
        let m = &self.metadata["seerProject"];
        Some((m["id"].as_str()?, m["since"].as_i64()?))
    }

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
    /// A failed call keeps its error there too: see [`tool_error`]; and when it began running:
    /// see [`tool_ran`].
    Tool { id: String, name: String, status: ToolStatus, title: String, input: Value, files: Vec<Attachment>, metadata: Value },
}

/// The child session a subagent call runs in, once the call names it.
pub fn subagent_session<'a>(input: &'a Value, metadata: &'a Value) -> Option<&'a str> {
    metadata["sessionID"].as_str().or_else(|| input["sessionID"].as_str()).filter(|s| !s.is_empty())
}

/// A message's model (`{ id, providerID, variant }`), when it names one.
fn model_of(v: &Value) -> Option<ModelRef> {
    let m = ModelRef::parse(v);
    (!m.id.is_empty() || !m.provider.is_empty()).then_some(m)
}

/// User messages' creation times, by message id, as [`user_files`] keeps their files.
fn user_created_store() -> &'static Mutex<HashMap<String, i64>> {
    static STORE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// When user message `id` was sent (ms), once known: a reply's duration runs from it.
pub fn user_created(id: &str) -> Option<i64> {
    user_created_store().lock().unwrap().get(id).copied()
}

/// Records when user message `id` was sent (ms).
pub fn set_user_created(id: &str, at: i64) {
    user_created_store().lock().unwrap().insert(id.to_owned(), at);
}

#[derive(Clone, Debug)]
pub enum Message {
    User { id: String, text: String },
    Assistant(Assistant),
    /// A shell command run from the composer (`!`): `text` reads `$ command`, `failed` when it
    /// timed out or exited non-zero, and `step` holds it as the `shell` call Desktop draws it as
    /// (`SessionShellMessage`): its command, output and how it ended (see [`shell_message`]).
    Note { id: String, text: String, failed: bool, step: Box<Assistant> },
    Compaction(Compaction),
    /// A switched agent or model, a move, updated instructions, a restart.
    Notice(Notice),
}

#[derive(Clone, Debug, Default)]
pub struct Assistant {
    pub id: String,
    pub completed: Option<i64>,
    pub parts: Vec<Part>,
    pub cost: f64,
    pub tokens: Tokens,
    pub error: Option<String>,
    /// The error's details, beside its message in `error`.
    pub failure: Option<StepError>,
    /// The server is waiting to try this step again.
    pub retry: Option<Retry>,
    /// When it started, or 0 when unknown.
    pub created: i64,
    /// The model that ran it (`model` in `/api` and `session.step.started`), as Desktop's reply
    /// footer names it; `None` until known.
    pub model: Option<ModelRef>,
    /// The agent that ran it, as the footer names it when agents are shown.
    pub agent: Option<String>,
    /// The files the step changed, by path (`snapshot.files`), for its turn's Changed files.
    pub changed: Vec<String>,
}

impl Message {
    pub fn id(&self) -> &str {
        match self {
            Message::User { id, .. } | Message::Note { id, .. } => id,
            Message::Assistant(a) => &a.id,
            Message::Compaction(c) => &c.id,
            Message::Notice(n) => &n.id,
        }
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_owned()
}

/// A JSON array's strings.
fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect()
}

fn i(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(0)
}

pub fn parse_projects(v: &Value) -> Vec<Project> {
    let list = v.as_array().or_else(|| v["data"].as_array());
    list.into_iter().flatten().filter_map(crate::project::parse_project).collect()
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
        busy: p["time"]["busy"].as_i64(),
        agent: s(&p["agent"]),
        created: i(&p["time"]["created"]),
        outcome: p["outcome"].as_str().map(str::to_owned),
        revert: p["revert"]["messageID"].as_str().map(str::to_owned),
        archived: p["time"]["archived"].as_i64(),
        metadata: p["metadata"].clone(),
    }
}

pub fn parse_sessions(v: &Value) -> Vec<SessionInfo> {
    v["data"].as_array().or_else(|| v.as_array()).into_iter().flatten().map(parse_session).collect()
}

fn tool_title(name: &str, input: &Value) -> String {
    let get = |k: &str| input[k].as_str().map(str::to_owned);
    let short = |p: String| if p.split(crate::path::SEPARATORS).count() > 3 { crate::path::tail(&p, 2).to_owned() } else { p };
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
            keep_tool_output(&mut metadata, &c["state"]["content"]);
            keep_tool_ran(&mut metadata, &c["time"]);
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
            if let Some(at) = m["time"]["created"].as_i64() {
                set_user_created(&id, at);
            }
            Message::User { text: quoted_text(&id, text, &m["metadata"]), id }
        }
        "assistant" => Message::Assistant(Assistant {
            id,
            completed: m["time"]["completed"].as_i64(),
            parts: m["content"].as_array().into_iter().flatten().filter_map(parse_part).collect(),
            cost: m["cost"].as_f64().unwrap_or(0.0),
            tokens: Tokens::parse(&m["tokens"]),
            error: m["error"]["message"].as_str().map(str::to_owned),
            failure: StepError::parse(&m["error"]),
            retry: Retry::parse(&m["retry"]),
            created: i(&m["time"]["created"]),
            model: model_of(&m["model"]),
            agent: m["agent"].as_str().filter(|a| !a.is_empty()).map(str::to_owned),
            changed: strings(&m["snapshot"]["files"]),
        }),
        "shell" => Message::Note {
            text: format!("$ {}", s(&m["command"])),
            failed: m["status"] == "timeout" || (m["status"] == "exited" && m["exit"].as_i64().is_some_and(|e| e != 0)),
            step: Box::new(tool::shell_step(&id, m)),
            id,
        },
        "compaction" => Message::Compaction(Compaction::parse(m)),
        _ => return notice::parse(m),
    })
}

/// A user message's text as its bubble shows it (Desktop's `readPromptPresentation`): without
/// the `Attached file:` lines and quotes it carries for the model, which show as cards over it
/// (see `mention::user_path_attachments` and [`user_quotes`]); the typed text when they aren't
/// at its end as Desktop puts them (`composer/request.ts`).
fn quoted_text(id: &str, text: String, metadata: &Value) -> String {
    let Some((display, quotes)) = crate::quote::parse_metadata(metadata) else { return text };
    let refs = crate::mention::path_attachments(metadata).unwrap_or_default();
    if quotes.is_empty() && refs.is_empty() {
        return text;
    }
    let tail: Vec<String> = refs.iter().map(|r| format!("Attached file: `{}`", r.path)).chain(quotes.iter().map(Quote::prompt_text)).collect();
    let tail = tail.join("\n");
    set_user_quotes(id, quotes);
    match text.strip_suffix(tail.as_str()) {
        Some(head) => head.strip_suffix('\n').unwrap_or(head).to_owned(),
        None => {
            // The recorded mention offsets index the raw text, not this one.
            crate::mention::set_user_mentions(id, Vec::new());
            display
        }
    }
}

/// Messages in chronological order from one `desc` page.
pub fn parse_messages(v: &Value) -> Vec<Message> {
    let mut list: Vec<&Value> = v["data"].as_array().into_iter().flatten().collect();
    // By `time.created`, then id. A steered prompt keeps the id it was sent with but takes its
    // delivery time, so it sorts after the work it waited behind, as the server orders it.
    list.sort_by(|a, b| (i(&a["time"]["created"]), a["id"].as_str()).cmp(&(i(&b["time"]["created"]), b["id"].as_str())));
    list.into_iter().filter_map(parse_message).collect()
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
        // A notice (a model switched with the prompt) doesn't end the turn it lands in.
        let last = self.messages.iter().rev().find(|m| !matches!(m, Message::Notice(_)));
        matches!(last, Some(Message::Assistant(a)) if a.completed.is_none() && a.error.is_none())
            || matches!(last, Some(Message::User { .. }))
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
                // Nor rewind a compaction that ended to a copy read while it ran.
                if let (Message::Compaction(old), Message::Compaction(new)) = (&*slot, &m)
                    && old.status != CompactionStatus::Running
                    && new.status == CompactionStatus::Running
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
                    set_user_created(&id, d["time"]["created"].as_i64().unwrap_or(now));
                    let text = s(&d["text"]);
                    crate::mention::record(&id, &text, d);
                    self.messages.push(Message::User { id, text });
                }
            }
            "session.step.started" => {
                let (model, agent) = (model_of(&d["model"]), d["agent"].as_str().filter(|a| !a.is_empty()).map(str::to_owned));
                match self.assistant(&mid) {
                    // A retried step starts again under its own id.
                    Some(a) => {
                        a.retry = None;
                        a.error = None;
                        a.failure = None;
                        a.model = model.or(a.model.take());
                        a.agent = agent.or(a.agent.take());
                    }
                    None => self.messages.push(Message::Assistant(Assistant { id: mid, created: now, model, agent, ..Default::default() })),
                }
            }
            "session.retry.scheduled" => {
                let Some(a) = self.assistant(&mid) else { return false };
                a.retry = Retry::parse(d);
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                // The run ends, as the idle message the server records says (a shutdown resumes it).
                let idle = !(ty == "session.execution.interrupted" && d["reason"] == "shutdown") && self.apply_idle(d);
                let Some(Message::Assistant(a)) = self.messages.iter_mut().rev().find(|m| matches!(m, Message::Assistant(_))) else { return idle };
                if a.retry.take().is_none() {
                    return idle;
                }
            }
            ty if notice::is_notice_event(ty) => return self.apply_notice(ty, d),
            "session.text.started" | "session.reasoning.started" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let part = if ty == "session.text.started" {
                    Part::Text { text: String::new() }
                } else {
                    Part::Reasoning { text: String::new(), started: now, completed: None }
                };
                let at = slot(&a.parts, ty == "session.text.started", ordinal);
                place(&mut a.parts, at, part);
            }
            "session.text.delta" | "session.reasoning.delta" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let delta = d["delta"].as_str().unwrap_or("");
                let want_text = ty == "session.text.delta";
                let at = slot(&a.parts, want_text, ordinal).or_else(|| slot(&a.parts, want_text, None));
                match at.map(|k| &mut a.parts[k]) {
                    Some(Part::Text { text }) => text.push_str(delta),
                    Some(Part::Reasoning { text, .. }) => text.push_str(delta),
                    _ => {
                        let part = if want_text {
                            Part::Text { text: delta.to_owned() }
                        } else {
                            Part::Reasoning { text: delta.to_owned(), started: now, completed: None }
                        };
                        a.parts.push(part);
                    }
                }
            }
            "session.text.ended" | "session.reasoning.ended" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let full = d["text"].as_str().map(str::to_owned);
                let want_text = ty == "session.text.ended";
                match slot(&a.parts, want_text, ordinal).or_else(|| slot(&a.parts, want_text, None)).map(|k| &mut a.parts[k]) {
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
                        replace_tool_metadata(metadata, &d["metadata"]);
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
                            replace_tool_metadata(metadata, &d["metadata"]);
                        }
                        if ty == "session.tool.failed" {
                            keep_tool_error(metadata, &d["error"]);
                        }
                        keep_tool_output(metadata, &d["content"]);
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
                a.retry = None;
                if d["files"].is_array() {
                    a.changed = strings(&d["files"]);
                }
                if ty == "session.step.failed" {
                    a.error = Some(d["error"]["message"].as_str().unwrap_or("The step failed").to_owned());
                    a.failure = StepError::parse(&d["error"]);
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

/// Where a step's text or reasoning part with `ordinal` sits in its mixed `parts`. The server
/// counts each kind on its own (a step's first thought and first answer are both 0), so it's
/// the ordinal-th part of that kind. Without an ordinal, the last part if it's that kind.
fn slot(parts: &[Part], text: bool, ordinal: Option<usize>) -> Option<usize> {
    let kind = |p: &Part| if text { matches!(p, Part::Text { .. }) } else { matches!(p, Part::Reasoning { .. }) };
    match ordinal {
        Some(o) => parts.iter().enumerate().filter(|(_, p)| kind(p)).nth(o).map(|(i, _)| i),
        None => parts.last().filter(|p| kind(p)).map(|_| parts.len() - 1),
    }
}

fn place(parts: &mut Vec<Part>, at: Option<usize>, part: Part) {
    match at {
        Some(i) => parts[i] = part,
        None => parts.push(part),
    }
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
    fn thoughts_and_answers_count_their_ordinals_apart() {
        // The server numbers a step's reasoning and text parts separately: both start at 0.
        let mut t = Transcript { session: "s".into(), messages: vec![] };
        let d = |v: Value| {
            let mut base = serde_json::json!({"sessionID": "s", "assistantMessageID": "m"});
            base.as_object_mut().unwrap().extend(v.as_object().unwrap().clone());
            base
        };
        t.apply("session.step.started", &d(serde_json::json!({})), 0);
        t.apply("session.reasoning.started", &d(serde_json::json!({"ordinal": 0})), 1);
        t.apply("session.reasoning.delta", &d(serde_json::json!({"ordinal": 0, "delta": "Look"})), 1);
        t.apply("session.reasoning.ended", &d(serde_json::json!({"ordinal": 0, "text": "Look first."})), 2);
        t.apply("session.text.started", &d(serde_json::json!({"ordinal": 0})), 3);
        t.apply("session.text.delta", &d(serde_json::json!({"ordinal": 0, "delta": "Done"})), 3);
        t.apply("session.reasoning.started", &d(serde_json::json!({"ordinal": 1})), 4);
        t.apply("session.reasoning.delta", &d(serde_json::json!({"ordinal": 1, "delta": "Again"})), 4);
        t.apply("session.text.delta", &d(serde_json::json!({"ordinal": 0, "delta": "."})), 5);
        t.apply("session.text.ended", &d(serde_json::json!({"ordinal": 0, "text": "Done."})), 5);
        let Message::Assistant(a) = &t.messages[0] else { panic!() };
        let got: Vec<String> = a.parts.iter().map(|p| match p {
            Part::Reasoning { text, completed, .. } => format!("thought {text} {completed:?}"),
            Part::Text { text } => format!("text {text}"),
            Part::Tool { .. } => "tool".into(),
        }).collect();
        assert_eq!(got, ["thought Look first. Some(2)", "text Done.", "thought Again None"]);
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

    #[test]
    fn a_quoted_prompt_shows_its_typed_text_and_keeps_its_quotes() {
        let q = Quote { text: "a\nb".into(), comment: None };
        let m = serde_json::json!({"id": "msg_quoted", "type": "user", "text": format!("look\nAttached file: `/x`\n{}", q.prompt_text()),
            "metadata": {"displayText": "look", "comments": [q.json()], "attachments": []}});
        let Some(Message::User { text, .. }) = parse_message(&m) else { panic!() };
        assert_eq!(text, "look\nAttached file: `/x`");
        assert_eq!(user_quotes("msg_quoted").as_ref(), [q.clone()]);
        let only = serde_json::json!({"id": "msg_only", "type": "user", "text": q.prompt_text(), "metadata": {"displayText": "", "comments": [q.json()]}});
        let Some(Message::User { text, .. }) = parse_message(&only) else { panic!() };
        assert_eq!(text, "");
    }
}
