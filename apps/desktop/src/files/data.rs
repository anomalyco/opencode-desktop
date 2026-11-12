//! Workspace file paths, tab IDs (`file://<path>`), fuzzy ranking, and server/fixture loaders.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use rune_kit::{FileNode, FileTree, TreeModel};
use serde_json::Value;

use crate::review::data::{encode_fs_path, normalize_path};
use opencode_kit::client::Server;

pub const OPEN_TAB_ID: &str = "open";
pub const OPEN_TAB_KEY: &str = "file:open";

pub fn file_tab_id(path: &str) -> String {
    format!("//{}", normalize_path(path))
}

pub fn file_tab_key(path: &str) -> String {
    format!("file:{}", file_tab_id(path))
}

pub fn file_tab_path(tab_id: &str) -> Option<String> {
    let rest = tab_id.strip_prefix("//")?;
    let norm = normalize_path(rest);
    (!norm.is_empty()).then_some(norm)
}

/// Splits `"src/net/stream.ts:24"` or `"src/net/stream.ts#L24"` into `("src/net/stream.ts", Some(24))`.
pub fn parse_path_and_line(raw: &str) -> (String, Option<u32>) {
    let trimmed = raw.trim();
    if let Some((base, frag)) = trimmed.rsplit_once("#L") {
        let digits: String = frag.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(line) = digits.parse::<u32>() {
            return (base.trim().to_owned(), Some(line.max(1)));
        }
    }
    if let Some((base, tail)) = trimmed.rsplit_once(':')
        && !base.is_empty()
        && let Ok(line) = tail.split(':').next().unwrap_or("").parse::<u32>()
    {
        return (base.trim().to_owned(), Some(line.max(1)));
    }
    (trimmed.to_owned(), None)
}

#[derive(Clone, Debug)]
pub enum FilePayload {
    Text(String),
    Binary(usize),
    /// A picture file's bytes (`image::is_image`).
    Image(Vec<u8>),
    Missing,
    /// The read failed for another reason (the server away, a sign-in page answering): the raw
    /// error, which the panel says in words and which a reopen reads again. Desktop's
    /// `FileEntryState` tells these apart too (`content-machine.ts`).
    Failed(String),
}

pub fn fetch_live_file_payload(server: &Server, directory: &str, path: &str) -> FilePayload {
    let encoded = encode_fs_path(path);
    let url = format!(
        "/api/fs/read/{encoded}?location%5Bdirectory%5D={}",
        crate::switcher::encode(directory)
    );
    match server.get_bytes(&url) {
        Ok(bytes) if super::image::is_image(path) => FilePayload::Image(bytes),
        Ok(bytes) => {
            if bytes.contains(&0) {
                FilePayload::Binary(bytes.len())
            } else {
                match String::from_utf8(bytes) {
                    Ok(s) => FilePayload::Text(s),
                    Err(e) => FilePayload::Binary(e.into_bytes().len()),
                }
            }
        }
        Err(e) if opencode_kit::wire_error::WireError::parse(&e).and_then(|w| w.status()) == Some(404) => FilePayload::Missing,
        Err(e) => FilePayload::Failed(e),
    }
}

/// Lists direct children of `rel_dir` (`""` for workspace root) via `/api/fs/list`.
pub fn fetch_live_dir(server: &Server, directory: &str, rel_dir: &str) -> Vec<(String, bool)> {
    let mut url = format!(
        "/api/fs/list?location%5Bdirectory%5D={}",
        crate::switcher::encode(directory)
    );
    if !rel_dir.is_empty() {
        url.push_str(&format!("&path={}", crate::switcher::encode(rel_dir)));
    }
    let Ok(v) = server.get(&url) else {
        return Vec::new();
    };
    let Some(arr) = v["data"].as_array().or_else(|| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| {
            let p = normalize_path(e["path"].as_str()?);
            if p.is_empty() {
                return None;
            }
            let is_dir = e["type"].as_str() == Some("directory");
            Some((p, is_dir))
        })
        .collect()
}

