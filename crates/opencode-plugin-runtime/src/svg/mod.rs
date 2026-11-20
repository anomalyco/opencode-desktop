//! Strict hostile-input SVG sanitizer and viewBox normalizer for plugin-declared icons
//! (`icons/<name>.svg`).
//!
//! Treats every plugin SVG as untrusted input: enforces hard caps on byte length, nesting depth,
//! element count, shape count, path length/commands, and SMIL animation tracks; restricts markup
//! to safe vector geometry and the SMIL animation elements animated icons need (`animate`,
//! `animateTransform`, `set`); forbids external references, `<script>`, `<foreignObject>`,
//! `<image>`, `<use>`, `<filter>`, and event handlers; and re-emits canonical escaped SVG body
//! markup normalized onto a square `0 0 view view` canvas with root presentation attributes
//! preserved on an outer `<g>`.

mod validate;

use std::fmt::Write as _;

use validate::{validate_anim, validate_group, validate_root, validate_shape};

/// Hard caps protecting the UI thread and `IconPool` from hostile or oversized plugin SVGs.
pub const MAX_SVG_BYTES: usize = 32 * 1024;
pub const MAX_DEPTH: usize = 16;
pub const MAX_ELEMENTS: usize = 128;
pub const MAX_SHAPES: usize = 64;
pub const MAX_ANIMATIONS: usize = 16;
pub const MAX_PATH_BYTES: usize = 16 * 1024;
pub const MAX_PATH_COMMANDS: usize = 1024;
pub const MAX_POINTS_NUMBERS: usize = 1024;
pub const MAX_KEYFRAMES: usize = 64;
pub(super) const MAX_TRANSFORM_OPS: usize = 8;
pub(super) const MIN_VIEW_DIM: f32 = 1.0;
pub(super) const MAX_VIEW_DIM: f32 = 4096.0;
pub(super) const MIN_DUR_SECS: f32 = 0.016;
pub(super) const MAX_CLOCK_SECS: f32 = 3600.0;

