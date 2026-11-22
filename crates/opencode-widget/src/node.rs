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
        /// widgets and views with a canvas (`widgets-fullscreen.md`).
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
        /// widgets and views with a canvas (`widgets-fullscreen.md`).
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
        /// widgets and views with a canvas (`widgets-fullscreen.md`).
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
        /// widgets and views with a canvas (`widgets-fullscreen.md`).
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
    /// A drawing surface, full width unless `width`, drawn one way, never both: in 2D by the
    /// widget template's `draw` (see [`crate::draw`]), or, with `wgsl`, on the GPU by that
    /// fragment shader (see [`crate::shader`]).
    Canvas {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        height: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        /// At most 60; when absent, 0 (draw on change only) in 2D, 60 on the GPU ([`Node::fps`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f64>,
        /// Seconds: animates only until it drew a frame with `t >= duration`, then on changes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
        /// An image src shown where it can't be drawn (a shader that fails to compile).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        poster: Option<String>,
        /// A GPU canvas: WGSL defining `fn shade(frag: vec2f) -> vec4f`, at most 16 KB.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wgsl: Option<String>,
        /// With `wgsl`, up to 16 floats for `widget.values`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<f64>,
        /// It takes input (the prelude sets it with `onPointer`): only then does it send
        /// `pointer` events, and in a stack take the taps over it. An id alone is for drawing.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pointer: bool,
        /// In full screen it grows to share the remaining height (`height` is the minimum) unless
        /// `false`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<bool>,
        /// On the root: offer full screen (`true`), never (`false`); absent, offered for live
        /// widgets and views with a canvas (`widgets-fullscreen.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fullscreen: Option<bool>,
        /// On the root: `"pointer"` asks for pointer capture, granted on the next click
        /// (`widgets-input.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<String>,
    },
    /// Children layered in order, the last on top, each where its [`Layer`] puts it
    /// (`widgets.md`, Stack).
    Stack {
        #[serde(default)]
        children: Layers,
    },
    /// A node type this client doesn't know: drawn as nothing (with a warning in development).
    #[serde(other)]
    Unknown,
}

