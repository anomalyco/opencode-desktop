//! Extension-filtered frame-tree parser for `browser.frames` and target inspection
//! (`packages/gui-plugins/src/browser/chromium.ts:2035-2075`).

use std::collections::HashSet;

use browser_policy::extension_origin;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameInfo {
    pub id: String,
    #[serde(rename = "parentID", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub url: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedFrameTree {
    /// Allowed page frames only (every extension frame and its subtree is omitted).
    pub frames: Vec<FrameInfo>,
    /// Frame IDs excluded because they (or an ancestor) belong to a browser extension.
    pub extension_frame_ids: HashSet<String>,
}

/// Parses a CDP `Page.getFrameTree` result, stripping any extension-originated frame and its
/// descendants so agent tools can never list, snapshot, evaluate, or wait inside extension UI.
pub fn parse_frame_tree(result: &Value) -> ParsedFrameTree {
    let mut out = ParsedFrameTree::default();
    if let Some(root) = result.get("frameTree") {
        walk(root, None, false, &mut out);
    }
    out
}

fn walk(node: &Value, parent_id: Option<&str>, in_ext: bool, out: &mut ParsedFrameTree) {
    let Some(frame) = node.get("frame") else { return };
    let id = frame.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
    let url = frame.get("url").and_then(Value::as_str).unwrap_or("").to_owned();
    let name = frame.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
    let ext = in_ext || extension_origin(&url);
    if ext {
        if !id.is_empty() {
            out.extension_frame_ids.insert(id.clone());
        }
    } else if !id.is_empty() {
        out.frames.push(FrameInfo {
            id: id.clone(),
            parent_id: parent_id.map(str::to_owned),
            url: truncate_chars(&url, 100_000),
            name: truncate_chars(&name, 2_048),
        });
    }
    if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
        let next_parent = if ext || id.is_empty() { None } else { Some(id.as_str()) };
        for child in children {
            walk(child, next_parent, ext, out);
        }
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    s.chars().take(max).collect()
}