/// Canonicalized safe icon ready for [`crate::icons::intern_icon`].
#[derive(Clone, Debug)]
pub struct SanitizedIcon {
    pub view: f32,
    pub body: String,
    pub animated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Svg,
    Group,
    Shape(ShapeKind),
    Anim(AnimKind),
    Skip,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ShapeKind {
    Path,
    Circle,
    Ellipse,
    Rect,
    Line,
    Polyline,
    Polygon,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AnimKind {
    Animate,
    AnimateTransform,
    Set,
}

pub(super) const PRESENTATION_ATTRS: [&str; 10] = [
    "fill",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "fill-rule",
    "opacity",
    "fill-opacity",
    "stroke-opacity",
];

/// Validates and canonicalizes a plugin-supplied SVG document.
/// Returns `Err(reason)` when the input is malformed, oversized, or uses disallowed constructs.
pub fn sanitize(svg: &str) -> Result<SanitizedIcon, &'static str> {
    if svg.is_empty() {
        return Err("empty SVG");
    }
    if svg.len() > MAX_SVG_BYTES {
        return Err("SVG exceeds 32 KiB limit");
    }
    if svg.as_bytes().contains(&0) {
        return Err("SVG contains NUL byte");
    }
    let cleaned = strip_comments_and_prolog(svg)?;
    let mut parser = Scanner::new(&cleaned);
    let mut stack: Vec<(&str, TagKind)> = Vec::new();
    let mut elements = 0usize;
    let mut shapes = 0usize;
    let mut animations = 0usize;
    let mut saw_root = false;
    let mut view_side = 16.0f32;
    let mut wrap_open = false;
    let mut body = String::with_capacity(cleaned.len().min(4096));

    while let Some(tok) = parser.next_token()? {
        match tok {
            Token::Text(txt) => {
                if !matches!(stack.last(), Some((_, TagKind::Skip))) && !txt.trim().is_empty() {
                    return Err("unexpected text content in SVG");
                }
            }
            Token::Close(name) => {
                let Some((open_name, kind)) = stack.pop() else {
                    return Err("unmatched closing tag");
                };
                if open_name != name {
                    return Err("mismatched closing tag");
                }
                match kind {
                    TagKind::Svg => {
                        if wrap_open {
                            body.push_str("</g>");
                            wrap_open = false;
                        }
                    }
                    TagKind::Skip => {}
                    TagKind::Group | TagKind::Shape(_) | TagKind::Anim(_) => {
                        let _ = write!(body, "</{name}>");
                    }
                }
            }
            Token::Open { name, attrs, self_closing } => {
                if matches!(stack.last(), Some((_, TagKind::Skip))) {
                    return Err("nested element inside metadata tag");
                }
                let kind = classify_tag(name)?;
                if !saw_root {
                    if kind != TagKind::Svg {
                        return Err("root element must be <svg>");
                    }
                    saw_root = true;
                } else if stack.is_empty() {
                    return Err("trailing content after </svg>");
                } else if kind == TagKind::Svg {
                    return Err("nested <svg> is not allowed");
                }

                elements += 1;
                if elements > MAX_ELEMENTS {
                    return Err("SVG exceeds element limit");
                }
                if stack.len() + usize::from(!self_closing) > MAX_DEPTH {
                    return Err("SVG exceeds nesting depth limit");
                }

                match kind {
                    TagKind::Svg => {
                        let (side, tx, ty, root_pres) = validate_root(&attrs)?;
                        view_side = side;
                        if !self_closing {
                            let need_wrap = tx.abs() > 1e-4 || ty.abs() > 1e-4 || !root_pres.is_empty();
                            if need_wrap {
                                body.push_str("<g");
                                if tx.abs() > 1e-4 || ty.abs() > 1e-4 {
                                    let _ = write!(body, r#" transform="translate({tx} {ty})""#);
                                }
                                emit_attrs(&mut body, &root_pres);
                                body.push('>');
                                wrap_open = true;
                            }
                            stack.push((name, kind));
                        }
                    }
                    TagKind::Skip => {
                        if !self_closing {
                            stack.push((name, kind));
                        }
                    }
                    TagKind::Group => {
                        let kept = validate_group(&attrs)?;
                        body.push_str("<g");
                        emit_attrs(&mut body, &kept);
                        if self_closing {
                            body.push_str("/>");
                        } else {
                            body.push('>');
                            stack.push((name, kind));
                        }
                    }
                    TagKind::Shape(sk) => {
                        shapes += 1;
                        if shapes > MAX_SHAPES {
                            return Err("SVG exceeds shape limit");
                        }
                        let kept = validate_shape(sk, &attrs)?;
                        let _ = write!(body, "<{name}");
                        emit_attrs(&mut body, &kept);
                        if self_closing {
                            body.push_str("/>");
                        } else {
                            body.push('>');
                            stack.push((name, kind));
                        }
                    }
                    TagKind::Anim(ak) => {
                        if !matches!(stack.last(), Some((_, TagKind::Group | TagKind::Shape(_)))) {
                            return Err("animation element must sit inside a shape or group");
                        }
                        animations += 1;
                        if animations > MAX_ANIMATIONS {
                            return Err("SVG exceeds animation limit");
                        }
                        let kept = validate_anim(ak, &attrs)?;
                        let _ = write!(body, "<{name}");
                        emit_attrs(&mut body, &kept);
                        if self_closing {
                            body.push_str("/>");
                        } else {
                            body.push('>');
                            stack.push((name, kind));
                        }
                    }
                }
            }
        }
    }

    if !saw_root || !stack.is_empty() {
        return Err("incomplete SVG document");
    }
    if shapes == 0 {
        return Err("SVG draws no geometry");
    }
    Ok(SanitizedIcon { view: view_side, body, animated: animations > 0 })
}

fn classify_tag(name: &str) -> Result<TagKind, &'static str> {
    match name {
        "svg" => Ok(TagKind::Svg),
        "g" => Ok(TagKind::Group),
        "path" => Ok(TagKind::Shape(ShapeKind::Path)),
        "circle" => Ok(TagKind::Shape(ShapeKind::Circle)),
        "ellipse" => Ok(TagKind::Shape(ShapeKind::Ellipse)),
        "rect" => Ok(TagKind::Shape(ShapeKind::Rect)),
        "line" => Ok(TagKind::Shape(ShapeKind::Line)),
        "polyline" => Ok(TagKind::Shape(ShapeKind::Polyline)),
        "polygon" => Ok(TagKind::Shape(ShapeKind::Polygon)),
        "animate" => Ok(TagKind::Anim(AnimKind::Animate)),
        "animateTransform" => Ok(TagKind::Anim(AnimKind::AnimateTransform)),
        "set" => Ok(TagKind::Anim(AnimKind::Set)),
        "title" | "desc" => Ok(TagKind::Skip),
        _ => Err("disallowed SVG element"),
    }
}

fn strip_comments_and_prolog(src: &str) -> Result<String, &'static str> {
    let mut out = String::with_capacity(src.len());
    let mut rest = src.trim_start_matches('\u{feff}');
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        if after.starts_with("<!--") {
            let Some(end) = after[4..].find("-->") else {
                return Err("unterminated XML comment");
            };
            rest = &after[4 + end + 3..];
            continue;
        }
        if after.starts_with("<!") {
            return Err("XML DTD or CDATA is not allowed");
        }
        if after.starts_with("<?") {
            let Some(end) = after[2..].find("?>") else {
                return Err("unterminated processing instruction");
            };
            let pi = &after[2..2 + end];
            if !pi.starts_with("xml") || !out.trim().is_empty() {
                return Err("XML processing instruction is not allowed");
            }
            rest = &after[2 + end + 2..];
            continue;
        }
        out.push('<');
        rest = &after[1..];
    }
    out.push_str(rest);
    Ok(out)
}

