//! Deep accessibility snapshot builder with landmark priority budgeting, 25% reading share,
//! editable-subtree privacy boundary, sensitive-field redaction hooks, and ref preservation
//! (`packages/gui-plugins/src/browser/snapshot.ts`).

mod format;

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

pub const DEFAULT_MAX_CHARS: usize = 20_000;

#[derive(Clone, Debug)]
pub struct SnapshotOptions {
    pub root: Option<i64>,
    pub depth: Option<usize>,
    pub text: Option<String>,
    pub interactive: bool,
    pub boxes: bool,
    pub max_lines: Option<usize>,
    pub max_chars: usize,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            root: None,
            depth: None,
            text: None,
            interactive: false,
            boxes: false,
            max_lines: None,
            max_chars: DEFAULT_MAX_CHARS,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotOutput {
    pub content: String,
    pub truncated: bool,
    pub refs: BTreeMap<String, i64>,
}

#[derive(Clone, Debug)]
pub(super) struct Area {
    pub id: i64,
    pub role: String,
    pub skipped: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Item {
    pub area_idx: usize,
    pub keep: bool,
    pub meaningful: bool,
    pub shown: usize,
    pub parent: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadingKind {
    Heading,
    Prose,
}

#[derive(Clone, Debug)]
pub(super) struct Line {
    pub label: String,
    pub value: Option<(String, String)>,
    pub id: Option<i64>,
    pub interactive: bool,
    pub area_idx: Option<usize>,
    pub entry_idx: Option<usize>,
    pub important: bool,
    pub deep: bool,
    pub reading: Option<ReadingKind>,
}

struct Branch {
    node_idx: usize,
    level: usize,
    parent_id: String,
    parent_name: String,
    scope: String,
    item: String,
    ordinal: usize,
    emphasis: bool,
    priority: usize,
    area_idx: Option<usize>,
    collection_idx: Option<usize>,
    entry_idx: Option<usize>,
}

/// First phase of snapshot generation: walked accessibility tree lines ready for async
/// `readableValue` (sensitive-field redaction) and optional `box` CDP lookups.
pub struct PreparedSnapshot {
    pub(super) buckets: [Vec<Line>; 6],
    pub(super) areas: Vec<Area>,
    pub(super) items: Vec<Item>,
    pub(super) opts: SnapshotOptions,
}

impl PreparedSnapshot {
    /// Backend DOM node IDs with non-empty field values that must be checked via `DESCRIBE_FIELD_JS`
    /// before printing (`value=<hidden>` unless explicitly confirmed readable).
    pub fn value_targets(&self) -> Vec<(i64, String)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for bucket in &self.buckets {
            for line in bucket {
                if !line.deep
                    && let (Some(id), Some((_, name))) = (line.id, &line.value)
                    && seen.insert(id)
                {
                    out.push((id, name.clone()));
                }
            }
        }
        out
    }

    /// Backend DOM node IDs needing bounding boxes when `opts.boxes == true`.
    pub fn box_targets(&self) -> Vec<i64> {
        if !self.opts.boxes {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for bucket in &self.buckets {
            for line in bucket {
                if !line.deep
                    && let Some(id) = line.id
                    && seen.insert(id)
                {
                    out.push(id);
                }
            }
        }
        out
    }
}

/// Walks a CDP `Accessibility.getFullAXTree` `nodes` array into a [`PreparedSnapshot`].
/// Never descends into editable subtrees (`snapshot.ts:101, 254`).
pub fn prepare_snapshot(
    nodes: &[Value],
    opts: SnapshotOptions,
    previous: &HashMap<i64, String>,
    picked: &HashMap<i64, String>,
) -> Result<PreparedSnapshot, String> {
    let mut index: HashMap<&str, usize> = HashMap::with_capacity(nodes.len());
    let mut root_idx: Option<usize> = if opts.root.is_none() && !nodes.is_empty() { Some(0) } else { None };
    for (i, node) in nodes.iter().enumerate() {
        if let Some(nid) = node.get("nodeId").and_then(Value::as_str) {
            index.insert(nid, i);
        }
        if let Some(want) = opts.root
            && node.get("backendDOMNodeId").and_then(Value::as_i64) == Some(want)
            && (root_idx.is_none()
                || root_idx.is_some_and(|r| nodes[r].get("ignored").and_then(Value::as_bool).unwrap_or(false)))
        {
            root_idx = Some(i);
        }
    }
    let Some(root_idx) = root_idx else {
        return Err(
            "Element is absent from this frame's accessibility snapshot. Refresh the snapshot without ref."
                .into(),
        );
    };

    let mut pending = vec![Branch {
        node_idx: root_idx,
        level: 0,
        parent_id: String::new(),
        parent_name: String::new(),
        scope: String::new(),
        item: String::new(),
        ordinal: 1,
        emphasis: false,
        priority: 5,
        area_idx: None,
        collection_idx: None,
        entry_idx: None,
    }];
    let mut buckets: [Vec<Line>; 6] = Default::default();
    let mut areas: Vec<Area> = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut headings: HashMap<String, String> = HashMap::new();
    let query = opts.text.as_deref().map(str::to_lowercase);

    while let Some(branch) = pending.pop() {
        let node = &nodes[branch.node_idx];
        let node_id = node.get("nodeId").and_then(Value::as_str).unwrap_or("");
        if !seen.insert(node_id) {
            continue;
        }
        let role = sanitize_role(ax_str(node.get("role")).as_deref().unwrap_or("node"));
        let name = collapse_ws(ax_str(node.get("name")).as_deref().unwrap_or(""));
        let id = node.get("backendDOMNodeId").and_then(Value::as_i64);
        let known = id
            .and_then(|bid| picked.get(&bid).or_else(|| previous.get(&bid)))
            .cloned();
        let props = extract_props(node);
        let editable = role == "textbox"
            || role == "searchbox"
            || props.get("editable").is_some_and(is_truthy_prop);
        let interactive = id.is_some()
            && role != "RootWebArea"
            && (editable || props.get("focusable").is_some_and(is_truthy_prop) || is_control(&role));

        let ignored = node.get("ignored").and_then(Value::as_bool).unwrap_or(false);
        let wrapper = matches!(role.as_str(), "generic" | "none" | "presentation")
            && name.is_empty()
            && !interactive
            && known.is_none()
            && node.get("value").is_none()
            && node.get("description").is_none()
            && props.is_empty();
        let visible = !ignored
            && !wrapper
            && role != "InlineTextBox"
            && !(role == "StaticText" && name == branch.parent_name && known.is_none());

        let area_idx = if visible
            && let Some(bid) = id
            && (is_landmark(&role) || is_collection(&role))
        {
            areas.push(Area { id: bid, role: role.clone(), skipped: 0 });
            Some(areas.len() - 1)
        } else {
            branch.area_idx
        };
        let collection_idx = if area_idx.is_some_and(|ai| is_collection(&areas[ai].role)) {
            area_idx
        } else {
            branch.collection_idx
        };
        let priority = branch.priority.min(if visible { landmark_rank(&role) } else { 5 });
        let emphasis = branch.emphasis
            || matches!(
                role.as_str(),
                "heading" | "columnheader" | "rowheader" | "status" | "alert" | "alertdialog"
            );
        let inherited = headings
            .get(&branch.parent_id)
            .cloned()
            .unwrap_or_else(|| branch.scope.clone());
        let scope = if opts.interactive
            && visible
            && !name.is_empty()
            && (is_landmark(&role) || is_collection(&role) || role == "heading")
        {
            format!("{role} {}", json_quote(&excerpt(&name, 60)))
        } else {
            inherited.clone()
        };
        if opts.interactive && visible && role == "heading" && !name.is_empty() {
            headings.insert(branch.parent_id.clone(), scope.clone());
        }

        let child_ids: Vec<&str> = node
            .get("childIds")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let is_item = visible && matches!(role.as_str(), "row" | "listitem" | "option");
        let item_name = if opts.interactive && matches!(role.as_str(), "row" | "listitem") && name.is_empty() {
            find_item_label(&child_ids, nodes, &index)
        } else {
            name.clone()
        };
        let item_str = if opts.interactive && is_item {
            if item_name.is_empty() {
                format!("{role} {}", branch.ordinal)
            } else {
                format!("{role} {}", json_quote(&excerpt(&item_name, 60)))
            }
        } else {
            branch.item.clone()
        };

        let entry_idx = if is_item && let Some(col_ai) = collection_idx {
            items.push(Item {
                area_idx: col_ai,
                keep: false,
                meaningful: false,
                shown: 0,
                parent: branch.entry_idx,
            });
            Some(items.len() - 1)
        } else {
            branch.entry_idx
        };

        let important = ["focused", "selected", "checked", "invalid"]
            .iter()
            .any(|&k| props.get(k).is_some_and(is_truthy_prop));

        if visible && let Some(ei) = entry_idx && (interactive || !name.trim().is_empty() || important) && important {
            let mut cur = Some(ei);
            while let Some(ci) = cur {
                items[ci].keep = true;
                cur = items[ci].parent;
            }
        }

        if visible && (!opts.interactive || interactive) {
            let mut flags = Vec::new();
            for key in [
                "checked",
                "disabled",
                "expanded",
                "selected",
                "pressed",
                "focused",
                "invalid",
                "required",
                "readonly",
            ] {
                if let Some(pv) = props.get(key) {
                    let is_false = matches!(pv, Value::Bool(false))
                        || matches!(pv, Value::String(s) if s == "false");
                    let omit_false = matches!(key, "invalid" | "required" | "readonly") && is_false;
                    if !omit_false {
                        flags.push(format!("{key}={}", prop_display(pv)));
                    }
                }
            }
            let context = if opts.interactive {
                [inherited.as_str(), branch.item.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" / ")
            } else {
                String::new()
            };
            let mut label = format!(
                "[{role}] {}",
                json_quote(&excerpt(&name, if interactive { 300 } else { 160 }))
            );
            if !flags.is_empty() {
                label.push(' ');
                label.push_str(&flags.join(" "));
            }
            if !context.is_empty() {
                label.push_str(" in ");
                label.push_str(&context);
            }

            let matches_query = match &query {
                None => true,
                Some(q) => {
                    label.to_lowercase().contains(q)
                        || name.to_lowercase().contains(q)
                        || known.as_ref().is_some_and(|k| format!("@{k}").contains(q))
                }
            };
            if matches_query {
                let pad = if opts.interactive {
                    String::new()
                } else {
                    "  ".repeat(branch.level.min(12))
                };
                if let Some(ei) = entry_idx
                    && (interactive || !name.trim().is_empty() || important)
                {
                    items[ei].meaningful = true;
                }
                let raw_val = ax_str(node.get("value"));
                let value = if interactive
                    && let Some(vs) = raw_val
                    && !vs.is_empty()
                {
                    Some((json_quote(&excerpt(&vs, 160)), name.clone()))
                } else {
                    None
                };
                let keep_id = if interactive
                    || known.is_some()
                    || area_idx.is_some_and(|ai| Some(areas[ai].id) == id)
                {
                    id
                } else {
                    None
                };
                let reading = if interactive || role == "RootWebArea" || name.trim().is_empty() {
                    None
                } else if emphasis {
                    Some(ReadingKind::Heading)
                } else {
                    Some(ReadingKind::Prose)
                };
                let bucket_i = if role == "RootWebArea" { 0 } else { priority.min(5) };
                buckets[bucket_i].push(Line {
                    label: format!("{pad}{label}"),
                    value,
                    id: keep_id,
                    interactive,
                    area_idx,
                    entry_idx,
                    important,
                    deep: opts.depth.is_some_and(|d| branch.level > d),
                    reading,
                });
            }
        }

        // Privacy boundary (`snapshot.ts:254`): never descend into editable subtrees.
        if editable {
            continue;
        }

        for (i, cid) in child_ids.iter().enumerate().rev() {
            if let Some(&cidx) = index.get(cid) {
                pending.push(Branch {
                    node_idx: cidx,
                    level: branch.level + usize::from(visible),
                    parent_id: if visible { node_id.to_owned() } else { branch.parent_id.clone() },
                    parent_name: if visible { name.clone() } else { branch.parent_name.clone() },
                    scope: scope.clone(),
                    item: item_str.clone(),
                    emphasis,
                    priority,
                    area_idx,
                    collection_idx,
                    entry_idx,
                    ordinal: i + 1,
                });
            }
        }
    }

    Ok(PreparedSnapshot { buckets, areas, items, opts })
}

fn find_item_label(children: &[&str], nodes: &[Value], index: &HashMap<&str, usize>) -> String {
    let mut stack: Vec<&str> = children.iter().rev().copied().collect();
    let mut seen: HashSet<&str> = HashSet::new();
    while let Some(cid) = stack.pop() {
        if !seen.insert(cid) {
            continue;
        }
        let Some(&idx) = index.get(cid) else { continue };
        let node = &nodes[idx];
        let role = ax_str(node.get("role")).unwrap_or_default();
        let editable = node
            .get("properties")
            .and_then(Value::as_array)
            .is_some_and(|ps| ps.iter().any(|p| p.get("name").and_then(Value::as_str) == Some("editable")));
        if is_control(&role) || is_collection(&role) || role == "row" || role == "listitem" || editable {
            continue;
        }
        let ignored = node.get("ignored").and_then(Value::as_bool).unwrap_or(false);
        let name = collapse_ws(ax_str(node.get("name")).as_deref().unwrap_or("")).trim().to_owned();
        if !ignored && !name.is_empty() {
            return name;
        }
        if let Some(arr) = node.get("childIds").and_then(Value::as_array) {
            for c in arr.iter().rev().filter_map(Value::as_str) {
                stack.push(c);
            }
        }
    }
    String::new()
}

fn ax_str(field: Option<&Value>) -> Option<String> {
    let v = field?.get("value")?;
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn extract_props(node: &Value) -> HashMap<&str, &Value> {
    let mut map = HashMap::new();
    if let Some(arr) = node.get("properties").and_then(Value::as_array) {
        for p in arr {
            if let (Some(k), Some(v)) = (
                p.get("name").and_then(Value::as_str),
                p.get("value").and_then(|o| o.get("value")),
            ) {
                map.insert(k, v);
            }
        }
    }
    map
}

fn is_truthy_prop(v: &&Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.is_empty() && s != "false",
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        _ => true,
    }
}

fn prop_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn sanitize_role(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(40)
        .collect()
}

fn collapse_ws(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_ws = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

fn excerpt(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_owned();
    }
    let prefix: String = s.chars().take(max_chars).collect();
    format!("{prefix}… [+{} chars]", count - max_chars)
}

fn json_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn is_control(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "link"
            | "textbox"
            | "searchbox"
            | "combobox"
            | "listbox"
            | "checkbox"
            | "radio"
            | "option"
            | "switch"
            | "tab"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "slider"
            | "spinbutton"
            | "treeitem"
            | "scrollbar"
    )
}

fn is_collection(role: &str) -> bool {
    matches!(role, "table" | "grid" | "treegrid" | "list" | "listbox" | "combobox" | "menu" | "tree")
}

fn is_landmark(role: &str) -> bool {
    matches!(
        role,
        "alert"
            | "dialog"
            | "alertdialog"
            | "main"
            | "form"
            | "search"
            | "region"
            | "navigation"
            | "contentinfo"
            | "banner"
            | "complementary"
    )
}

fn landmark_rank(role: &str) -> usize {
    match role {
        "alert" | "alertdialog" | "dialog" => 1,
        "main" => 2,
        "form" | "search" => 3,
        r if is_landmark(r) => 4,
        _ => 5,
    }
}