/// Searches workspace files via `/api/fs/find`.
pub fn fetch_live_find(server: &Server, directory: &str, query: &str) -> Vec<String> {
    let url = format!(
        "/api/fs/find?location%5Bdirectory%5D={}&query={}&type=file&limit=100",
        crate::switcher::encode(directory),
        crate::switcher::encode(query)
    );
    let _ = Duration::from_millis(0);
    let Ok(v) = server.get(&url) else {
        return Vec::new();
    };
    let Some(arr) = v["data"].as_array().or_else(|| v.as_array()) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in arr {
        if item["type"].as_str() == Some("directory") {
            continue;
        }
        let Some(raw) = item["path"].as_str().or_else(|| item.as_str()) else {
            continue;
        };
        let norm = normalize_path(raw);
        if !norm.is_empty() && seen.insert(norm.clone()) {
            out.push(norm);
        }
    }
    out
}

/// Builds a `TreeModel` over known files and directories; empty directories keep a placeholder
/// sentinel child so they render with an expandable folder chevron until opened.
pub fn build_workspace_tree(
    files: &BTreeSet<String>,
    dirs: &BTreeSet<String>,
    expanded: &BTreeSet<String>,
) -> TreeModel<FileNode<bool>> {
    let mut entries: Vec<(String, bool)> = files.iter().map(|p| (p.clone(), false)).collect();
    for d in dirs {
        let prefix = format!("{d}/");
        let has_child = files.iter().any(|f| f.starts_with(&prefix))
            || dirs.iter().any(|other| other.starts_with(&prefix));
        if !has_child {
            entries.push((format!("{d}/__placeholder__"), true));
        }
    }
    let mut tree = FileTree::new()
        .compact_folders(false)
        .build(entries.iter().map(|(p, placeholder)| (p.as_str(), *placeholder)));
    for d in expanded {
        tree.set_expanded(d, true);
    }
    tree
}

/// Case-insensitive fuzzy/substring scorer for `⌘P` and sidebar file search.
pub fn rank_file_query(paths: &[String], query: &str) -> Vec<String> {
    let (clean, _) = parse_path_and_line(query);
    let needle = clean.trim().to_lowercase();
    if needle.is_empty() {
        return paths.to_vec();
    }
    let mut scored: Vec<(i32, &String)> = paths
        .iter()
        .filter_map(|p| score_path(p, &needle).map(|sc| (sc, p)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| rune::text::compare_names(a.1, b.1)));
    scored.into_iter().map(|(_, p)| p.clone()).collect()
}

fn score_path(path: &str, needle_lower: &str) -> Option<i32> {
    let hay = path.to_lowercase();
    let base = opencode_kit::path::basename(&hay);
    if base == needle_lower {
        return Some(1000 - path.len() as i32);
    }
    if base.starts_with(needle_lower) {
        return Some(850 - path.len() as i32);
    }
    if base.contains(needle_lower) {
        return Some(700 - path.len() as i32);
    }
    if hay.contains(needle_lower) {
        return Some(500 - path.len() as i32);
    }
    // Ordered subsequence match across path characters.
    let mut chars = hay.chars();
    for nc in needle_lower.chars() {
        chars.find(|&hc| hc == nc)?;
    }
    Some(200 - path.len() as i32)
}

pub fn load_fixture_workspace(fixture: &Value) -> (BTreeSet<String>, HashMap<String, String>) {
    let mut files = BTreeSet::new();
    let mut contents = HashMap::new();
    if let Some(obj) = fixture["file_contents"].as_object() {
        for (k, v) in obj {
            let norm = normalize_path(k);
            if !norm.is_empty() {
                files.insert(norm.clone());
                if let Some(s) = v.as_str() {
                    contents.insert(norm, s.to_owned());
                }
            }
        }
    }
    if let Some(arr) = fixture["files"].as_array() {
        for item in arr {
            if item["type"].as_str() == Some("directory") {
                continue;
            }
            if let Some(raw) = item["path"].as_str() {
                let norm = normalize_path(raw);
                if !norm.is_empty() {
                    files.insert(norm.clone());
                    contents.entry(norm.clone()).or_insert_with(|| {
                        format!("// {norm}\n")
                    });
                }
            }
        }
    }
    (files, contents)
}
