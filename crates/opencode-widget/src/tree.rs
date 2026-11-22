//! A native host's tree building (`widgets.md`, Native hosts): a view as the program built it
//! (handlers as function markers, `key`s, `children` as written) made into the tree clients get,
//! exactly as the prelude's `__widget_tree` makes it, in place on the VM's value: ids from the
//! nodes' paths, handlers and keys taken off, children flattened. It also says which ids have a
//! handler and which canvas each key names.

use std::collections::HashMap;

use psychoscript::{HostString, HostValue};

/// The tree clients get from a view as built.
#[derive(Debug, Default)]
pub struct Tree {
    pub view: HostValue,
    /// The ids whose node has a handler, with its event (`press`, `change` or `pointer`).
    pub handlers: HashMap<String, &'static str>,
    /// The first canvas with each `String(key)`: its id, and whether it's drawn by its `wgsl`.
    pub canvases: HashMap<String, (String, bool)>,
}

/// The node types that take events: their event, handler prop, and whether they always get an id
/// (`always`) or get one for a `key` alone (`keyed`).
fn control(t: &str) -> Option<(&'static str, &'static str, bool, bool)> {
    Some(match t {
        "button" => ("press", "onPress", true, false),
        "card" => ("press", "onPress", false, false),
        "checkbox" | "slider" | "select" | "input" => ("change", "onChange", true, false),
        "carousel" => ("change", "onChange", false, false),
        "canvas" => ("pointer", "onPointer", false, true),
        _ => return None,
    })
}

/// Builds the tree from `root`, a view as built. Errs with a program error's message where the
/// prelude would throw (a key `encodeURIComponent` can't encode).
pub fn build(root: HostValue) -> Result<Tree, String> {
    let mut tree = Tree::default();
    let path = match &root {
        HostValue::Object(fields) => match key(fields) {
            Some(k) => segment(k)?,
            None => "0".to_owned(),
        },
        _ => "0".to_owned(),
    };
    tree.view = node(root, &path, &mut tree)?;
    Ok(tree)
}

/// A node's `key`, when it's neither `undefined` nor `null` (nor absent).
fn key(fields: &[(HostString, HostValue)]) -> Option<&HostValue> {
    prop(fields, "key").filter(|k| !matches!(k, HostValue::Undefined | HostValue::Null))
}

fn prop<'a>(fields: &'a [(HostString, HostValue)], name: &str) -> Option<&'a HostValue> {
    fields.iter().find(|(k, _)| k.as_str() == Some(name)).map(|(_, v)| v)
}

/// `":" + encodeURIComponent(String(key))`.
fn segment(key: &HostValue) -> Result<String, String> {
    let text = string(key)?;
    let mut out = String::with_capacity(text.len() + 1);
    out.push(':');
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[usize::from(b >> 4)]));
                out.push(char::from(b"0123456789ABCDEF"[usize::from(b & 15)]));
            }
        }
    }
    Ok(out)
}

/// `String(key)` for the keys a view has: a string as it is (one with a lone surrogate can't be
/// encoded, as `encodeURIComponent` throws), a number as ECMAScript writes it, `true`/`false`; an
/// array its items joined with commas; anything else as an object's `[object Object]`.
fn string(v: &HostValue) -> Result<String, String> {
    Ok(match v {
        HostValue::String(s) => match s.as_str() {
            Some(s) => s.to_owned(),
            None => return Err("encodeURIComponent received malformed URI data: String contained an illegal UTF-16 sequence.".into()),
        },
        HostValue::Number(n) => ryu_js::Buffer::new().format(*n).to_owned(),
        HostValue::Bool(b) => b.to_string(),
        HostValue::Undefined => "undefined".into(),
        HostValue::Null => "null".into(),
        HostValue::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(match item {
                    None | Some(HostValue::Undefined | HostValue::Null) => String::new(),
                    Some(v) => string(v)?,
                });
            }
            parts.join(",")
        }
        _ => "[object Object]".into(),
    })
}

