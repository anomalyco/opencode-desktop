//! The step ABI's values: what `__input()` returns ([`StepInput`]), the events a view emits
//! ([`WidgetEvent`]), what a step returns ([`StepOutput`]), and a response to an open widget
//! ([`Response`], the body of `POST /session/{sessionID}/widget/{messageID}/respond`).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::draw::Frame;
use crate::node::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Press,
    Change,
    /// A canvas's or shader's pointer: `value: { kind: "down" | "move" | "up", x, y, width, height }`.
    Pointer,
    /// The widget entered or left full screen: `{ id: "", value: true | false }`.
    Fullscreen,
}

/// What a canvas's pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerKind {
    Down,
    Move,
    Up,
}

impl PointerKind {
    fn name(self) -> &'static str {
        match self {
            PointerKind::Down => "down",
            PointerKind::Move => "move",
            PointerKind::Up => "up",
        }
    }
}

/// `{ id, type, value? }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

impl WidgetEvent {
    /// A `button` or `card` press.
    pub fn press(id: impl Into<String>) -> WidgetEvent {
        WidgetEvent { id: id.into(), kind: EventKind::Press, value: None }
    }

    /// A control's new value: a checkbox's bool, a slider's number, a select's string (or list),
    /// an input's text, a carousel's page.
    pub fn change(id: impl Into<String>, value: impl Into<Value>) -> WidgetEvent {
        WidgetEvent { id: id.into(), kind: EventKind::Change, value: Some(value.into()) }
    }

    /// A canvas's pointer at (`x`, `y`), logical pixels from its top-left, with the canvas's
    /// logical size so `update` can hit-test a layout that follows the width.
    /// The widget entered (`true`) or left full screen.
    pub fn fullscreen(on: bool) -> WidgetEvent {
        WidgetEvent { id: String::new(), kind: EventKind::Fullscreen, value: Some(Value::Bool(on)) }
    }

    pub fn pointer(id: impl Into<String>, kind: PointerKind, x: f64, y: f64, width: f64, height: f64) -> WidgetEvent {
        let value = json!({ "kind": kind.name(), "x": x, "y": y, "width": width, "height": height });
        WidgetEvent { id: id.into(), kind: EventKind::Pointer, value: Some(value) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum StepInput {
    Init { data: Value },
    Event { data: Value, state: Value, event: WidgetEvent },
    /// A canvas's frame: returns `{ commands }` (see [`crate::draw`]).
    Draw { data: Value, state: Value, frame: Frame },
}

/// Deserializes a field that is there, `null` included, as `Some`: `submit: null` submits null.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepOutput {
    #[serde(default)]
    pub state: Value,
    pub view: Node,
    /// Present: the user submitted this value.
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub submit: Option<Value>,
    /// Present (and true): the user dismissed the widget.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dismiss: bool,
    /// Present: a message to the agent; the widget stays open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<SendMessage>,
}

/// A non-final message to the agent (`send(value, state, { wake })`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendMessage {
    #[serde(default)]
    pub value: Value,
    /// Whether it starts the agent's turn (true unless the template said otherwise).
    #[serde(default = "yes")]
    pub wake: bool,
}

fn yes() -> bool {
    true
}

impl SendMessage {
    /// The message endpoint's body: `{ value, wake }`.
    pub fn body(&self) -> Value {
        json!({ "value": self.value, "wake": self.wake })
    }
}

impl StepOutput {
    /// Reads a step's return value, or why it isn't a valid step output.
    pub fn from_value(v: Value) -> Result<StepOutput, String> {
        if !v.is_object() {
            return Err(format!("a step must return an object with state and view, got {}", kind(&v)));
        }
        StepOutput::deserialize(v).map_err(|e| format!("invalid step output: {e}"))
    }

    /// What the user answered this step with, when it ends the widget.
    pub fn response(&self) -> Option<Response> {
        match (&self.submit, self.dismiss) {
            (Some(value), _) => Some(Response::Submit(value.clone())),
            (None, true) => Some(Response::Dismiss),
            (None, false) => None,
        }
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// How the user ended an open widget.
#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Submit(Value),
    Dismiss,
}

impl Response {
    /// The respond endpoint's body: `{ value }` or `{ dismiss: true }`.
    pub fn body(&self) -> Value {
        match self {
            Response::Submit(value) => json!({ "value": value }),
            Response::Dismiss => json!({ "dismiss": true }),
        }
    }
}
