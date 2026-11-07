//! Maps contract type strings to Rust types and to the host's prop decoders.

use super::super::model::{Contract, TypeKind};
use super::super::sig;

/// A record field as Rust sees it.
pub enum RustField {
    /// Plain data, with its Rust type.
    Data(String),
    /// A Luau function the host keeps as a reference.
    Callback,
    /// A host object or opaque value the host attaches itself.
    Handle,
}

pub fn field(c: &Contract, ty: &str) -> Result<RustField, String> {
    if sig::is_function(ty) {
        return Ok(RustField::Callback);
    }
    let (inner, optional) = sig::strip_optional(ty);
    if is_handle(c, inner) {
        return Ok(RustField::Handle);
    }
    let rust = data(c, inner)?;
    Ok(RustField::Data(if optional { format!("Option<{rust}>") } else { rust }))
}

fn is_handle(c: &Contract, name: &str) -> bool {
    c.type_def(name).is_some_and(|t| matches!(t.kind, TypeKind::Opaque | TypeKind::Object))
}

fn data(c: &Contract, ty: &str) -> Result<String, String> {
    let ty = sig::unwrap_parens(ty);
    if let Some(body) = ty.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        let body = body.trim();
        if let Some(value) = body.strip_prefix("[string]:") {
            return Ok(format!("BTreeMap<String, {}>", data(c, value.trim())?));
        }
        return Ok(format!("Vec<{}>", data(c, body)?));
    }
    Ok(match ty {
        "string" | "IconName" => "String".to_owned(),
        "number" => "f64".to_owned(),
        "integer" => "i64".to_owned(),
        "boolean" => "bool".to_owned(),
        "any" => "Value".to_owned(),
        "Color" => "ColorSpec".to_owned(),
        name => match (c.enum_def(name), c.type_def(name)) {
            (Some(e), _) if e.rust => name.to_owned(),
            (_, Some(t)) if t.rust && t.kind == TypeKind::Record => name.to_owned(),
            _ => return Err(format!("no Rust type for `{ty}`: mark it `rust` or keep it out of Rust records")),
        },
    })
}

/// The `PropType` expression the host decodes a `ui.*` prop with.
pub fn prop(c: &Contract, ty: &str) -> String {
    if sig::is_function(ty) {
        return "PropType::Function".to_owned();
    }
    let inner = sig::unwrap_parens(sig::strip_optional(ty).0);
    if let Some(body) = inner.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        let body = body.trim();
        if body.starts_with('[') {
            return "PropType::Map".to_owned();
        }
        if body == "UiNode" {
            return "PropType::Nodes".to_owned();
        }
        return format!("PropType::List(\"{body}\")");
    }
    match inner {
        "number" => "PropType::Number".to_owned(),
        "integer" => "PropType::Integer".to_owned(),
        "string" | "IconName" => "PropType::String".to_owned(),
        "boolean" => "PropType::Bool".to_owned(),
        "any" => "PropType::Any".to_owned(),
        "Color" => "PropType::Color".to_owned(),
        "UiSize" => "PropType::Size".to_owned(),
        "MemoVersion" => "PropType::MemoVersion".to_owned(),
        "UiHandler" => "PropType::Handler".to_owned(),
        "UiNode" => "PropType::Node".to_owned(),
        name if c.enum_def(name).is_some() => format!("PropType::Enum(\"{name}\")"),
        name if is_handle(c, name) => format!("PropType::Handle(\"{name}\")"),
        name if c.type_def(name).is_some() => format!("PropType::Record(\"{name}\")"),
        _ => "PropType::Any".to_owned(),
    }
}

/// `cacheRead` is `cache_read`; keywords become raw identifiers.
pub fn snake(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    match out.as_str() {
        "type" | "mod" | "match" | "use" | "ref" | "move" | "loop" | "box" | "fn" | "impl" | "struct" | "enum" | "self" | "crate" | "async" | "await" => format!("r#{out}"),
        _ => out,
    }
}

/// `session.panel` is `SessionPanel`, `ghost-muted` is `GhostMuted`, `GET` is `Get`.
pub fn pascal(value: &str) -> String {
    value
        .split(['.', '-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = if w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) { w.to_ascii_lowercase() } else { w.to_owned() };
            let mut chars = lower.chars();
            chars.next().map(|f| f.to_ascii_uppercase().to_string() + chars.as_str()).unwrap_or_default()
        })
        .collect()
}
