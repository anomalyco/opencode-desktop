//! OpenCode's data as this app sees it: projects, sessions and transcripts, parsed leniently
//! from the server's JSON (`/api/project`, `/api/session`, `/api/session/:id/message`) and
//! kept current by the same events `/api/event` streams.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

mod attachment;
mod compaction;
mod inbox;
mod notice;
mod pending;
pub mod preview;
mod tool;

pub use attachment::{Attachment, base64_decode, media_hash, parse_attachment, prompt_extras_generation, set_user_files, user_files};
pub(crate) use attachment::extras_changed;
use attachment::parse_attachments;
pub use compaction::{Compaction, CompactionStatus, background_compaction};
pub use inbox::{Delivery, Inbox, InboxChange, InboxItem, NoticeEffect, NoticeEvent, NoticeState, inbox_payload, parse_inbox_item, step_notice};
pub use pending::{Field, FieldKind, FieldOption, Pending, parse_form, parse_permission};
pub use notice::{Finished, Notice, NoticeKind, Outcome, Retry, StepError, is_notice_event, parse_inbox_notice};
pub use tool::{shell_message, tool_error, tool_error_metadata, tool_output, tool_ran};
use tool::{keep_tool_error, keep_tool_output, keep_tool_ran, replace_tool_metadata};
pub use crate::project::{LocationInfo, ProjectMeta, parse_location, parse_project, parse_worktrees, project_git, project_meta, project_sandboxes, set_project_sandboxes};
pub use crate::quote::{CommentOrigin, FileComment, PromptComment, Quote, set_user_comments, user_comments};

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

    /// The image to show for project `id`, as Desktop picks it (`getProjectAvatarSource`,
    /// `project-avatar.tsx:78-86`): a chosen image, else none when a colour is chosen, else
    /// OpenCode's favicon for OpenCode's own repo, else the discovered one.
    pub fn source(&self, id: &str) -> Option<&str> {
        if !self.override_url.is_empty() {
            return Some(&self.override_url);
        }
        if !self.color.is_empty() {
            return None;
        }
        if OPENCODE_PROJECT_IDS.contains(&id) {
            return Some(OPENCODE_FAVICON);
        }
        (!self.url.is_empty()).then_some(self.url.as_str())
    }
}

/// OpenCode's own repo (`project-avatar.tsx:72-76`): its ids hash the origin remote, V1's the
/// root commit. The repo holds no icon file the server would find, so Desktop names one.
const OPENCODE_PROJECT_IDS: [&str; 3] =
    ["012780c4098d08caa4ea8c479ed0a4690489f38d", "476fbfe6befa9e63ddd7f9ae71655999a0104256", "4b0ea68d7af9a6031a7ffda7ad66e0cb83315750"];

