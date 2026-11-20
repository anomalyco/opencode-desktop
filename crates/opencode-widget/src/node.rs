//! The view tree a step returns (the contract's "Nodes"): every node is `{ type, ...props }`.
//! Unknown props are ignored; an unknown node type reads as [`Node::Unknown`], which renders as
//! nothing. A known node with a missing or mistyped required prop fails the whole view, as the
//! contract counts an invalid view as a failed step. Unknown values of an enumerated prop (a
//! tone, a style) fall back to its default rather than failing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Muted,
    Accent,
    Success,
    Warning,
    Error,
    #[default]
    #[serde(other)]
    Default,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextStyle {
    Title,
    Caption,
    Code,
    #[default]
    #[serde(other)]
    Body,
}

/// A column's cross-axis alignment (`stretch` by default, as a block lays out).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    #[serde(other)]
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ButtonVariant {
    Primary,
    Danger,
    #[default]
    #[serde(other)]
    Secondary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A select's value: one option's, or several when `multiple`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SelectValue {
    One(String),
    Many(Vec<String>),
}

impl SelectValue {
    /// Whether option `value` is picked.
    pub fn has(&self, value: &str) -> bool {
        match self {
            SelectValue::One(v) => v == value,
            SelectValue::Many(vs) => vs.iter().any(|v| v == value),
        }
    }

    /// The picked options' values.
    pub fn values(&self) -> Vec<&str> {
        match self {
            SelectValue::One(v) => vec![v.as_str()],
            SelectValue::Many(vs) => vs.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Node {
    Column {
        #[serde(default)]
        children: Vec<Node>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap: Option<u8>,
        #[serde(default)]
        align: Align,
    },
    Row {
        #[serde(default)]
        children: Vec<Node>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gap: Option<u8>,
        /// `start` when absent; `stretch` is not a row alignment and reads as `start`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        align: Option<Align>,
        #[serde(default)]
        wrap: bool,
    },
    Text {
        text: String,
        #[serde(default)]
        style: TextStyle,
        #[serde(default)]
        tone: Tone,
    },
    Markdown {
        text: String,
    },
    Image {
        src: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
        /// Width over height.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aspect: Option<f64>,
    },
    Carousel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(default)]
        children: Vec<Node>,
    },
    Card {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        selected: bool,
        #[serde(default)]
        children: Vec<Node>,
    },
    Button {
        id: String,
        label: String,
        #[serde(default)]
        variant: ButtonVariant,
        #[serde(default)]
        disabled: bool,
    },
    Checkbox {
        id: String,
        label: String,
        checked: bool,
        #[serde(default)]
        disabled: bool,
    },
    Slider {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        value: f64,
        min: f64,
        max: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
    },
    Select {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        options: Vec<SelectOption>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<SelectValue>,
        #[serde(default)]
        multiple: bool,
    },
    Input {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(default)]
        multiline: bool,
    },
    Divider,
    /// A node type this client doesn't know: drawn as nothing (with a warning in development).
    #[serde(other)]
    Unknown,
}

impl Node {
    /// The node's children, for the containers that have them.
    pub fn children(&self) -> &[Node] {
        match self {
            Node::Column { children, .. } | Node::Row { children, .. } | Node::Carousel { children, .. } | Node::Card { children, .. } => children,
            _ => &[],
        }
    }

    /// The id events from this node carry, when it emits any.
    pub fn id(&self) -> Option<&str> {
        match self {
            Node::Button { id, .. } | Node::Checkbox { id, .. } | Node::Slider { id, .. } | Node::Select { id, .. } | Node::Input { id, .. } => Some(id),
            Node::Carousel { id, .. } | Node::Card { id, .. } => id.as_deref(),
            _ => None,
        }
    }

    /// Reads a view from a step's JSON.
    pub fn from_value(v: &Value) -> Result<Node, String> {
        Node::deserialize(v).map_err(|e| format!("invalid view: {e}"))
    }

    /// Calls `f` on this node and every node under it, depth first.
    pub fn walk(&self, f: &mut dyn FnMut(&Node)) {
        f(self);
        for c in self.children() {
            c.walk(f);
        }
    }

    /// Words a reader or a search finds in the view: texts, labels and options, in order.
    pub fn plain_text(&self) -> String {
        let mut out = Vec::new();
        self.walk(&mut |n| match n {
            Node::Text { text, .. } | Node::Markdown { text } => out.push(text.clone()),
            Node::Button { label, .. } | Node::Checkbox { label, .. } => out.push(label.clone()),
            Node::Slider { label: Some(label), .. } | Node::Select { label: Some(label), .. } | Node::Input { label: Some(label), .. } => out.push(label.clone()),
            Node::Image { alt: Some(alt), .. } => out.push(alt.clone()),
            _ => {}
        });
        out.join("\n")
    }
}
