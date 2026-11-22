//! A `canvas` node's drawing (the contract's "Canvas"): the [`Frame`] a draw step takes and the
//! compact draw [`Command`]s it returns, `["rect", x, y, w, h, paint]` and the like. Commands
//! are read straight from the step's JSON into flat Rust values (`f32` coordinates, colors parsed
//! once), so a renderer walks them without looking anything up.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;

use serde_json::{Map, Value, json};

/// The contract's per-step draw budget.
pub const DRAW_TIMEOUT_MS: u64 = 8;
/// The contract's per-frame command budget.
pub const MAX_COMMANDS: usize = 2_000;
/// Frames a canvas may overrun its budget in a row before it stops animating.
pub const OVERRUNS_TO_FREEZE: u32 = 3;
/// The highest `fps` a canvas runs at, 2D or GPU.
pub const MAX_FPS: f64 = 60.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pointer {
    pub x: f64,
    pub y: f64,
    pub down: bool,
}

/// A rectangle in a node's logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// This rect (in the coordinates `origin` is in) over a node at `origin`, `size` big: in the
    /// node's own coordinates, or `None` when it doesn't overlap the node.
    pub fn over(self, origin: [f64; 2], size: [f64; 2]) -> Option<Rect> {
        let r = Rect { x: self.x - origin[0], y: self.y - origin[1], ..self };
        (r.x < size[0] && r.y < size[1] && r.x + r.width > 0.0 && r.y + r.height > 0.0).then_some(r)
    }
}

/// What a draw step knows of the frame it draws.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// The canvas node's id.
    pub id: Option<String>,
    /// Seconds since the canvas appeared.
    pub t: f64,
    /// Seconds since the previous frame.
    pub dt: f64,
    pub width: f64,
    pub height: f64,
    /// Device pixels per logical pixel.
    pub scale: f64,
    pub pointer: Option<Pointer>,
    pub dark: bool,
    /// The widget is full screen.
    #[serde(default)]
    pub fullscreen: bool,
    /// Full screen, the close button's area over this canvas, in its coordinates
    /// (`widgets-fullscreen.md`); `None` when it doesn't overlap the canvas.
    #[serde(default)]
    pub close: Option<Rect>,
}

/// A theme color a command names instead of a value; the client resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Text,
    Muted,
    Accent,
    Bg,
    Surface,
    Border,
    Success,
    Warning,
    Error,
}

impl Token {
    fn name(self) -> &'static str {
        match self {
            Token::Text => "text",
            Token::Muted => "muted",
            Token::Accent => "accent",
            Token::Bg => "bg",
            Token::Surface => "surface",
            Token::Border => "border",
            Token::Success => "success",
            Token::Warning => "warning",
            Token::Error => "error",
        }
    }
}

/// `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"` (straight alpha), or a theme token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Rgba([u8; 4]),
    Token(Token),
}

impl Color {
    pub fn parse(s: &str) -> Option<Color> {
        let Some(hex) = s.strip_prefix('#') else {
            return Some(Color::Token(match s {
                "text" => Token::Text,
                "muted" => Token::Muted,
                "accent" => Token::Accent,
                "bg" => Token::Bg,
                "surface" => Token::Surface,
                "border" => Token::Border,
                "success" => Token::Success,
                "warning" => Token::Warning,
                "error" => Token::Error,
                _ => return None,
            }));
        };
        let digit = |i: usize| hex.as_bytes().get(i).and_then(|c| (*c as char).to_digit(16)).map(|d| d as u8);
        let pair = |i: usize| Some(digit(i)? * 16 + digit(i + 1)?);
        match hex.len() {
            3 => Some(Color::Rgba([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255])),
            6 => Some(Color::Rgba([pair(0)?, pair(2)?, pair(4)?, 255])),
            8 => Some(Color::Rgba([pair(0)?, pair(2)?, pair(4)?, pair(6)?])),
            _ => None,
        }
    }

