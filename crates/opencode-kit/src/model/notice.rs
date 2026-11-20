//! A step's failure and retry, and the session's notices (agent, model or variant switched,
//! moved, instructions updated, continuing after a restart), as OpenCode's message updater
//! records them: parsed from messages and kept current by their `session.*` events.

use serde_json::Value;

use super::{Message, ModelRef, Transcript, s};

/// A step's structured error (`Session.StructuredError`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepError {
    /// Its `type`, such as `provider.free-limit`.
    pub kind: String,
    pub message: String,
    /// The HTTP status the provider answered with (100–599), as `status` carries it: Console's
    /// 402 means the workspace is out of credits.
    pub status: Option<u16>,
    /// How long the provider asked to wait, from when the failure was recorded.
    pub retry_after_ms: Option<i64>,
    /// The provider's response body.
    pub body: Option<String>,
}

impl StepError {
    pub fn parse(v: &Value) -> Option<StepError> {
        let message = v["message"].as_str()?;
        Some(StepError {
            kind: s(&v["type"]),
            message: message.to_owned(),
            status: v["status"].as_u64().filter(|n| (100..=599).contains(n)).map(|n| n as u16),
            retry_after_ms: v["retryAfterMs"].as_i64(),
            body: v["response"]["body"].as_str().map(str::to_owned),
        })
    }
}

/// The server is about to try a failed step again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Retry {
    pub attempt: u32,
    /// When, in epoch milliseconds.
    pub at: i64,
    /// What failed.
    pub message: String,
}

impl Retry {
    pub fn parse(v: &Value) -> Option<Retry> {
        let attempt = v["attempt"].as_u64()? as u32;
        Some(Retry { attempt, at: v["at"].as_i64().unwrap_or(0), message: s(&v["error"]["message"]) })
    }
}

/// What a widget's note to the agent was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetNote {
    /// Its final answer (or dismissal).
    Response,
    /// A message it sent while staying open (`send`).
    Message,
    /// A client reported that running it failed (`report`).
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: String,
    pub kind: NoticeKind,
}

impl Notice {
    /// An idle marker: the run ended.
    pub fn is_idle(&self) -> bool {
        matches!(self.kind, NoticeKind::Idle { .. })
    }