impl Node {
    /// Whether `other` is this view with only its GPU canvases' `values` changed: the canvases at
    /// those paths (as renderers key them, `w`, `w.0`, `w.0.1`…) and their new values, when it is.
    pub fn values_only(&self, other: &Node) -> Option<Vec<(String, Vec<f64>)>> {
        fn walk(a: &Node, b: &Node, path: &str, out: &mut Vec<(String, Vec<f64>)>) -> bool {
            match (a, b) {
                (Node::Canvas { wgsl: Some(_), values: va, .. }, Node::Canvas { wgsl: Some(_), values: vb, .. }) => {
                    let mut bare = b.clone();
                    if let Node::Canvas { values, .. } = &mut bare {
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
                    // The node itself, without its children, must match too (a stack's layers).
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
            Node::Column { capture, .. } | Node::Row { capture, .. } | Node::Carousel { capture, .. } | Node::Card { capture, .. } | Node::Canvas { capture, .. } => capture.as_deref() == Some("pointer"),
            _ => false,
        }
    }

    /// Whether this view, as the root, offers full screen: its `fullscreen` prop, else for a
    /// live widget or a view with a canvas (a stack's included).
    pub fn offers_fullscreen(&self, live: bool) -> bool {
        let own = match self {
            Node::Column { fullscreen, .. } | Node::Row { fullscreen, .. } | Node::Carousel { fullscreen, .. } | Node::Card { fullscreen, .. } | Node::Canvas { fullscreen, .. } => *fullscreen,
            _ => None,
        };
        own.unwrap_or_else(|| {
            let mut drawn = false;
            self.walk(&mut |n| drawn |= matches!(n, Node::Canvas { .. }));
            live || drawn
        })
    }

    /// The node's children, for the containers that have them.
    pub fn children(&self) -> &[Node] {
        match self {
            Node::Column { children, .. } | Node::Row { children, .. } | Node::Carousel { children, .. } | Node::Card { children, .. } => children,
            Node::Stack { children } => &children.nodes,
            _ => &[],
        }
    }

    /// A canvas's frames per second: its `fps`, else 60 on the GPU and 0 in 2D; at most 60.
    pub fn fps(&self) -> f64 {
        match self {
            Node::Canvas { fps, wgsl, .. } => fps.unwrap_or(if wgsl.is_some() { crate::shader::DEFAULT_FPS } else { 0.0 }).clamp(0.0, crate::draw::MAX_FPS),
            _ => 0.0,
        }
    }

    /// The id events from this node carry, when it emits any.
    pub fn id(&self) -> Option<&str> {
        match self {
            Node::Button { id, .. } | Node::Checkbox { id, .. } | Node::Slider { id, .. } | Node::Select { id, .. } | Node::Input { id, .. } => Some(id),
            Node::Carousel { id, .. } | Node::Card { id, .. } | Node::Canvas { id, .. } => id.as_deref(),
            _ => None,
        }
    }

    /// Reads a view from a step's JSON: invalid when it doesn't parse, or a canvas has `values`
    /// without `wgsl` or more than 16 KB of it.
    pub fn from_value(v: &Value) -> Result<Node, String> {
        Node::deserialize(v).map_err(|e| format!("invalid view: {e}")).and_then(Node::checked)
    }

    /// [`Node::from_value`] on the view the VM handed over (`widget.render`'s node), read as its
    /// JSON would be, without making it.
    #[cfg(feature = "runner")]
    pub fn from_host(v: &psychoscript::HostValue) -> Result<Node, String> {
        crate::host_value::read::<Node>(v).map_err(|e| format!("invalid view: {e}")).and_then(Node::checked)
    }

    /// A view as read, or why it's invalid all the same: a canvas with `values` but no `wgsl`, or
    /// more than 16 KB of it.
    fn checked(node: Node) -> Result<Node, String> {
        let mut bad = None;
        node.walk(&mut |n| {
            if let Node::Canvas { wgsl, values, .. } = n {
                if wgsl.is_none() && !values.is_empty() {
                    bad.get_or_insert("a canvas's `values` need `wgsl`".to_owned());
                }
                if let Some(w) = wgsl.as_ref().filter(|w| w.len() > crate::shader::MAX_SOURCE) {
                    bad.get_or_insert(format!("a canvas's wgsl is {} bytes, over the limit of {}", w.len(), crate::shader::MAX_SOURCE));
                }
            }
        });
        match bad {
            Some(why) => Err(format!("invalid view: {why}")),
            None => Ok(node),
        }
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
            Node::Image { alt: Some(alt), .. } | Node::Canvas { alt: Some(alt), .. } => out.push(alt.clone()),
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
            Node::Stack { children } => children.nodes.clear(),
            _ => {}
        }
        n
    };
    strip(a) == strip(b)
}

/// Where a child of a `stack` sits in its area (`widgets.md`, Stack).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Place {
    #[default]
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
    /// The whole area: a canvas draws at its size.
    Fill,
}

/// Space between a stack's edges and a child, in logical pixels: one number, or per side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Inset {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl<'de> Deserialize<'de> for Inset {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            All(f64),
            Sides {
                #[serde(default)]
                top: f64,
                #[serde(default)]
                right: f64,
                #[serde(default)]
                bottom: f64,
                #[serde(default)]
                left: f64,
            },
        }
        let i = match Wire::deserialize(d)? {
            Wire::All(v) => Inset { top: v, right: v, bottom: v, left: v },
            Wire::Sides { top, right, bottom, left } => Inset { top, right, bottom, left },
        };
        if [i.top, i.right, i.bottom, i.left].iter().all(|v| v.is_finite() && *v >= 0.0) {
            Ok(i)
        } else {
            Err(serde::de::Error::custom("an inset is a finite number of logical pixels, at least 0"))
        }
    }
}

/// How a node sits as a child of a stack: any node may carry `place` and `inset`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    #[serde(default)]
    pub place: Place,
    #[serde(default)]
    pub inset: Inset,
}

/// A stack's children, bottom first, and how each sits (`place`, `inset` on the child itself).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layers {
    pub nodes: Vec<Node>,
    pub layers: Vec<Layer>,
}

impl<'de> Deserialize<'de> for Layers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Vec::<Value>::deserialize(d)?;
        let mut out = Layers::default();
        for v in raw {
            out.layers.push(Layer::deserialize(&v).map_err(serde::de::Error::custom)?);
            out.nodes.push(Node::deserialize(&v).map_err(serde::de::Error::custom)?);
        }
        Ok(out)
    }
}

impl Serialize for Layers {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut out = Vec::with_capacity(self.nodes.len());
        for (n, l) in self.nodes.iter().zip(&self.layers) {
            let mut v = serde_json::to_value(n).map_err(serde::ser::Error::custom)?;
            if let Some(m) = v.as_object_mut() {
                if l.place != Place::default() {
                    m.insert("place".into(), serde_json::to_value(l.place).map_err(serde::ser::Error::custom)?);
                }
                if l.inset != Inset::default() {
                    m.insert("inset".into(), serde_json::to_value(l.inset).map_err(serde::ser::Error::custom)?);
                }
            }
            out.push(v);
        }
        out.serialize(s)
    }
}