enum Token<'a> {
    Open { name: &'a str, attrs: Vec<(&'a str, String)>, self_closing: bool },
    Close(&'a str),
    Text(&'a str),
}

struct Scanner<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn next_token(&mut self) -> Result<Option<Token<'a>>, &'static str> {
        if self.pos >= self.src.len() {
            return Ok(None);
        }
        let rem = &self.src[self.pos..];
        if !rem.starts_with('<') {
            let end = rem.find('<').unwrap_or(rem.len());
            self.pos += end;
            return Ok(Some(Token::Text(&rem[..end])));
        }
        if rem.starts_with("</") {
            let Some(close) = rem.find('>') else {
                return Err("unterminated closing tag");
            };
            let name = rem[2..close].trim();
            if !is_xml_name(name) {
                return Err("invalid closing tag name");
            }
            self.pos += close + 1;
            return Ok(Some(Token::Close(name)));
        }
        // Scan opening tag respecting quoted attribute values.
        let bytes = rem.as_bytes();
        let mut i = 1usize;
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
            i += 1;
        }
        let name = &rem[name_start..i];
        if !is_xml_name(name) {
            return Err("invalid opening tag name");
        }
        let mut attrs = Vec::new();
        loop {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                return Err("unterminated opening tag");
            }
            if bytes[i] == b'>' {
                self.pos += i + 1;
                return Ok(Some(Token::Open { name, attrs, self_closing: false }));
            }
            if bytes[i] == b'/' {
                if bytes.get(i + 1) != Some(&b'>') {
                    return Err("malformed self-closing tag");
                }
                self.pos += i + 2;
                return Ok(Some(Token::Open { name, attrs, self_closing: true }));
            }
            let attr_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'>' && bytes[i] != b'/' {
                i += 1;
            }
            let attr_name = &rem[attr_start..i];
            if !is_xml_name(attr_name) {
                return Err("invalid attribute name");
            }
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if bytes.get(i) != Some(&b'=') {
                return Err("expected '=' after attribute name");
            }
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let Some(&quote) = bytes.get(i).filter(|&&b| b == b'"' || b == b'\'') else {
                return Err("attribute value must be quoted");
            };
            i += 1;
            let val_start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            if i >= bytes.len() {
                return Err("unterminated attribute value");
            }
            let raw_val = &rem[val_start..i];
            i += 1;
            if attrs.len() >= 32 {
                return Err("too many attributes on SVG element");
            }
            if attrs.iter().any(|(k, _)| *k == attr_name) {
                return Err("duplicate attribute");
            }
            attrs.push((attr_name, decode_entities(raw_val)?));
        }
    }
}

fn is_xml_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}

fn decode_entities(raw: &str) -> Result<String, &'static str> {
    if !raw.contains('&') {
        return Ok(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(semi) = after.find(';') else {
            return Err("malformed XML entity");
        };
        let ent = &after[..semi];
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => return Err("disallowed XML entity"),
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}


fn emit_attrs(out: &mut String, attrs: &[(&str, String)]) {
    for (k, v) in attrs {
        let _ = write!(out, r#" {k}="{v}""#);
    }
}
