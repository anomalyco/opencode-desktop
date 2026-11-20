//! Deep accessibility snapshot builder with landmark priority budgeting, 25% reading share,
//! editable-subtree privacy boundary, sensitive-field redaction hooks, and ref preservation
//! (`packages/gui-plugins/src/browser/snapshot.ts`).

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
struct Area {
    id: i64,
    role: String,
    skipped: usize,
}

#[derive(Clone, Debug)]
struct Item {
    area_idx: usize,
    keep: bool,
    meaningful: bool,
    shown: usize,
    parent: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadingKind {
    Heading,
    Prose,
}

#[derive(Clone, Debug)]
struct Line {
    label: String,
    value: Option<(String, String)>,
    id: Option<i64>,
    interactive: bool,
    area_idx: Option<usize>,
    entry_idx: Option<usize>,
    important: bool,
    deep: bool,
    reading: Option<ReadingKind>,
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
    buckets: [Vec<Line>; 6],
    areas: Vec<Area>,
    items: Vec<Item>,
    opts: SnapshotOptions,
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

    /// Formats the final bounded snapshot (`snapshot.ts:280-440`), printing `value=<hidden>` for any
    /// field whose ID is absent or `false` in `readable`.
    pub fn finish(
        mut self,
        readable: &HashMap<i64, bool>,
        boxes: &HashMap<i64, String>,
        previous: &HashMap<i64, String>,
        picked: &HashMap<i64, String>,
        next_ref: &mut impl FnMut() -> String,
    ) -> SnapshotOutput {
        let mut refs: BTreeMap<String, i64> = BTreeMap::new();
        let mut issued: HashMap<i64, String> = HashMap::new();
        let mut resolve_ref = |id: i64| -> String {
            if let Some(p) = picked.get(&id) {
                return p.clone();
            }
            if let Some(p) = previous.get(&id) {
                return p.clone();
            }
            if let Some(p) = issued.get(&id) {
                return p.clone();
            }
            let r = next_ref();
            issued.insert(id, r.clone());
            r
        };

        let mut ordered: Vec<Line> = Vec::new();
        let mut bucket_ranges: [(usize, usize); 6] = [(0, 0); 6];
        for (bi, bucket) in self.buckets.into_iter().enumerate() {
            let start = ordered.len();
            ordered.extend(bucket);
            bucket_ranges[bi] = (start, ordered.len());
        }

        let reading_indices: Vec<usize> = if self.opts.interactive {
            Vec::new()
        } else {
            ordered
                .iter()
                .enumerate()
                .filter(|(_, l)| {
                    !l.deep
                        && (l.reading == Some(ReadingKind::Heading)
                            || (l.reading == Some(ReadingKind::Prose) && l.entry_idx.is_none()))
                })
                .map(|(idx, _)| idx)
                .collect()
        };

        let max_chars = self.opts.max_chars;
        let max_lines = self.opts.max_lines.unwrap_or(usize::MAX);
        let mut placed = vec![false; ordered.len()];
        let mut output: Vec<(usize, String)> = Vec::new();
        let mut omitted_by_area: Vec<(Option<usize>, usize)> = Vec::new();
        let mut omitted = 0usize;
        let mut omitted_interactive = 0usize;
        let mut size = 0usize;
        let mut full = false;

        let bump_omit = |line: &Line,
                         omitted: &mut usize,
                         omitted_interactive: &mut usize,
                         omitted_by_area: &mut Vec<(Option<usize>, usize)>| {
            *omitted += 1;
            if line.interactive {
                *omitted_interactive += 1;
            }
            if let Some((_, count)) = omitted_by_area.iter_mut().find(|(a, _)| *a == line.area_idx) {
                *count += 1;
            } else {
                omitted_by_area.push((line.area_idx, 1));
            }
        };

        for pass in 0..7usize {
            let chars_budget = if pass == 0 { max_chars / 4 } else { max_chars };
            let lines_budget = if pass == 0 {
                if max_lines == usize::MAX { usize::MAX } else { max_lines / 4 }
            } else {
                max_lines
            };
            let candidates: Vec<usize> = if pass == 0 {
                reading_indices.clone()
            } else {
                let (s, e) = bucket_ranges[pass - 1];
                let mut g: [Vec<usize>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
                for idx in s..e {
                    if placed[idx] {
                        continue;
                    }
                    let l = &ordered[idx];
                    let entry_keep = l.entry_idx.is_some_and(|ei| self.items[ei].keep);
                    let is_area_root = l.id.is_some()
                        && l.area_idx.is_some_and(|ai| Some(self.areas[ai].id) == l.id);
                    let rank = if l.important || entry_keep {
                        0
                    } else if l.interactive || l.reading == Some(ReadingKind::Heading) {
                        1
                    } else if is_area_root {
                        2
                    } else {
                        3
                    };
                    g[rank].push(idx);
                }
                g.into_iter().flatten().collect()
            };

            for idx in candidates {
                if pass == 0 && output.len() >= lines_budget {
                    break;
                }
                let line = &ordered[idx];
                if line.deep || full || output.len() >= max_lines {
                    bump_omit(line, &mut omitted, &mut omitted_interactive, &mut omitted_by_area);
                    continue;
                }
                let key = line.id.map(&mut resolve_ref);
                let mut text = if let Some(k) = &key {
                    let pad_len = line.label.bytes().take_while(|b| *b == b' ').count();
                    format!("{}@{k} {}", &line.label[..pad_len], &line.label[pad_len..])
                } else {
                    line.label.clone()
                };
                if let (Some(id), Some((val_json, _))) = (line.id, &line.value) {
                    if readable.get(&id).copied().unwrap_or(false) {
                        text.push_str(" value=");
                        text.push_str(val_json);
                    } else {
                        text.push_str(" value=<hidden>");
                    }
                }
                if let Some(id) = line.id
                    && let Some(bx) = boxes.get(&id)
                {
                    text.push_str(bx);
                }
                let next_len = size + usize::from(!output.is_empty()) + text.len();
                if next_len > chars_budget && pass == 0 {
                    break;
                }
                if next_len > chars_budget {
                    full = true;
                    bump_omit(line, &mut omitted, &mut omitted_interactive, &mut omitted_by_area);
                    continue;
                }
                size = next_len;
                placed[idx] = true;
                output.push((idx, text));
            }
        }

        let build_content = |rows: &mut [(usize, String)],
                             ordered: &[Line],
                             refs: &mut BTreeMap<String, i64>,
                             resolve_ref: &mut dyn FnMut(i64) -> String|
         -> String {
            rows.sort_by_key(|(idx, _)| *idx);
            let mut out = String::new();
            for (i, (idx, text)) in rows.iter().enumerate() {
                if let Some(id) = ordered[*idx].id {
                    let k = resolve_ref(id);
                    refs.insert(k, id);
                }
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(text);
            }
            out
        };

        if omitted == 0 {
            let content = build_content(&mut output, &ordered, &mut refs, &mut resolve_ref);
            return SnapshotOutput { content, truncated: false, refs };
        }

        for &(idx, _) in &output {
            let mut cur = ordered[idx].entry_idx;
            while let Some(ei) = cur {
                self.items[ei].shown += 1;
                cur = self.items[ei].parent;
            }
        }
        for item in &self.items {
            if item.meaningful && item.shown == 0 {
                self.areas[item.area_idx].skipped += 1;
            }
        }

        let make_footer = |omitted: usize,
                           omitted_interactive: usize,
                           omitted_by_area: &[(Option<usize>, usize)],
                           areas: &[Area],
                           refs: &mut BTreeMap<String, i64>,
                           resolve_ref: &mut dyn FnMut(i64) -> String|
         -> String {
            let places: Vec<String> = omitted_by_area
                .iter()
                .take(5)
                .map(|&(area_opt, count)| match area_opt {
                    None => format!("document ({count} nodes)"),
                    Some(ai) => {
                        let a = &areas[ai];
                        let k = resolve_ref(a.id);
                        refs.insert(k.clone(), a.id);
                        if a.skipped > 0 {
                            format!("… +{} more in @{k} [{}] ({count} nodes)", a.skipped, a.role)
                        } else {
                            format!("@{k} [{}] ({count} nodes)", a.role)
                        }
                    }
                })
                .collect();
            let extra = if omitted_by_area.len() > 5 {
                format!("; {} other containers", omitted_by_area.len() - 5)
            } else {
                String::new()
            };
            format!(
                "… {omitted} nodes omitted ({omitted_interactive} interactive): {}{extra}. Use find, snapshot({{tabID, ref}}), or raise maxChars/maxLines.",
                places.join(", ")
            )
        };

        let mut hint = make_footer(
            omitted,
            omitted_interactive,
            &omitted_by_area,
            &self.areas,
            &mut refs,
            &mut resolve_ref,
        );
        while !output.is_empty() && size + 1 + hint.len() > max_chars {
            let Some((idx, text)) = output.pop() else { break };
            let mut cur = ordered[idx].entry_idx;
            while let Some(ei) = cur {
                self.items[ei].shown = self.items[ei].shown.saturating_sub(1);
                if self.items[ei].shown == 0 && self.items[ei].meaningful {
                    self.areas[self.items[ei].area_idx].skipped += 1;
                }
                cur = self.items[ei].parent;
            }
            bump_omit(&ordered[idx], &mut omitted, &mut omitted_interactive, &mut omitted_by_area);
            size = size.saturating_sub(text.len() + usize::from(!output.is_empty()));
            hint = make_footer(
                omitted,
                omitted_interactive,
                &omitted_by_area,
                &self.areas,
                &mut refs,
                &mut resolve_ref,
            );
        }

        let sep = if output.is_empty() {
            ""
        } else if output.len() < max_lines {
            "\n"
        } else {
            " "
        };
        let mut content = build_content(&mut output, &ordered, &mut refs, &mut resolve_ref);
        content.push_str(sep);
        content.push_str(&hint);
        SnapshotOutput { content, truncated: true, refs }
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
            if let (Some(k), Some(v)) = (p.get("name").and_then(Value::as_str), p.get("value").and_then(|o| o.get("value"))) {
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
