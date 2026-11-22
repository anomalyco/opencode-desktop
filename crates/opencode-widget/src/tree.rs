//! A native host's tree building (`widgets.md`, Native hosts): a view as the program built it
//! (handlers as function markers, `key`s, `children` as written) made into the tree clients get,
//! exactly as the prelude's `__widget_tree` makes it, in place on the VM's value: ids from the
//! nodes' paths, handlers and keys taken off, children flattened. It also says which ids have a
//! handler and which canvas each key names, and fails a view that breaks Keys and depth (a key of
//! another type, more than 100 levels) or carries a function anywhere but a handler prop.

use std::collections::HashMap;

use psychoscript::{HostError, HostString, HostValue};

/// The most levels a view nests, and the failures of a view that breaks the rules of views.
pub const LEVELS: usize = 100;
pub const DEEP: &str = "The view nests more than 100 levels";
pub const KEY: &str = "A node's key must be a string, a number or a boolean";

/// The tree clients get from a view as built.
#[derive(Debug, Default)]
pub struct Tree {
    pub view: HostValue,
    /// The ids whose node has a handler, with its event (`press`, `change` or `pointer`).
    pub handlers: HashMap<String, &'static str>,
    /// The first canvas with each `String(key)`: its id, and whether it's drawn by its `wgsl`.
    pub canvases: HashMap<String, (String, bool)>,
}