/// The drawing of `https://opencode.ai/favicon.svg` (`packages/ui/src/assets/favicon/favicon.svg`),
/// which Desktop loads for those ids: carried here, so it shows on the first frame and offline.
const OPENCODE_FAVICON: &str = "data:image/svg+xml;utf8,<svg width=\"512\" height=\"512\" viewBox=\"0 0 512 512\" fill=\"none\" xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"512\" height=\"512\" fill=\"#131010\"/><path d=\"M320 224V352H192V224H320Z\" fill=\"#5A5858\"/><path fill-rule=\"evenodd\" clip-rule=\"evenodd\" d=\"M384 416H128V96H384V416ZM320 160H192V352H320V160Z\" fill=\"white\"/></svg>";

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

    /// When it was last active: `time.updated`, else `time.created` (`updated ?? created`).
    pub fn active_ms(&self) -> i64 {
        if self.updated > 0 { self.updated } else { self.created }
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

/// What the stores beside `Message::User` keep by message id (its files, creation times, mentions,
/// path attachments, comments), kept only for the messages `keep` names: an app that lets go of a
/// transcript calls it, so those go too. A message parsed again fills them again.
pub fn retain_user_messages(keep: &dyn Fn(&str) -> bool) {
    attachment::retain_user_files(keep);
    user_created_store().lock().unwrap().retain(|id, _| keep(id));
    crate::mention::retain_user_messages(keep);
    crate::quote::retain_user_comments(keep);
    // What was let go of a projection may have read: it projects again.
    extras_changed();
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
    /// What the step reported as it finished (`tokens`); none while it streams, or when it
    /// ended without them.
    pub tokens: Option<Tokens>,
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
            tokens: m["tokens"].is_object().then(|| Tokens::parse(&m["tokens"])),
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
/// (see `mention::user_path_attachments` and [`user_comments`]); the typed text when they aren't
/// at its end as Desktop puts them (`composer/request.ts`).
fn quoted_text(id: &str, text: String, metadata: &Value) -> String {
    let Some((display, comments)) = crate::quote::parse_metadata(metadata) else { return text };
    let refs = crate::mention::path_attachments(metadata).unwrap_or_default();
    if comments.is_empty() && refs.is_empty() {
        return text;
    }
    let tail: Vec<String> = refs.iter().map(|r| format!("Attached file: `{}`", r.path)).chain(comments.iter().map(PromptComment::prompt_text)).collect();
    let tail = tail.join("\n");
    set_user_comments(id, comments);
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

/// In a dropped stream's gapped steps (`Transcript::apply_caught`): a step not known yet when it
/// dropped is gapped too, until the page read after it lands.
pub const UNSEEN: &str = "\u{0}unseen";

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

    /// The latest assistant step while it's unfinished: the reply whose calls are running. Only
    /// the newest step can be; an older one left without its end (a stream that dropped) isn't
    /// the one at work. It stops at the newest assistant step, so it reads a few messages at any
    /// transcript size.
    pub fn open_reply(&self) -> Option<&Assistant> {
        self.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        }).filter(|a| a.completed.is_none())
    }

    /// What a stream that drops now leaves mid-text: the assistant steps still open, and
    /// ([`UNSEEN`]) any step the drop hides the start of, until the next page read says.
    pub fn open_steps(&self) -> HashSet<String> {
        self.messages.iter().filter_map(|m| match m {
            Message::Assistant(a) if a.completed.is_none() && a.error.is_none() => Some(a.id.clone()),
            _ => None,
        }).chain(std::iter::once(UNSEEN.to_owned())).collect()
    }

    /// [`Transcript::apply`] after the stream dropped while the steps in `gapped` were open (the
    /// catching-up barrier). OpenCode's deltas carry no offset, so one heard after the gap
    /// can't be told apart from one the gap swallowed: a gapped step's deltas are held back, and
    /// its text grows again only from whole text (`*.ended`, a page read). What it shows stays a
    /// prefix of what it ends as: nothing lost into the middle, nothing said twice. Returns
    /// whether it changed, and whether the step ended without its whole text (its page should be
    /// read again).
    pub fn apply_caught(&mut self, ty: &str, d: &Value, now: i64, gapped: &mut HashSet<String>) -> (bool, bool) {
        let mid = s(&d["assistantMessageID"]);
        // A step heard of only after the drop, mid-text, may have started in the gap: its start
        // is gone, so it's gapped too. One whose start is heard now began after it.
        if gapped.contains(UNSEEN) && !mid.is_empty() && self.assistant(&mid).is_none() && matches!(ty, "session.text.delta" | "session.reasoning.delta") {
            gapped.insert(mid.clone());
        }
        if !gapped.contains(&mid) {
            return (self.apply(ty, d, now), false);
        }
        match ty {
            "session.text.delta" | "session.reasoning.delta" => (false, false),
            "session.step.ended" | "session.step.failed" => {
                gapped.remove(&mid);
                (self.apply(ty, d, now), true)
            }
            _ => (self.apply(ty, d, now), false),
        }
    }

    /// [`Transcript::merge`] after a dropped stream (which, like any page read, keeps the text a
    /// step shows where the page's copy is only a start of it): an open step this page brings
    /// that the drop hid joins the barrier, and a step the page says ended leaves it.
    pub fn merge_caught(&mut self, fresh: Vec<Message>, gapped: &mut HashSet<String>) {
        let unseen = gapped.remove(UNSEEN);
        for m in &fresh {
            let Message::Assistant(new) = m else { continue };
            let open = new.completed.is_none() && new.error.is_none();
            // An open step this page brings that the drop hid: what streams next is its middle.
            if unseen && open && !self.messages.iter().any(|o| o.id() == new.id) {
                gapped.insert(new.id.clone());
            }
            if !open {
                gapped.remove(&new.id);
            }
        }
        self.merge(fresh);
    }

    pub fn merge(&mut self, fresh: Vec<Message>) {
        let mut prev: Option<String> = None;
        for mut m in fresh {
            let id = m.id().to_owned();
            let after = prev.replace(id).and_then(|p| self.messages.iter().position(|o| o.id() == p));
            if let Some(slot) = self.messages.iter_mut().find(|o| o.id() == m.id()) {
                // Keep live-streamed text when the fetched copy is behind.
                if let (Message::Assistant(old), Message::Assistant(new)) = (&*slot, &mut m)
                    && new.completed.is_none()
                {
                    if old.parts.len() > new.parts.len() {
                        continue;
                    }
                    // OpenCode writes a part's text when the part ends: a page read mid-reply has
                    // it empty or short, and taking it would wipe what streamed (and the deltas
                    // after would run on from nothing).
                    for (fresh_part, shown) in new.parts.iter_mut().zip(&old.parts) {
                        match (fresh_part, shown) {
                            (Part::Text { text: f }, Part::Text { text: o }) | (Part::Reasoning { text: f, .. }, Part::Reasoning { text: o, .. }) if o.starts_with(f.as_str()) => *f = o.clone(),
                            _ => {}
                        }
                    }
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
                    a.tokens = Some(Tokens::parse(&d["tokens"]));
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

pub fn tokens_short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