/// Whether a child is skipped (keeping its index): what JavaScript counts false.
fn falsy(v: &HostValue) -> bool {
    match v {
        HostValue::Undefined | HostValue::Null | HostValue::Bool(false) => true,
        HostValue::Number(n) => *n == 0.0 || n.is_nan(),
        HostValue::String(s) => s.as_str() == Some(""),
        _ => false,
    }
}

/// A node as clients get it: its props in order without `key`, `id`, `pointer`, `onPress`,
/// `onChange`, `onPointer` and `children`, then its `id` (when it takes events), `pointer` (a
/// canvas with a handler) and `children` (flattened).
fn node(v: HostValue, path: &str, tree: &mut Tree) -> Result<HostValue, String> {
    let HostValue::Object(fields) = v else { return Ok(v) };
    let mut ty = None;
    let mut key_of = None;
    let mut children = None;
    let mut handled: [bool; 3] = [false; 3];
    let mut wgsl = false;
    let mut out = Vec::with_capacity(fields.len() + 2);
    for (k, v) in fields {
        match k.as_str() {
            Some("type") => {
                ty = match &v {
                    HostValue::String(s) => s.as_str().and_then(control),
                    _ => None,
                };
                out.push((k, v));
            }
            Some("key") => key_of = Some(v),
            Some("children") => children = Some(v),
            Some(h @ ("onPress" | "onChange" | "onPointer")) => {
                if matches!(v, HostValue::FunctionMarker) {
                    handled[match h {
                        "onPress" => 0,
                        "onChange" => 1,
                        _ => 2,
                    }] = true;
                }
            }
            Some("id" | "pointer") => {}
            Some("wgsl") => {
                wgsl = matches!(v, HostValue::String(_));
                out.push((k, v));
            }
            _ => out.push((k, v)),
        }
    }
    let key_of = key_of.filter(|k| !matches!(k, HostValue::Undefined | HostValue::Null));
    if let Some((event, handler, always, keyed)) = ty {
        let handled = handled[match handler {
            "onPress" => 0,
            "onChange" => 1,
            _ => 2,
        }];
        let keyed = keyed && key_of.is_some();
        if handled || always || keyed {
            out.push(("id".into(), HostValue::String(path.into())));
            if handled {
                tree.handlers.insert(path.to_owned(), event);
            }
            if event == "pointer" && handled {
                out.push(("pointer".into(), HostValue::Bool(true)));
            }
            if keyed && let Some(k) = &key_of {
                let name = string(k)?;
                tree.canvases.entry(name).or_insert_with(|| (path.to_owned(), wgsl));
            }
        }
    }
    match children {
        None | Some(HostValue::Undefined) => {}
        Some(list) => {
            let list = match list {
                HostValue::Array(items) => items,
                one => vec![Some(one)],
            };
            let mut into = Vec::with_capacity(list.len());
            let mut seen: HashMap<String, u32> = HashMap::new();
            flatten(list, "", path, &mut into, &mut seen, tree)?;
            out.push(("children".into(), HostValue::Array(into)));
        }
    }
    Ok(HostValue::Object(out))
}

fn flatten(list: Vec<Option<HostValue>>, prefix: &str, path: &str, into: &mut Vec<Option<HostValue>>, seen: &mut HashMap<String, u32>, tree: &mut Tree) -> Result<(), String> {
    for (i, item) in list.into_iter().enumerate() {
        let Some(item) = item.filter(|v| !falsy(v)) else { continue };
        match item {
            HostValue::Array(items) => flatten(items, &format!("{prefix}{i}."), path, into, seen, tree)?,
            HostValue::Object(ref fields) => {
                let child = match key(fields) {
                    Some(k) => {
                        let s = segment(k)?;
                        let n = seen.entry(s.clone()).or_insert(0);
                        *n += 1;
                        if *n > 1 { format!("{path}/{s}~{n}") } else { format!("{path}/{s}") }
                    }
                    None => format!("{path}/{prefix}{i}"),
                };
                into.push(Some(node(item, &child, tree)?));
            }
            other => into.push(Some(other)),
        }
    }
    Ok(())
}