    fn to_value(self) -> Value {
        match self {
            Color::Rgba([r, g, b, a]) => Value::String(format!("#{r:02x}{g:02x}{b:02x}{a:02x}")),
            Color::Token(t) => Value::String(t.name().into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientShape {
    /// From (x1, y1) to (x2, y2).
    Linear([f32; 4]),
    /// Center (cx, cy) and radius.
    Radial([f32; 3]),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub shape: GradientShape,
    /// Offsets in 0..=1 with their colors, in order.
    pub stops: Vec<(f32, Color)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Color(Color),
    Gradient(Gradient),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cap {
    #[default]
    Butt,
    Round,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paint {
    pub fill: Option<Fill>,
    pub stroke: Option<Color>,
    /// The stroke's width (1 when unset).
    pub width: f32,
    pub cap: Cap,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    M(f32, f32),
    L(f32, f32),
    Q(f32, f32, f32, f32),
    C(f32, f32, f32, f32, f32, f32),
    Z,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextOptions {
    /// Logical pixels (13 when unset).
    pub size: f32,
    /// 400, 500, 600 or 700.
    pub weight: u16,
    pub align: TextAlign,
    /// The "text" token when unset.
    pub color: Option<Color>,
    pub mono: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Save,
    Restore,
    Translate(f32, f32),
    Rotate(f32),
    Scale(f32, f32),
    Alpha(f32),
    Rect { x: f32, y: f32, w: f32, h: f32, paint: Paint },
    RRect { x: f32, y: f32, w: f32, h: f32, r: f32, paint: Paint },
    Circle { cx: f32, cy: f32, r: f32, paint: Paint },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, paint: Paint },
    Path { segments: Vec<Segment>, paint: Paint },
    /// `x` is where `align` puts the text; `y` its baseline.
    Text { x: f32, y: f32, text: String, style: TextOptions },
    Image { src: String, x: f32, y: f32, w: f32, h: f32 },
}

/// A JSON-shaped value draw commands are read from: a `serde_json::Value`, or (feature `runner`)
/// the VM's `HostValue` read as its JSON would be, so both give the same commands and errors.
pub(crate) trait Arg {
    /// A number (`Value::as_f64`).
    fn number(&self) -> Option<f64>;
    fn text(&self) -> Option<Cow<'_, str>>;
    fn flag(&self) -> Option<bool>;
    fn is_null(&self) -> bool;
    /// An array's length.
    fn len(&self) -> Option<usize>;
    /// An array's item (a hole is null).
    fn at(&self, i: usize) -> Option<&Self>;
    /// An object's member.
    fn field(&self, key: &str) -> Option<&Self>;
    fn is_object(&self) -> bool;
}

impl Arg for Value {
    fn number(&self) -> Option<f64> {
        self.as_f64()
    }
    fn text(&self) -> Option<Cow<'_, str>> {
        self.as_str().map(Cow::Borrowed)
    }
    fn flag(&self) -> Option<bool> {
        self.as_bool()
    }
    fn is_null(&self) -> bool {
        Value::is_null(self)
    }
    fn len(&self) -> Option<usize> {
        self.as_array().map(Vec::len)
    }
    fn at(&self, i: usize) -> Option<&Self> {
        self.as_array()?.get(i)
    }
    fn field(&self, key: &str) -> Option<&Self> {
        self.as_object()?.get(key)
    }
    fn is_object(&self) -> bool {
        Value::is_object(self)
    }
}

fn num<A: Arg>(v: &A, what: &str) -> Result<f32, String> {
    match v.number() {
        Some(n) if n.is_finite() => Ok(n as f32),
        _ => Err(format!("{what} must be a finite number")),
    }
}

fn color<A: Arg>(v: &A) -> Result<Color, String> {
    v.text().as_deref().and_then(Color::parse).ok_or_else(|| "not a color".to_owned())
}

fn items<A: Arg>(a: &A) -> impl Iterator<Item = &A> {
    (0..a.len().unwrap_or(0)).filter_map(|i| a.at(i))
}

/// A gradient: `linear` and `radial` both checked when given (linear wins), then `stops`.
fn gradient<A: Arg>(v: &A) -> Result<Gradient, String> {
    let coords = |key: &str, n: usize| -> Result<Option<Vec<f32>>, String> {
        let Some(a) = v.field(key) else { return Ok(None) };
        if a.len() != Some(n) {
            return Err(format!("a {key} gradient takes {n} numbers"));
        }
        items(a).map(|x| num(x, key)).collect::<Result<Vec<_>, _>>().map(Some)
    };
    let shape = match (coords("linear", 4)?, coords("radial", 3)?) {
        (Some(c), _) => GradientShape::Linear([c[0], c[1], c[2], c[3]]),
        (None, Some(c)) => GradientShape::Radial([c[0], c[1], c[2]]),
        (None, None) => return Err("a gradient needs linear or radial".into()),
    };
    let stops = v.field("stops").filter(|s| s.len().is_some()).ok_or("a gradient needs stops")?;
    let stops = items(stops)
        .map(|s| match (s.len(), s.at(0), s.at(1)) {
            (Some(2), Some(o), Some(c)) => Ok((num(o, "a stop's offset")?.clamp(0.0, 1.0), color(c)?)),
            _ => Err("a stop is [offset, color]".to_owned()),
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Gradient { shape, stops })
}

/// A paint's fields, in the spec's order: `fill`, `stroke`, `width`, `cap`. An absent (or
/// undefined) field takes its default; `null` is a value, and wrong.
fn paint<A: Arg>(v: &A) -> Result<Paint, String> {
    if !v.is_object() {
        return Err("a paint is an object".into());
    }
    let fill = v.field("fill").map(|f| if f.is_object() { gradient(f).map(Fill::Gradient) } else { color(f).map(Fill::Color) }).transpose()?;
    let stroke = v.field("stroke").map(color).transpose()?;
    let width = v.field("width").map(|w| num(w, "a stroke's width").map(|w| w.max(0.0))).transpose()?.unwrap_or(1.0);
    let cap = match v.field("cap").map(|c| c.text()) {
        None => Cap::Butt,
        Some(c) => match c.as_deref() {
            Some("butt") => Cap::Butt,
            Some("round") => Cap::Round,
            _ => return Err(r#"a stroke's cap must be "butt" or "round""#.into()),
        },
    };
    Ok(Paint { fill, stroke, width, cap })
}

fn segment<A: Arg>(v: &A) -> Result<Segment, String> {
    let Some(len) = v.len() else { return Err("a path segment is an array".into()) };
    let verb = v.at(0).and_then(Arg::text).unwrap_or_default();
    let count = match verb.as_ref() {
        "M" | "L" => 2,
        "Q" => 4,
        "C" => 6,
        "Z" => 0,
        _ => return Err("unknown path segment".into()),
    };
    if len != count + 1 {
        return Err(format!("a \"{verb}\" path segment takes {count} coordinates"));
    }
    let n = |i: usize| v.at(i).map_or_else(|| Err("a path coordinate must be a finite number".to_owned()), |x| num(x, "a path coordinate"));
    Ok(match verb.as_ref() {
        "M" => Segment::M(n(1)?, n(2)?),
        "L" => Segment::L(n(1)?, n(2)?),
        "Q" => Segment::Q(n(1)?, n(2)?, n(3)?, n(4)?),
        "C" => Segment::C(n(1)?, n(2)?, n(3)?, n(4)?, n(5)?, n(6)?),
        _ => Segment::Z,
    })
}

/// A text's style: left off, undefined or `null` is `{}`. Its fields, in the spec's order: `size`,
/// `weight`, `align`, `color`, `mono`.
fn text_options<A: Arg>(v: Option<&A>) -> Result<TextOptions, String> {
    let style = v.filter(|v| !v.is_null());
    if style.is_some_and(|s| !s.is_object()) {
        return Err("a text style is an object".into());
    }
    let get = |k: &str| style.and_then(|s| s.field(k));
    Ok(TextOptions {
        size: get("size").map(|s| num(s, "a text size")).transpose()?.unwrap_or(13.0).clamp(1.0, 512.0),
        weight: match get("weight").map(Arg::number) {
            None => 400,
            Some(Some(w)) if [400.0, 500.0, 600.0, 700.0].contains(&w) => w as u16,
            Some(_) => return Err("a text weight must be 400, 500, 600 or 700".into()),
        },
        align: match get("align").map(|a| a.text()) {
            None => TextAlign::Left,
            Some(a) => match a.as_deref() {
                Some("left") => TextAlign::Left,
                Some("center") => TextAlign::Center,
                Some("right") => TextAlign::Right,
                _ => return Err(r#"a text align must be "left", "center" or "right""#.into()),
            },
        },
        color: get("color").map(color).transpose()?,
        mono: get("mono").map(|m| m.flag().ok_or("a text mono must be true or false")).transpose()?.unwrap_or(false),
    })
}

/// One compact command, from a `Value` or (feature `runner`) a `HostValue`, checked as the spec's
/// Draw command checks say: its name, its length (too many, then too few), then its arguments in
/// order.
pub(crate) fn command<A: Arg>(a: &A) -> Result<Command, String> {
    let Some(len) = a.len() else { return Err("a command is an array".into()) };
    let op = a.at(0).and_then(Arg::text).ok_or("a command starts with its name")?;
    // Its arguments after the name, and how many of them may be left off (a text's style).
    let (count, optional) = match op.as_ref() {
        "save" | "restore" => (0, 0),
        "rotate" | "alpha" => (1, 0),
        "translate" | "scale" | "path" => (2, 0),
        "circle" => (4, 0),
        "text" => (4, 1),
        "rect" | "line" | "image" => (5, 0),
        "rrect" => (6, 0),
        _ => return Err(format!("unknown draw command \"{op}\"")),
    };
    if len > count + 1 {
        return Err(format!("\"{op}\" has more than {count} arguments"));
    }
    if len < count + 1 - optional {
        return Err(format!("\"{op}\" is missing argument {len}"));
    }
    let at = |i: usize| a.at(i).ok_or_else(|| format!("\"{op}\" is missing argument {i}"));
    let n = |i: usize| at(i).and_then(|v| num(v, &format!("\"{op}\" argument {i}")));
    let p = |i: usize| at(i).and_then(paint);
    Ok(match op.as_ref() {
        "save" => Command::Save,
        "restore" => Command::Restore,
        "translate" => Command::Translate(n(1)?, n(2)?),
        "rotate" => Command::Rotate(n(1)?),
        "scale" => Command::Scale(n(1)?, n(2)?),
        "alpha" => Command::Alpha(n(1)?.clamp(0.0, 1.0)),
        "rect" => Command::Rect { x: n(1)?, y: n(2)?, w: n(3)?, h: n(4)?, paint: p(5)? },
        "rrect" => Command::RRect { x: n(1)?, y: n(2)?, w: n(3)?, h: n(4)?, r: n(5)?, paint: p(6)? },
        "circle" => Command::Circle { cx: n(1)?, cy: n(2)?, r: n(3)?, paint: p(4)? },
        "line" => Command::Line { x1: n(1)?, y1: n(2)?, x2: n(3)?, y2: n(4)?, paint: p(5)? },
        "path" => Command::Path {
            segments: {
                let s = at(1)?;
                if s.len().is_none() {
                    return Err("\"path\" takes a list of segments".into());
                }
                items(s).map(segment).collect::<Result<_, _>>()?
            },
            paint: p(2)?,
        },
        "text" => Command::Text {
            x: n(1)?,
            y: n(2)?,
            text: at(3)?.text().ok_or("\"text\" argument 3 must be a string")?.into_owned(),
            style: text_options(a.at(4))?,
        },
        _ => Command::Image {
            src: at(1)?.text().ok_or("\"image\" takes a src")?.into_owned(),
            x: n(2)?,
            y: n(3)?,
            w: n(4)?,
            h: n(5)?,
        },
    })
}

impl Command {
    /// Reads one compact command.
    pub fn from_value(v: &Value) -> Result<Command, String> {
        command(v)
    }

    /// The compact array the contract writes it as.
    pub fn to_value(&self) -> Value {
        fn paint(p: &Paint) -> Value {
            let mut m = Map::new();
            match &p.fill {
                Some(Fill::Color(c)) => {
                    m.insert("fill".into(), c.to_value());
                }
                Some(Fill::Gradient(g)) => {
                    let (key, coords): (&str, Vec<f32>) = match g.shape {
                        GradientShape::Linear(c) => ("linear", c.to_vec()),
                        GradientShape::Radial(c) => ("radial", c.to_vec()),
                    };
                    let stops: Vec<Value> = g.stops.iter().map(|(o, c)| json!([o, c.to_value()])).collect();
                    m.insert("fill".into(), json!({ key: coords, "stops": stops }));
                }
                None => {}
            }
            if let Some(s) = p.stroke {
                m.insert("stroke".into(), s.to_value());
            }
            if p.width != 1.0 {
                m.insert("width".into(), json!(p.width));
            }
            if p.cap == Cap::Round {
                m.insert("cap".into(), json!("round"));
            }
            Value::Object(m)
        }
        match self {
            Command::Save => json!(["save"]),
            Command::Restore => json!(["restore"]),
            Command::Translate(x, y) => json!(["translate", x, y]),
            Command::Rotate(r) => json!(["rotate", r]),
            Command::Scale(x, y) => json!(["scale", x, y]),
            Command::Alpha(a) => json!(["alpha", a]),
            Command::Rect { x, y, w, h, paint: p } => json!(["rect", x, y, w, h, paint(p)]),
            Command::RRect { x, y, w, h, r, paint: p } => json!(["rrect", x, y, w, h, r, paint(p)]),
            Command::Circle { cx, cy, r, paint: p } => json!(["circle", cx, cy, r, paint(p)]),
            Command::Line { x1, y1, x2, y2, paint: p } => json!(["line", x1, y1, x2, y2, paint(p)]),
            Command::Path { segments, paint: p } => {
                let segs: Vec<Value> = segments
                    .iter()
                    .map(|s| match *s {
                        Segment::M(x, y) => json!(["M", x, y]),
                        Segment::L(x, y) => json!(["L", x, y]),
                        Segment::Q(a, b, x, y) => json!(["Q", a, b, x, y]),
                        Segment::C(a, b, c, d, x, y) => json!(["C", a, b, c, d, x, y]),
                        Segment::Z => json!(["Z"]),
                    })
                    .collect();
                json!(["path", segs, paint(p)])
            }
            Command::Text { x, y, text, style } => {
                let mut m = Map::new();
                m.insert("size".into(), json!(style.size));
                m.insert("weight".into(), json!(style.weight));
                m.insert(
                    "align".into(),
                    json!(match style.align {
                        TextAlign::Left => "left",
                        TextAlign::Center => "center",
                        TextAlign::Right => "right",
                    }),
                );
                if let Some(c) = style.color {
                    m.insert("color".into(), c.to_value());
                }
                if style.mono {
                    m.insert("mono".into(), json!(true));
                }
                json!(["text", x, y, text, m])
            }
            Command::Image { src, x, y, w, h } => json!(["image", src, x, y, w, h]),
        }
    }
}

impl Serialize for Command {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Command::from_value(&Value::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// A draw step's return value: `{ commands }`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawOutput {
    pub commands: Vec<Command>,
}

impl DrawOutput {
    /// Reads a draw step's return value, held to the command budget.
    pub fn from_value(v: &Value) -> Result<DrawOutput, String> {
        commands(v.get("commands")).map(|commands| DrawOutput { commands })
    }

    /// [`DrawOutput::from_value`] on the list the VM handed over (`widget.draw`'s commands, or a
    /// draw step's `commands`), read as its JSON would be, without making it.
    #[cfg(feature = "runner")]
    pub fn from_host(list: Option<&psychoscript::HostValue>) -> Result<DrawOutput, String> {
        let commands = match list {
            // A command holding what JSON writes its own way (a Map, a Date) is read from its JSON.
            Some(l) if !crate::host_value::plain(l) => commands(l.to_json().as_ref()),
            l => commands(l),
        };
        commands.map(|commands| DrawOutput { commands })
    }
}

/// A draw's commands, held to the budget, from a `Value` or (feature `runner`) a `HostValue`.
fn commands<A: Arg>(list: Option<&A>) -> Result<Vec<Command>, String> {
    let list = list.filter(|l| l.len().is_some()).ok_or("draw commands must be a list")?;
    let n = list.len().unwrap_or(0);
    if n > MAX_COMMANDS {
        return Err(format!("{n} draw commands is over the budget of {MAX_COMMANDS} per frame"));
    }
    items(list).enumerate().map(|(i, c)| command(c).map_err(|e| format!("draw command {i}: {e}"))).collect()
}
