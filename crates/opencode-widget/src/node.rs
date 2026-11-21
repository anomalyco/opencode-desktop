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
        /// On the root: offer full screen (`true`), never (`false`); absent, offered for live
        /// widgets and views with a canvas or shader (`widgets-fullscreen.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fullscreen: Option<bool>,
        /// On the root: `"pointer"` asks for pointer capture, granted on the next click
        /// (`widgets-input.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<String>,
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
        /// On the root: offer full screen (`true`), never (`false`); absent, offered for live
        /// widgets and views with a canvas or shader (`widgets-fullscreen.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fullscreen: Option<bool>,
        /// On the root: `"pointer"` asks for pointer capture, granted on the next click
        /// (`widgets-input.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<String>,
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
        /// On the root: offer full screen (`true`), never (`false`); absent, offered for live
        /// widgets and views with a canvas or shader (`widgets-fullscreen.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fullscreen: Option<bool>,
        /// On the root: `"pointer"` asks for pointer capture, granted on the next click
        /// (`widgets-input.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<String>,
    },
    Card {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        selected: bool,
        #[serde(default)]
        children: Vec<Node>,
        /// On the root: offer full screen (`true`), never (`false`); absent, offered for live
        /// widgets and views with a canvas or shader (`widgets-fullscreen.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fullscreen: Option<bool>,
        /// On the root: `"pointer"` asks for pointer capture, granted on the next click
        /// (`widgets-input.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<String>,
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
    /// Drawn every frame by the widget template's `draw` (see [`crate::draw`]); full width unless `width`.
    Canvas {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        height: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        /// 0 (draw on change only) when absent, at most 60.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f64>,
        /// Seconds: animates only until it drew a frame with `t >= duration`, then on changes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
        /// In full screen it grows to share the remaining height (`height` is the minimum) unless
        /// `false`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<bool>,
        /// On the root: offer full screen (`true`), never (`false`); absent, offered for live
        /// widgets and views with a canvas or shader (`widgets-fullscreen.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fullscreen: Option<bool>,
        /// On the root: `"pointer"` asks for pointer capture, granted on the next click
        /// (`widgets-input.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<String>,
    },
    /// A WGSL fragment shader (see [`crate::shader`]); full width unless `width`.
    Shader {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        height: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        wgsl: String,
        /// Up to 16 floats for `widget.values`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<f64>,
        /// 60 when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        poster: Option<String>,
        /// In full screen, grows to take the remaining height (`height` is the minimum).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<bool>,
        /// On the root: offer full screen (`true`), never (`false`); absent, offered for live
        /// widgets and views with a canvas or shader (`widgets-fullscreen.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fullscreen: Option<bool>,
        /// On the root: `"pointer"` asks for pointer capture, granted on the next click
        /// (`widgets-input.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<String>,
    },
    /// A node type this client doesn't know: drawn as nothing (with a warning in development).
    #[serde(other)]
    Unknown,
}

impl Node {
    /// Whether `other` is this view with only its shaders' `values` changed: the shaders at those
    /// paths (as renderers key them, `w`, `w.0`, `w.0.1`…) and their new values, when it is.
    pub fn values_only(&self, other: &Node) -> Option<Vec<(String, Vec<f64>)>> {
        fn walk(a: &Node, b: &Node, path: &str, out: &mut Vec<(String, Vec<f64>)>) -> bool {
            match (a, b) {
                (Node::Shader { values: va, .. }, Node::Shader { values: vb, .. }) => {
                    let mut bare = b.clone();
                    if let Node::Shader { values, .. } = &mut bare {
                        values.clone_from(va);
                    }
                    if &bare != a {
                        return false;
                    }
                    if va != vb {
                        out.push((path.to_owned(), vb.clone()));
                    }
                    true
                }
                _ => {
                    let (ka, kb) = (a.children(), b.children());
                    if ka.len() != kb.len() || std::mem::discriminant(a) != std::mem::discriminant(b) {
                        return false;
                    }
                    if ka.is_empty() {
                        return a == b;
                    }
                    // The node itself, without its children, must match too.
                    if !same_shell(a, b) {
                        return false;
                    }
                    ka.iter().zip(kb).enumerate().all(|(i, (x, y))| walk(x, y, &format!("{path}.{i}"), out))
                }
            }
        }
        let mut out = Vec::new();
        walk(self, other, "w", &mut out).then_some(out)
    }

    /// Whether this view, as the root, asks for pointer capture (`capture: "pointer"`).
    pub fn wants_capture(&self) -> bool {
        match self {
            Node::Column { capture, .. } | Node::Row { capture, .. } | Node::Carousel { capture, .. } | Node::Card { capture, .. } | Node::Canvas { capture, .. } | Node::Shader { capture, .. } => capture.as_deref() == Some("pointer"),
            _ => false,
        }
    }

    /// Whether this view, as the root, offers full screen: its `fullscreen` prop, else for a
    /// live widget or a view with a canvas or shader.
    pub fn offers_fullscreen(&self, live: bool) -> bool {
        let own = match self {
            Node::Column { fullscreen, .. } | Node::Row { fullscreen, .. } | Node::Carousel { fullscreen, .. } | Node::Card { fullscreen, .. } | Node::Canvas { fullscreen, .. } | Node::Shader { fullscreen, .. } => *fullscreen,
            _ => None,
        };
        own.unwrap_or_else(|| {
            let mut drawn = false;
            self.walk(&mut |n| drawn |= matches!(n, Node::Canvas { .. } | Node::Shader { .. }));
            live || drawn
        })
    }

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
            Node::Carousel { id, .. } | Node::Card { id, .. } | Node::Canvas { id, .. } | Node::Shader { id, .. } => id.as_deref(),
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
            Node::Image { alt: Some(alt), .. } | Node::Canvas { alt: Some(alt), .. } | Node::Shader { alt: Some(alt), .. } => out.push(alt.clone()),
            _ => {}
        });
        out.join("\n")
    }
}

/// Whether two containers match but for their children.
fn same_shell(a: &Node, b: &Node) -> bool {
    let strip = |n: &Node| {
        let mut n = n.clone();
        match &mut n {
            Node::Column { children, .. } | Node::Row { children, .. } | Node::Carousel { children, .. } | Node::Card { children, .. } => children.clear(),
            _ => {}
        }
        n
    };
    strip(a) == strip(b)
}