/// Why a view didn't build.
#[derive(Debug)]
pub enum Fail {
    /// It breaks Keys and depth: a program error in phase `view`, with this message.
    View(String),
    /// What the host call throws to the program (a function where none may go, a key
    /// `encodeURIComponent` can't encode).
    Thrown(HostValue),
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

/// The runtime's error for a function in a host call's argument named `label`
/// (`Argument 1 to __host.post`), as it throws one without `functions: "mark"`: a `TypeError`
/// located at the call.
pub fn carried(label: &str) -> HostValue {
    HostError::located("TypeError", format!("{label} contains a function, which cannot be passed to the host."))
}

/// Fails with [`carried`] when `v` holds a function marker anywhere.
pub fn strict(v: &HostValue, label: &str) -> Result<(), HostValue> {
    if marked(v) { Err(carried(label)) } else { Ok(()) }
}

fn marked(v: &HostValue) -> bool {
    match v {
        HostValue::FunctionMarker => true,
        HostValue::Array(items) => items.iter().flatten().any(marked),
        HostValue::Object(fields) => fields.iter().any(|(_, v)| marked(v)),
        HostValue::Map(entries) => entries.iter().any(|(k, v)| marked(k) || marked(v)),
        HostValue::Set(items) => items.iter().any(marked),
        _ => false,
    }
}

/// Builds the tree from `root`, a view as built in a host call's argument named `label`.
pub fn build(root: HostValue, label: &str) -> Result<Tree, Fail> {
    let mut tree = Tree::default();
    if !matches!(root, HostValue::Object(_)) {
        strict(&root, label).map_err(Fail::Thrown)?;
        tree.view = root;
        return Ok(tree);
    }
    let path = match &root {
        HostValue::Object(fields) => match key(fields)? {
            Some(k) => segment(k)?,
            None => "0".to_owned(),
        },
        _ => "0".to_owned(),
    };
    tree.view = node(root, &path, 1, label, &mut tree)?;
    Ok(tree)
}

/// A node's `key`: none when `undefined` or `null` (or absent); a string, a number or a boolean;
/// anything else fails the view.
fn key(fields: &[(HostString, HostValue)]) -> Result<Option<&HostValue>, Fail> {
    match fields.iter().find(|(k, _)| k.as_str() == Some("key")).map(|(_, v)| v) {
        None | Some(HostValue::Undefined | HostValue::Null) => Ok(None),
        Some(k @ (HostValue::String(_) | HostValue::Number(_) | HostValue::Bool(_))) => Ok(Some(k)),
        Some(_) => Err(Fail::View(KEY.into())),
    }
}

/// `":" + encodeURIComponent(String(key))` for a key that is one (see [`key`]).
fn segment(key: &HostValue) -> Result<String, Fail> {
    let text = match key {
        HostValue::String(s) => match s.as_str() {
            Some(s) => s.to_owned(),
            None => {
                let e = HostError::located("URIError", "encodeURIComponent received malformed URI data: String contained an illegal UTF-16 sequence.");
                return Err(Fail::Thrown(e));
            }
        },
        HostValue::Number(n) => ryu_js::Buffer::new().format(*n).to_owned(),
        HostValue::Bool(b) => b.to_string(),
        _ => return Err(Fail::View(KEY.into())),
    };
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

/// Whether a child is skipped (keeping its index): what JavaScript counts false.
fn falsy(v: &HostValue) -> bool {
    match v {
        HostValue::Undefined | HostValue::Null | HostValue::Bool(false) => true,
        HostValue::Number(n) => *n == 0.0 || n.is_nan(),
        HostValue::String(s) => s.as_str() == Some(""),
        _ => false,
    }
}

/// What JavaScript's `typeof` calls an object (a node, an array, a Map…): what counts a level.
fn object(v: &HostValue) -> bool {
    !matches!(v, HostValue::Undefined | HostValue::Null | HostValue::Bool(_) | HostValue::Number(_) | HostValue::String(_) | HostValue::FunctionMarker)
}

/// A node at `level` as clients get it: its props in order without `key`, `id`, `pointer`,
/// `onPress`, `onChange`, `onPointer` and `children`, then its `id` (when it takes events),
/// `pointer` (a canvas with a handler) and `children` (flattened). Its key is checked first, then
/// its props, then its children.
fn node(v: HostValue, path: &str, level: usize, label: &str, tree: &mut Tree) -> Result<HostValue, Fail> {
    let HostValue::Object(fields) = v else { return Ok(v) };
    let key_of = key(&fields)?.cloned();
    let mut ty = None;
    let mut children = None;
    let mut handled: [bool; 3] = [false; 3];
    let mut wgsl = false;
    let mut out = Vec::with_capacity(fields.len() + 2);
    for (k, v) in fields {
        match k.as_str() {
            Some("children") => {
                children = Some(v);
                continue;
            }
            Some(h @ ("onPress" | "onChange" | "onPointer")) => {
                if matches!(v, HostValue::FunctionMarker) {
                    handled[handler(h)] = true;
                }
                continue;
            }
            _ => {}
        }
        strict(&v, label).map_err(Fail::Thrown)?;
        match k.as_str() {
            Some("type") => {
                ty = match &v {
                    HostValue::String(s) => s.as_str().and_then(control),
                    _ => None,
                };
                out.push((k, v));
            }
            Some("key" | "id" | "pointer") => {}
            Some("wgsl") => {
                wgsl = matches!(v, HostValue::String(_));
                out.push((k, v));
            }
            _ => out.push((k, v)),
        }
    }
    if let Some((event, name, always, keyed)) = ty {
        let handled = handled[handler(name)];
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
                let name = match k {
                    HostValue::String(s) => s.to_string_lossy().into_owned(),
                    HostValue::Number(n) => ryu_js::Buffer::new().format(*n).to_owned(),
                    HostValue::Bool(b) => b.to_string(),
                    _ => String::new(),
                };
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
            flatten(list, "", path, level + 1, label, &mut into, &mut seen, tree)?;
            out.push(("children".into(), HostValue::Array(into)));
        }
    }
    Ok(HostValue::Object(out))
}

fn handler(name: &str) -> usize {
    match name {
        "onPress" => 0,
        "onChange" => 1,
        _ => 2,
    }
}

/// Appends `list`'s items, at `level`, to `into`: holes skipped (keeping their index), arrays
/// flattened one level down, nodes built.
#[allow(clippy::too_many_arguments)]
fn flatten(list: Vec<Option<HostValue>>, prefix: &str, path: &str, level: usize, label: &str, into: &mut Vec<Option<HostValue>>, seen: &mut HashMap<String, u32>, tree: &mut Tree) -> Result<(), Fail> {
    for (i, item) in list.into_iter().enumerate() {
        let Some(item) = item.filter(|v| !falsy(v)) else { continue };
        if level > LEVELS && object(&item) {
            return Err(Fail::View(DEEP.into()));
        }
        match item {
            HostValue::Array(items) => flatten(items, &format!("{prefix}{i}."), path, level + 1, label, into, seen, tree)?,
            HostValue::Object(ref fields) => {
                let child = match key(fields)? {
                    Some(k) => {
                        let s = segment(k)?;
                        let n = seen.entry(s.clone()).or_insert(0);
                        *n += 1;
                        if *n > 1 { format!("{path}/{s}~{n}") } else { format!("{path}/{s}") }
                    }
                    None => format!("{path}/{prefix}{i}"),
                };
                into.push(Some(node(item, &child, level, label, tree)?));
            }
            other => {
                strict(&other, label).map_err(Fail::Thrown)?;
                into.push(Some(other));
            }
        }
    }
    Ok(())
}
