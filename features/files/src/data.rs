//! Workspace file paths, tab IDs (`file://<path>`), fuzzy ranking, and server/fixture loaders.

use std::collections::{BTreeSet, HashMap};

use opencode_kit::client::Server;
use opencode_search::query::{encode_fs_path, percent_encode};
use rune_kit::{FileNode, FileTree, TreeModel};
use serde_json::Value;

#[allow(unused_imports)]
pub use opencode_search::file_rank::{
    OPEN_TAB_ID, OPEN_TAB_KEY, file_tab_id, file_tab_key, file_tab_path, normalize_path,
    parse_path_and_line,
};

/// `path` inside `directory`, relative to it; `None` for a path outside it.
pub fn relative_path(path: &str, directory: &str) -> Option<String> {
    let rel = if opencode_kit::path::is_absolute(path) {
        opencode_kit::path::strip_dir(path, directory)?
    } else {
        path.trim_start_matches("./")
    };
    let rel = normalize_path(rel);
    (!rel.is_empty()).then_some(rel)
}

#[derive(Clone, Debug)]
pub enum FilePayload {
    Text(String),
    Html { source: String, document: std::sync::Arc<super::html::Page> },
    Table { source: String, document: std::sync::Arc<super::delimited::Table> },
    Binary(usize),
    /// A picture file's bytes (`image::is_image`).
    Image(Vec<u8>),
    Missing,
    /// The read failed for another reason (the server away, a sign-in page answering): the raw
    /// error, which the panel says in words and which a reopen reads again. Desktop's
    /// `FileEntryState` tells these apart too (`content-machine.ts`).
    Failed(opencode_kit::wire_error::WireError),
}

pub fn fetch_live_file_payload(server: &Server, directory: &str, path: &str) -> FilePayload {
    let encoded = encode_fs_path(path);
    let url = format!(
        "/api/fs/read/{encoded}?location%5Bdirectory%5D={}",
        percent_encode(directory)
    );
    match server.get_bytes(&url) {
        Ok(bytes) if super::image::is_image(path) => FilePayload::Image(bytes),
        Ok(bytes) => {
            if bytes.contains(&0) {
                FilePayload::Binary(bytes.len())
            } else {
                match String::from_utf8(bytes) {
                    Ok(s) => match super::delimited::Table::parse(path, &s) {
                        Some(document) => FilePayload::Table { source: s, document: document.into() },
                        None => match super::html::Page::parse(path, &s) {
                            Some(document) => FilePayload::Html { source: s, document },
                            None => FilePayload::Text(s),
                        },
                    },
                    Err(e) => FilePayload::Binary(e.into_bytes().len()),
                }
            }
        }
        Err(e) if e.status() == Some(404) => FilePayload::Missing,
        Err(e) => FilePayload::Failed(e),
    }
}

/// Lists direct children of `rel_dir` (`""` for workspace root) via `/api/fs/list`.
pub fn fetch_live_dir(server: &Server, directory: &str, rel_dir: &str) -> Vec<(String, bool)> {
    try_fetch_live_dir(server, directory, rel_dir).unwrap_or_default()
}

/// `fetch_live_dir`, `None` when the server didn't list it (so nothing is taken as gone).
pub fn try_fetch_live_dir(server: &Server, directory: &str, rel_dir: &str) -> Option<Vec<(String, bool)>> {
    let mut url = format!(
        "/api/fs/list?location%5Bdirectory%5D={}",
        percent_encode(directory)
    );
    if !rel_dir.is_empty() {
        url.push_str(&format!("&path={}", percent_encode(rel_dir)));
    }
    let v = server.get(&url).ok()?;
    let arr = v["data"].as_array().or_else(|| v.as_array())?;
    Some(
        arr.iter()
            .filter_map(|e| {
                let p = normalize_path(e["path"].as_str()?);
                if p.is_empty() {
                    return None;
                }
                let is_dir = e["type"].as_str() == Some("directory");
                Some((p, is_dir))
            })
            .collect(),
    )
}

/// Searches workspace files via `/api/fs/find`, at most `limit` of them.
pub fn fetch_live_find(server: &Server, directory: &str, query: &str, limit: usize) -> Vec<String> {
    let url = format!(
        "/api/fs/find?location%5Bdirectory%5D={}&query={}&type=file&limit={limit}",
        percent_encode(directory),
        percent_encode(query)
    );
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
    opencode_search::file_rank::rank_file_query_with(paths, query, rune::text::compare_names)
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
