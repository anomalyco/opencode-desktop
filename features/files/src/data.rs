//! Workspace file paths, tab IDs (`file://<path>`), fuzzy ranking, and server/fixture loaders.

use std::collections::{BTreeSet, HashMap};

use opencode_kit::client::Server;
use opencode_search::query::{encode_fs_path, percent_encode};
use rune_kit::{FileNode, FileTree, TreeModel};
use serde_json::Value;

#[allow(unused_imports)]
pub use opencode_search::file_rank::{
    OPEN_TAB_ID, OPEN_TAB_KEY, document_path, file_tab_id, file_tab_key, file_tab_path, normalize_path,
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

/// A skill's file that is no file on a disk (`isVirtualSkill`, official
/// `packages/app/src/workspaces/files/model.tsx:245`): `skill://<id>/<file>`, or a built-in
/// skill's older `/builtin/<file>`.
pub fn is_virtual_skill(path: &str) -> bool {
    skill_file(path).is_some() || path.starts_with("/builtin/")
}

/// The `<id>/<file>` of a `skill://<id>/<file>` path.
fn skill_file(path: &str) -> Option<&str> {
    path.get(..8).filter(|scheme| scheme.eq_ignore_ascii_case("skill://")).map(|_| &path[8..])
}

/// Whether `path` is no file of the session's folder: one outside it by its absolute path, or a
/// skill's virtual file. Such a file is in no tree, is not watched and opens in no app.
pub fn is_outside(path: &str) -> bool {
    is_virtual_skill(path) || opencode_kit::path::is_absolute(path)
}

/// A skill's virtual file (`readBuiltin`, `model.tsx:257-269`, anomalyco/opencode#66201):
/// `skill://<id>/<file>` is read through the server, which loads what a skill serves on demand
/// (an MCP server's); a `/builtin/` path, and a `skill://` one on a server from before that
/// endpoint (it answers 404), has its body inline in the skills list, where official read both
/// until that change.
fn fetch_skill_file(server: &Server, directory: &str, path: &str) -> FilePayload {
    let location = format!("location%5Bdirectory%5D={}", percent_encode(directory));
    if let Some(rest) = skill_file(path) {
        let (id, file) = rest.split_once('/').unwrap_or((rest, ""));
        match server.get_bytes(&format!("/api/experimental/skill/{}/file/{}?{location}", percent_encode(id), encode_fs_path(file))) {
            Ok(bytes) => return payload(path, bytes),
            Err(e) if e.status() == Some(404) => {}
            Err(e) => return FilePayload::Failed(e),
        }
    }
    let list = match server.get_bytes(&format!("/api/skill?{location}")) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap_or_default(),
        Err(e) if e.status() == Some(404) => return FilePayload::Missing,
        Err(e) => return FilePayload::Failed(e),
    };
    let body = list["data"]
        .as_array()
        .or_else(|| list.as_array())
        .into_iter()
        .flatten()
        .find(|skill| skill["path"].as_str() == Some(path))
        .and_then(|skill| skill["content"].as_str())
        .filter(|content| !content.is_empty());
    body.map_or(FilePayload::Missing, |content| payload(path, content.as_bytes().to_vec()))
}

pub fn fetch_live_file_payload(server: &Server, directory: &str, path: &str) -> FilePayload {
    if is_virtual_skill(path) {
        return fetch_skill_file(server, directory, path);
    }
    // A file outside the session's folder is read with its own folder as the Location and its
    // name as the path, as Desktop reads it (`/api/fs/read/todo.md?location[directory]=<folder>`).
    let (directory, path) = if opencode_kit::path::is_absolute(path) {
        let folder = opencode_kit::path::parent(path);
        (if folder.is_empty() { "/" } else { folder }, opencode_kit::path::basename(path))
    } else {
        (directory, path)
    };
    let encoded = encode_fs_path(path);
    let url = format!(
        "/api/fs/read/{encoded}?location%5Bdirectory%5D={}",
        percent_encode(directory)
    );
    match server.get_bytes(&url) {
        Ok(bytes) => payload(path, bytes),
        Err(e) if e.status() == Some(404) => FilePayload::Missing,
        Err(e) => FilePayload::Failed(e),
    }
}

/// What the viewer shows of file `path`'s `bytes`: a picture, a table, a page, text, or a
/// binary file's size.
fn payload(path: &str, bytes: Vec<u8>) -> FilePayload {
    if super::image::is_image(path) {
        return FilePayload::Image(bytes);
    }
    if bytes.contains(&0) {
        return FilePayload::Binary(bytes.len());
    }
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
            let norm = document_path(k);
            if !norm.is_empty() {
                // A file outside the folder (an absolute path) and a skill's virtual file have a
                // body and are no row of the tree.
                if !is_outside(&norm) {
                    files.insert(norm.clone());
                }
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
