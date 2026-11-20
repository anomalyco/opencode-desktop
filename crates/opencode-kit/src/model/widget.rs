//! Agent widgets in a session (anomalyco/opencode `specs/v2/widgets.md`): a widget is the
//! `widget` tool call in the history ([`WidgetCall`], input `{ template, data, title?, wait? }`),
//! identified by its assistant message and call id; the widgets plugin keeps its state
//! ([`WidgetState`], RPC `opencode.widget`: `states`, and the `changed` event). Every client runs
//! the template's program itself.

use serde_json::Value;

use super::{Message, Part, ToolStatus, i, s};

/// The plugin's event for a posted or answered widget: `{ sessionID, state }`.
pub const WIDGET_CHANGED_EVENT: &str = "rpc.opencode.widget.changed";

#[derive(Clone, Debug, PartialEq)]
pub enum WidgetStatus {
    Open,
    /// Answered with `response`.
    Submitted(Value),
    Dismissed,
    /// A newer widget replaced it (`replaces`): the timeline shows "Updated below".
    Superseded,
}

/// One widget as the plugin keeps it (`Widget.State`).
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetState {
    pub session: String,
    /// The assistant message holding the `widget` tool call.
    pub message: String,
    pub call: String,
    /// The template's name and its content-addressed version.
    pub name: String,
    pub version: String,
    pub title: Option<String>,
    pub status: WidgetStatus,
    /// The tool call waits for the answer.
    pub wait: bool,
    pub created: i64,
}

impl WidgetState {
    /// A `Widget.State` from `states` or `changed`.
    pub fn parse(v: &Value) -> Option<WidgetState> {
        let message = v["messageID"].as_str()?.to_owned();
        let call = v["callID"].as_str()?.to_owned();
        let name = v["template"]["name"].as_str()?.to_owned();
        let status = match v["status"].as_str() {
            Some("submitted") => WidgetStatus::Submitted(v["response"].clone()),
            Some("dismissed") => WidgetStatus::Dismissed,
            Some("superseded") => WidgetStatus::Superseded,
            _ => WidgetStatus::Open,
        };
        Some(WidgetState {
            session: s(&v["sessionID"]),
            message,
            call,
            name,
            version: s(&v["template"]["version"]),
            title: v["title"].as_str().filter(|t| !t.is_empty()).map(str::to_owned),
            status,
            wait: v["wait"].as_bool().unwrap_or(false),
            created: i(&v["time"]["created"]),
        })
    }

    /// The key a client looks it up by: [`widget_key`].
    pub fn key(&self) -> String {
        widget_key(&self.message, &self.call)
    }

    /// The name it goes by: its title, else its template's name (the server's `label`).
    pub fn label(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    /// `name@version`: its program's cache key.
    pub fn template_key(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

/// A widget's key: its message's id and its call's (a call id is unique only within its message).
pub fn widget_key(message: &str, call: &str) -> String {
    format!("{message}/{call}")
}

/// A `widget` tool call in the history.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetCall {
    pub message: String,
    pub call: String,
    pub status: ToolStatus,
    /// `{ template, data, title?, wait? }`.
    pub input: Value,
    /// `{ template: { name, version } }` once completed.
    pub metadata: Value,
}

impl WidgetCall {
    pub fn key(&self) -> String {
        widget_key(&self.message, &self.call)
    }

    /// The data its template runs on.
    pub fn data(&self) -> &Value {
        &self.input["data"]
    }
}

/// The `widget` tool calls of `messages`, oldest first.
pub fn widget_calls(messages: &[Message]) -> Vec<WidgetCall> {
    let mut out = Vec::new();
    for m in messages {
        let a = match m {
            Message::Assistant(a) => a,
            Message::Note { step, .. } => &**step,
            _ => continue,
        };
        for p in &a.parts {
            if let Part::Tool { id, name, status, input, metadata, .. } = p
                && name == "widget"
            {
                out.push(WidgetCall { message: a.id.clone(), call: id.clone(), status: status.clone(), input: input.clone(), metadata: metadata.clone() });
            }
        }
    }
    out
}
