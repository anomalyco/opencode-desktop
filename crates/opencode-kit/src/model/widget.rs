//! An agent widget in a session (`Session.Message.Widget`, anomalyco/opencode
//! `specs/v2/widgets.md`): a named template at a version, the JSON data it runs on, and whether
//! the user answered it. Every client runs the template's program itself; this is only the
//! message, kept current by `session.widget.posted` and `session.widget.responded`.

use serde_json::Value;

use super::{Message, i, s};

#[derive(Clone, Debug, PartialEq)]
pub enum WidgetStatus {
    Open,
    /// Answered with `response`.
    Submitted(Value),
    Dismissed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: String,
    pub title: Option<String>,
    /// The template's name and its content-addressed version.
    pub name: String,
    pub version: String,
    pub data: Value,
    pub status: WidgetStatus,
    pub created: i64,
    /// The assistant message that posted it, through its `widget` tool call.
    pub tool_message: Option<String>,
}

impl Widget {
    /// The name it goes by: its title, else its template's name (the server's `label`).
    pub fn label(&self) -> &str {
        self.title.as_deref().filter(|t| !t.is_empty()).unwrap_or(&self.name)
    }

    /// `name@version`: its program's cache key.
    pub fn template_key(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

fn status(v: &Value, response: &Value) -> WidgetStatus {
    match v.as_str() {
        Some("submitted") => WidgetStatus::Submitted(response.clone()),
        Some("dismissed") => WidgetStatus::Dismissed,
        _ => WidgetStatus::Open,
    }
}

/// A `widget` message from `/api/session/:id/message`.
pub(super) fn parse(m: &Value) -> Option<Message> {
    let name = m["template"]["name"].as_str()?.to_owned();
    Some(Message::Widget(Widget {
        id: s(&m["id"]),
        title: m["title"].as_str().map(str::to_owned),
        name,
        version: s(&m["template"]["version"]),
        data: m["data"].clone(),
        status: status(&m["status"], &m["response"]),
        created: i(&m["time"]["created"]),
        tool_message: m["tool"]["messageID"].as_str().map(str::to_owned),
    }))
}

/// Applies `session.widget.posted` / `session.widget.responded` to `messages`; whether anything
/// changed.
pub(super) fn apply(messages: &mut Vec<Message>, ty: &str, d: &Value, now: i64) -> bool {
    let id = s(&d["messageID"]);
    if id.is_empty() {
        return false;
    }
    match ty {
        "session.widget.posted" => {
            if messages.iter().any(|m| m.id() == id) {
                return false;
            }
            let Some(name) = d["template"]["name"].as_str() else { return false };
            let w = Widget {
                id,
                title: d["title"].as_str().map(str::to_owned),
                name: name.to_owned(),
                version: s(&d["template"]["version"]),
                data: d["data"].clone(),
                status: WidgetStatus::Open,
                created: d["time"]["created"].as_i64().unwrap_or(now),
                tool_message: d["tool"]["messageID"].as_str().map(str::to_owned),
            };
            messages.push(Message::Widget(w));
            true
        }
        "session.widget.responded" => {
            let Some(Message::Widget(w)) = messages.iter_mut().find(|m| m.id() == id) else { return false };
            let next = status(&d["status"], &d["response"]);
            if w.status == next {
                return false;
            }
            w.status = next;
            true
        }
        _ => false,
    }
}
