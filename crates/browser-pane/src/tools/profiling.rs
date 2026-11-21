//! Pure-Rust analyzers for Chromium performance traces, V8 CPU profiles, and V8 heap snapshots
//! (`packages/gui-plugins/src/browser/analysis.ts`).

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

const MAX_NAME: usize = 2_048;
const MAX_TEXT: usize = 100_000;

/// Analyzes a Chromium performance trace JSON object (`analysis.ts:45-85`).
pub fn analyze_trace(value: &Value, limit: usize) -> Result<Value, String> {
    let events_arr = value
        .get("traceEvents")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "Selected file is not a Chromium performance trace. Use a fileID returned by browser.trace.stop for this tab."
                .to_owned()
        })?;
    let mut agg: HashMap<String, (u64, f64, f64)> = HashMap::new();
    let mut long_tasks: Vec<f64> = Vec::new();
    for ev in events_arr {
        if ev.get("ph").and_then(Value::as_str) != Some("X") {
            continue;
        }
        let Some(name) = ev.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(dur_us) = ev.get("dur").and_then(Value::as_f64).filter(|d| d.is_finite()) else {
            continue;
        };
        let dur_ms = dur_us / 1000.0;
        let key = truncate_chars(name, MAX_NAME);
        let entry = agg.entry(key).or_insert((0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += dur_ms;
        if dur_ms > entry.2 {
            entry.2 = dur_ms;
        }
        if matches!(name, "RunTask" | "ThreadControllerImpl::RunTask") && dur_ms > 50.0 {
            long_tasks.push(dur_ms);
        }
    }
    let mut rows: Vec<(String, u64, f64, f64)> = agg
        .into_iter()
        .map(|(name, (count, total, max))| (name, count, total, max))
        .collect();
    rows.sort_by(|a, b| b.2.total_cmp(&a.2));
    rows.truncate(limit.clamp(1, 500));
    let blocking: f64 = long_tasks.iter().map(|d| d - 50.0).sum();
    let long_insight = if !long_tasks.is_empty() {
        format!(
            "{} recorded tasks exceeded 50 ms; inspect their stacks in the exported trace.",
            long_tasks.len()
        )
    } else {
        "No tasks over 50 ms were observed in the retained trace. This is not proof that the page has no long tasks."
            .to_owned()
    };
    Ok(json!({
        "metrics": [
            { "name": "recordedEvents", "value": events_arr.len(), "unit": "count" },
            { "name": "longTasks", "value": long_tasks.len(), "unit": "count" },
            { "name": "longTaskBlocking", "value": blocking, "unit": "ms" },
        ],
        "events": rows.into_iter().map(|(name, count, total_ms, max_ms)| {
            json!({ "name": name, "count": count, "totalMs": total_ms, "maxMs": max_ms })
        }).collect::<Vec<_>>(),
        "insights": [
            "Event durations can overlap and must not be summed into total page time.",
            long_insight,
            "This renderer-process trace does not calculate LCP, INP, CLS, or Lighthouse performance scores.",
        ],
    }))
}

/// Analyzes a V8 `.cpuprofile` JSON object (`analysis.ts:87-113`).
pub fn analyze_cpu(value: &Value, limit: usize) -> Result<Value, String> {
    let start = value.get("startTime").and_then(Value::as_f64).filter(|v| v.is_finite());
    let end = value.get("endTime").and_then(Value::as_f64).filter(|v| v.is_finite());
    let nodes = value.get("nodes").and_then(Value::as_array);
    let (Some(start), Some(end), Some(nodes)) = (start, end, nodes) else {
        return Err(
            "Selected file is not a CPU profile. Use a fileID returned by browser.cpu.stop for this tab."
                .into(),
        );
    };
    let samples = value.get("samples").and_then(Value::as_array);
    let deltas = value.get("timeDeltas").and_then(Value::as_array);
    let mut times: HashMap<i64, f64> = HashMap::new();
    if let Some(samples) = samples {
        for (idx, sid) in samples.iter().enumerate() {
            let Some(id) = sid.as_i64() else { continue };
            let dt = deltas
                .and_then(|d| d.get(idx))
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .unwrap_or(0.0);
            *times.entry(id).or_insert(0.0) += dt / 1000.0;
        }
    }
    let mut funcs: Vec<(String, String, u64, f64)> = Vec::new();
    for node in nodes {
        let Some(id) = node.get("id").and_then(Value::as_i64) else { continue };
        let self_ms = times.get(&id).copied().unwrap_or(0.0);
        if self_ms <= 0.0 {
            continue;
        }
        let Some(cf) = node.get("callFrame") else { continue };
        let name = truncate_chars(cf.get("functionName").and_then(Value::as_str).unwrap_or(""), MAX_NAME);
        let url = truncate_chars(cf.get("url").and_then(Value::as_str).unwrap_or(""), MAX_TEXT);
        let line = cf.get("lineNumber").and_then(Value::as_i64).unwrap_or(0).saturating_add(1).max(0) as u64;
        funcs.push((name, url, line, self_ms));
    }
    funcs.sort_by(|a, b| b.3.total_cmp(&a.3));
    funcs.truncate(limit.clamp(1, 500));
    Ok(json!({
        "durationMs": ((end - start) / 1000.0).max(0.0),
        "functions": funcs.into_iter().map(|(name, url, line, self_ms)| {
            json!({ "name": name, "url": url, "line": line, "selfMs": self_ms })
        }).collect::<Vec<_>>(),
    }))
}

/// Validated V8 heap snapshot ready for bounded `summary`, `query`, `object`, and `compare`
/// queries (`analysis.ts:120-260`).
pub struct HeapSnapshot {
    nodes: Vec<i64>,
    edges: Vec<i64>,
    strings: Vec<String>,
    node_types: Vec<String>,
    edge_types: Vec<String>,
    width: usize,
    edge_width: usize,
    idx_type: usize,
    idx_name: usize,
    idx_id: usize,
    idx_size: usize,
    idx_count: usize,
    idx_edge_type: usize,
    idx_edge_name: usize,
    idx_edge_to: usize,
    self_bytes: u64,
    classes: BTreeMap<String, (u64, u64)>,
}

impl HeapSnapshot {
    pub fn parse(value: &Value) -> Result<Self, String> {
        let malformed = || {
            "Heap snapshot layout is unsupported or incomplete. Use a complete capture from browser.heap.snapshot."
                .to_owned()
        };
        let meta = value
            .get("snapshot")
            .and_then(|s| s.get("meta"))
            .ok_or_else(|| {
                "Selected file is not a V8 heap snapshot. Use a fileID returned by browser.heap.snapshot for this tab."
                    .to_owned()
            })?;
        let node_fields: Vec<&str> = meta
            .get("node_fields")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let edge_fields: Vec<&str> = meta
            .get("edge_fields")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let width = node_fields.len();
        let edge_width = edge_fields.len();
        let find = |arr: &[&str], name: &str| arr.iter().position(|&f| f == name);
        let (Some(idx_type), Some(idx_name), Some(idx_id), Some(idx_size), Some(idx_count)) = (
            find(&node_fields, "type"),
            find(&node_fields, "name"),
            find(&node_fields, "id"),
            find(&node_fields, "self_size"),
            find(&node_fields, "edge_count"),
        ) else {
            return Err(malformed());
        };
        let (Some(idx_edge_type), Some(idx_edge_name), Some(idx_edge_to)) = (
            find(&edge_fields, "type"),
            find(&edge_fields, "name_or_index"),
            find(&edge_fields, "to_node"),
        ) else {
            return Err(malformed());
        };
        let node_types: Vec<String> = meta
            .get("node_types")
            .and_then(Value::as_array)
            .and_then(|a| a.get(idx_type))
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|v| v.as_str().unwrap_or("unknown").to_owned())
            .collect();
        let edge_types: Vec<String> = meta
            .get("edge_types")
            .and_then(Value::as_array)
            .and_then(|a| a.get(idx_edge_type))
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|v| v.as_str().unwrap_or("unknown").to_owned())
            .collect();
        let nodes: Vec<i64> = value
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|v| v.as_i64().ok_or_else(malformed))
            .collect::<Result<_, _>>()?;
        let edges: Vec<i64> = value
            .get("edges")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|v| v.as_i64().ok_or_else(malformed))
            .collect::<Result<_, _>>()?;
        let strings: Vec<String> = value
            .get("strings")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_owned())
            .collect();

        if width == 0 || edge_width == 0 || !nodes.len().is_multiple_of(width) || !edges.len().is_multiple_of(edge_width) {
            return Err(malformed());
        }

        let mut classes: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let mut self_bytes = 0u64;
        let mut edge_total = 0u64;
        for offset in (0..nodes.len()).step_by(width) {
            let ec = nodes[offset + idx_count];
            if ec < 0 {
                return Err(malformed());
            }
            edge_total = edge_total.checked_add(ec as u64).ok_or_else(malformed)?;
            let sz = nodes[offset + idx_size].max(0) as u64;
            let name_i = nodes[offset + idx_name].max(0) as usize;
            let name = truncate_chars(strings.get(name_i).map(String::as_str).unwrap_or(""), MAX_NAME);
            let entry = classes.entry(name).or_insert((0, 0));
            entry.0 += 1;
            entry.1 = entry.1.saturating_add(sz);
            self_bytes = self_bytes.saturating_add(sz);
        }
        if edge_total.checked_mul(edge_width as u64) != Some(edges.len() as u64) {
            return Err(malformed());
        }
        for offset in (idx_edge_to..edges.len()).step_by(edge_width) {
            let to = edges[offset];
            if to < 0 || (to as usize) >= nodes.len() || !(to as usize).is_multiple_of(width) {
                return Err(malformed());
            }
        }
        Ok(Self {
            nodes,
            edges,
            strings,
            node_types,
            edge_types,
            width,
            edge_width,
            idx_type,
            idx_name,
            idx_id,
            idx_size,
            idx_count,
            idx_edge_type,
            idx_edge_name,
            idx_edge_to,
            self_bytes,
            classes,
        })
    }

    fn node_json(&self, offset: usize) -> Value {
        let name_i = self.nodes[offset + self.idx_name].max(0) as usize;
        let type_i = self.nodes[offset + self.idx_type].max(0) as usize;
        json!({
            "id": self.nodes[offset + self.idx_id],
            "name": truncate_chars(self.strings.get(name_i).map(String::as_str).unwrap_or(""), MAX_TEXT),
            "type": self.node_types.get(type_i).map(String::as_str).unwrap_or("unknown"),
            "selfBytes": self.nodes[offset + self.idx_size].max(0),
            "edgeCount": self.nodes[offset + self.idx_count].max(0),
        })
    }

    pub fn summary(&self, limit: usize) -> Value {
        let mut list: Vec<(&str, u64, u64)> = self
            .classes
            .iter()
            .map(|(k, &(c, b))| (k.as_str(), c, b))
            .collect();
        list.sort_by(|a, b| b.2.cmp(&a.2));
        list.truncate(limit.clamp(1, 500));
        json!({
            "nodes": self.nodes.len() / self.width,
            "edges": self.edges.len() / self.edge_width,
            "selfBytes": self.self_bytes,
            "classes": list.into_iter().map(|(name, count, bytes)| {
                json!({ "name": name, "count": count, "bytes": bytes })
            }).collect::<Vec<_>>(),
        })
    }

    pub fn query(&self, filter: &str, limit: usize) -> Value {
        let needle = filter.to_lowercase();
        let mut found: Vec<(i64, usize)> = Vec::new();
        for offset in (0..self.nodes.len()).step_by(self.width) {
            let name_i = self.nodes[offset + self.idx_name].max(0) as usize;
            let name = self.strings.get(name_i).map(String::as_str).unwrap_or("");
            if needle.is_empty() || name.to_lowercase().contains(&needle) {
                found.push((self.nodes[offset + self.idx_size], offset));
            }
        }
        let limit = limit.clamp(1, 500);
        let truncated = found.len() > limit;
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.truncate(limit);
        json!({
            "nodes": found.into_iter().map(|(_, off)| self.node_json(off)).collect::<Vec<_>>(),
            "truncated": truncated,
        })
    }

    pub fn object(&self, id: i64, limit: usize) -> Result<Value, String> {
        let target = (0..self.nodes.len())
            .step_by(self.width)
            .find(|&off| self.nodes[off + self.idx_id] == id)
            .ok_or_else(|| {
                "Object ID was not found in this heap snapshot. Call browser.heap.query with the same tabID and fileID."
                    .to_owned()
            })?;
        let limit = limit.clamp(1, 500);
        let mut references = Vec::new();
        let mut retainers = Vec::new();
        let mut truncated = false;
        let mut edge_off = 0usize;
        for offset in (0..self.nodes.len()).step_by(self.width) {
            let count = self.nodes[offset + self.idx_count].max(0) as usize;
            for _ in 0..count {
                let to = self.edges[edge_off + self.idx_edge_to] as usize;
                if offset == target || to == target {
                    let et_i = self.edges[edge_off + self.idx_edge_type].max(0) as usize;
                    let et = self.edge_types.get(et_i).map(String::as_str).unwrap_or("");
                    let raw = self.edges[edge_off + self.idx_edge_name];
                    let name = if matches!(et, "element" | "hidden") {
                        raw.to_string()
                    } else {
                        truncate_chars(self.strings.get(raw.max(0) as usize).map(String::as_str).unwrap_or(""), MAX_TEXT)
                    };
                    if offset == target {
                        if references.len() < limit {
                            references.push(json!({ "name": name, "node": self.node_json(to) }));
                        } else {
                            truncated = true;
                        }
                    }
                    if to == target && et != "weak" {
                        if retainers.len() < limit {
                            retainers.push(json!({ "name": name, "node": self.node_json(offset) }));
                        } else {
                            truncated = true;
                        }
                    }
                }
                edge_off += self.edge_width;
            }
        }
        Ok(json!({
            "node": self.node_json(target),
            "references": references,
            "retainers": retainers,
            "truncated": truncated,
        }))
    }

    pub fn compare(before: &Self, after: &Self, limit: usize) -> Value {
        let mut names: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
        for (name, &(c, b)) in &after.classes {
            names.insert(name.as_str(), (c as i64, b as i64));
        }
        for (name, &(c, b)) in &before.classes {
            let e = names.entry(name.as_str()).or_insert((0, 0));
            e.0 -= c as i64;
            e.1 -= b as i64;
        }
        let mut deltas: Vec<(&str, i64, i64)> = names
            .into_iter()
            .filter(|(_, (cd, bd))| *cd != 0 || *bd != 0)
            .map(|(n, (cd, bd))| (n, cd, bd))
            .collect();
        deltas.sort_by(|a, b| b.2.cmp(&a.2));
        deltas.truncate(limit.clamp(1, 500));
        json!({
            "classes": deltas.into_iter().map(|(name, count_delta, bytes_delta)| {
                json!({ "name": name, "countDelta": count_delta, "bytesDelta": bytes_delta })
            }).collect::<Vec<_>>(),
        })
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    s.chars().take(max).collect()
}