    /// An idle marker's failure, when the run failed outside its steps.
    pub fn idle_failure(&self) -> Option<&StepError> {
        match &self.kind {
            NoticeKind::Idle { failure } => failure.as_ref(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoticeKind {
    Agent { agent: String, previous: Option<String> },
    Model { model: ModelRef, previous: Option<ModelRef> },
    /// The session's working directory changed.
    Moved { directory: String },
    /// Instruction files changed, by their sources.
    Instructions { sources: Vec<String> },
    Restart,
    /// A background shell or subagent came back (`Shell finished`, `Explore failed`).
    Finished(Finished),
    /// A background tool report, linked to the initiating call by its job ID. `script`: a
    /// finished background Code Mode script's outcome (`source == "codemode"`,
    /// anomalyco/opencode#64741). `required`: true when the job errored (`timelineNoticeRequired`).
    Job { id: String, description: String, script: Option<Outcome>, required: bool },
    Text(String),
    /// An agent widget's answer or message to the agent (`metadata.widget`): the synthetic
    /// message the server admits for it, which the timeline shows as one quiet line
    /// (`Responded to "Hero image"`, `Hero image: blue`). `message` and `call`: the widget's
    /// tool call; `label`: the server's `description` of it.
    Widget { message: String, call: String, note: WidgetNote, label: String },
    /// The run ended (an `idle` message): it closes the turn whose changed files Changed files
    /// lists. `failure`: the run failed before or between its steps (`outcome: "failed"` with an
    /// `error`, a model with no route, say), which the turn shows as its error.
    Idle { failure: Option<StepError> },
}

/// A synthetic message a background shell or subagent sends back when it ends, as Desktop's
/// timeline shows it: who (`Shell`, or the agent's name), how it ended, and what it was.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    /// The subagent's agent (`explore`); `None` for a shell.
    pub agent: Option<String>,
    pub subagent: bool,
    pub outcome: Outcome,
    /// Its `description`: the subagent's task, or the command that failed to start.
    pub data: Option<String>,
    /// Shown however Timeline detail places notices: it failed (`timelineNoticeRequired`).
    pub required: bool,
    /// The shell (`metadata.shellID`) or child session (`metadata.childID`) it came from.
    pub shell: Option<String>,
    pub child: Option<String>,
}

/// How a background shell or subagent ended (`metadata.state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Error,
    Cancelled,
}

fn outcome_of(state: Option<&str>) -> Outcome {
    match state {
        Some("error") => Outcome::Error,
        Some("cancelled") => Outcome::Cancelled,
        _ => Outcome::Completed,
    }
}

/// A shell or subagent's synthetic message as a notice, or `None` when Desktop shows none: a
/// shell's with no description that didn't fail (`isNotice`, `timelineNoticeRequired`). A
/// subagent's report is always one, its own row in the timeline (anomalyco/opencode#64017).
/// Falls back to `<background-result>` / legacy `<shell>` / `<subagent>` XML attributes when
/// metadata fields are absent (anomalyco/opencode#64971).
fn finished(m: &Value, description: Option<&str>, subagent: bool, bg: Option<&super::BackgroundResult>) -> Option<NoticeKind> {
    let meta = &m["metadata"];
    let state = meta["state"].as_str().or_else(|| bg.and_then(|b| b.state.as_deref()));
    // Shell completion reports the process outcome in metadata, not its state (`shellResultFailed`).
    let shell_failed = meta["timeout"] == true || meta["exit"].as_i64().is_some_and(|e| e != 0);
    let required = state == Some("error") || (!subagent && state == Some("completed") && shell_failed);
    let data = description
        .or_else(|| {
            bg.and_then(|b| if subagent { b.description.as_deref() } else { b.command.as_deref() })
                .map(str::trim)
                .filter(|v| !v.is_empty())
        })
        .map(str::to_owned);
    if !subagent && data.is_none() && !required {
        return None;
    }
    let outcome = outcome_of(state);
    let id = |k: &str| meta[k].as_str().filter(|v| !v.is_empty()).map(str::to_owned);
    Some(NoticeKind::Finished(Finished {
        agent: subagent.then(|| id("agent")).flatten(),
        subagent,
        outcome,
        data,
        required,
        shell: (!subagent).then(|| id("shellID").or_else(|| bg.and_then(|b| b.id.clone()))).flatten(),
        child: subagent.then(|| id("childID").or_else(|| bg.and_then(|b| b.session_id.clone()))).flatten(),
    }))
}

/// A synthetic inbox item (`{id, type, payload, delivery}` or `{inboxID, item}`) as the notice
/// Desktop shows for it while it waits: a background shell or subagent that came back.
pub fn parse_inbox_notice(v: &Value) -> Option<Notice> {
    let (id, item) = if v["item"].is_object() { (&v["inboxID"], &v["item"]) } else { (&v["id"], v) };
    if item["type"] != "synthetic" {
        return None;
    }
    let kind = kind("synthetic", &item["payload"])?;
    Some(Notice { id: s(id), kind })
}

fn model_ref(v: &Value) -> Option<ModelRef> {
    v.is_object().then(|| ModelRef::parse(v))
}

fn instructions(sources: Vec<String>) -> NoticeKind {
    NoticeKind::Instructions { sources }
}

/// A notice message, or `None` for a type that isn't one or a notice the timeline doesn't show.
pub(super) fn parse(m: &Value) -> Option<Message> {
    let id = s(&m["id"]);
    let kind = kind(m["type"].as_str()?, m)?;
    Some(Message::Notice(Notice { id, kind }))
}

fn kind(ty: &str, m: &Value) -> Option<NoticeKind> {
    let description = m["description"].as_str().map(str::trim).filter(|d| !d.is_empty());
    Some(match ty {
        "idle" => NoticeKind::Idle { failure: if m["outcome"] == "failed" { StepError::parse(&m["error"]) } else { None } },
        "agent-switched" => NoticeKind::Agent { agent: s(&m["agent"]), previous: m["previous"].as_str().map(str::to_owned) },
        "model-switched" => NoticeKind::Model { model: ModelRef::parse(&m["model"]), previous: model_ref(&m["previous"]) },
        "location-switched" => NoticeKind::Moved { directory: s(&m["location"]["directory"]) },
        // A widget's answer or message (`metadata.widget = { messageID, callID, kind }`).
        "synthetic" if m["metadata"]["widget"].is_object() => {
            let w = &m["metadata"]["widget"];
            let text = m["text"].as_str().unwrap_or("");
            let note = match w["kind"].as_str() {
                Some("message") => WidgetNote::Message,
                Some("error") => WidgetNote::Error,
                _ => WidgetNote::Response,
            };
            let label = description.map(str::to_owned).unwrap_or_else(|| text.lines().next().unwrap_or("").to_owned());
            NoticeKind::Widget { message: s(&w["messageID"]), call: s(&w["callID"]), note, label }
        }
        "system" => {
            let sources: Vec<String> = m["metadata"]["instructionSources"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
            if m["metadata"]["notice"] == "instructions" && !sources.is_empty() {
                return Some(instructions(sources));
            }
            match description.and_then(|d| d.strip_prefix("Instructions updated: ")) {
                Some(keys) => instructions(keys.split(',').map(str::trim).filter(|k| !k.is_empty()).map(str::to_owned).collect()),
                None => NoticeKind::Text(description.map_or_else(|| s(&m["text"]), str::to_owned)),
            }
        }
        "synthetic" => {
            if m["metadata"]["notice"] == "restart" || description == Some("Continuing after restart") {
                return Some(NoticeKind::Restart);
            }
            let bg = m["text"].as_str().and_then(super::parse_background_result);
            let source = m["metadata"]["source"].as_str();
            let is_subagent = source == Some("subagent")
                || (source.is_none() && matches!(&bg, Some(b) if b.kind == super::BackgroundResultKind::Subagent));
            let is_shell = source == Some("shell")
                || (source.is_none() && matches!(&bg, Some(b) if b.kind == super::BackgroundResultKind::Shell));
            if is_subagent || is_shell {
                return finished(m, description, is_subagent, bg.as_ref());
            }
            // Background Code Mode script notice (`timeline-next/row.tsx`, anomalyco/opencode#64741, #64971).
            let is_codemode = source == Some("codemode")
                || (source.is_none() && matches!(&bg, Some(b) if b.kind == super::BackgroundResultKind::CodeMode));
            if is_codemode {
                let id = m["metadata"]["jobID"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .or_else(|| bg.as_ref().and_then(super::BackgroundResult::target_id))
                    .unwrap_or("");
                let outcome = outcome_of(m["metadata"]["state"].as_str().or_else(|| bg.as_ref().and_then(|b| b.state.as_deref())));
                return Some(NoticeKind::Job {
                    id: id.to_owned(),
                    description: description.unwrap_or("Script").to_owned(),
                    script: Some(outcome),
                    required: outcome == Outcome::Error,
                });
            }
            // Image, video, or unknown future `<background-result kind="...">` notices: tolerated
            // without crashing or dropping messages even when metadata/description are sparse.
            let job_id = m["metadata"]["jobID"]
                .as_str()
                .filter(|id| !id.is_empty())
                .or_else(|| bg.as_ref().and_then(super::BackgroundResult::target_id));
            let state = m["metadata"]["state"].as_str().or_else(|| bg.as_ref().and_then(|b| b.state.as_deref()));
            let required = state == Some("error");
            if let Some(id) = job_id {
                let label = description
                    .map(str::to_owned)
                    .or_else(|| bg.as_ref().and_then(super::BackgroundResult::fallback_description))
                    .unwrap_or_else(|| id.to_owned());
                return Some(NoticeKind::Job { id: id.to_owned(), description: label, script: None, required });
            }
            let text = description
                .map(str::to_owned)
                .or_else(|| bg.as_ref().and_then(super::BackgroundResult::fallback_description))?;
            NoticeKind::Text(text)
        }
        _ => return None,
    })
}

/// The notice a `session.*` event appends, under the id its message takes (`evt_` → `msg_`).
fn from_event(ty: &str, d: &Value) -> Option<Notice> {
    let id = d["eventID"].as_str()?.replacen("evt_", "msg_", 1);
    let kind = match ty {
        "session.agent.selected" => kind("agent-switched", d)?,
        "session.model.selected" => kind("model-switched", d)?,
        "session.moved" => kind("location-switched", d)?,
        "session.instructions.updated" => {
            d["text"].as_str()?;
            let sources = d["delta"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
            instructions(sources)
        }
        "session.synthetic" => kind("synthetic", d)?,
        _ => return None,
    };
    Some(Notice { id, kind })
}

/// Whether `ty` is an event that appends a notice.
pub fn is_notice_event(ty: &str) -> bool {
    matches!(ty, "session.agent.selected" | "session.model.selected" | "session.moved" | "session.instructions.updated" | "session.synthetic")
}

impl Transcript {
    /// Applies a notice event; returns whether it added one.
    pub(super) fn apply_notice(&mut self, ty: &str, d: &Value) -> bool {
        let Some(n) = from_event(ty, d) else { return false };
        if self.messages.iter().any(|m| m.id() == n.id) {
            return false;
        }
        self.messages.push(Message::Notice(n));
        true
    }

    /// The run ended (`session.execution.*`): its idle marker, under the id its message takes.
    pub(super) fn apply_idle(&mut self, d: &Value) -> bool {
        let Some(id) = d["eventID"].as_str().map(|e| e.replacen("evt_", "msg_", 1)) else { return false };
        if self.messages.iter().any(|m| m.id() == id) {
            return false;
        }
        // `session.execution.failed` carries the error its idle message records.
        let failure = StepError::parse(&d["error"]);
        self.messages.push(Message::Notice(Notice { id, kind: NoticeKind::Idle { failure } }));
        true
    }

    /// The running step's retry, if the server is waiting to try it again.
    pub fn retrying(&self) -> Option<&Retry> {
        self.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        })?.retry.as_ref()
    }
}
