//! Review panel data model: server & fixture diff loading, off-main-thread patch parsing,
//! depth-first directory-first path sorting (`review/kinds.ts`), and file filtering.

use std::collections::HashMap;

use rune_kit::diff::{FileDiff, FileStatus, parse};
use rune_kit::{FileNode, FileTree, TreeModel};
use serde_json::Value;

use super::ChangeMode;
use opencode_kit::client::Server;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewFileMeta {
    pub path: String,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
}

pub struct FetchedDiffs {
    pub session_id: String,
    pub mode: ChangeMode,
    pub revision: u64,
    pub raw_diff: Value,
    pub branch_current: Option<String>,
    pub branch_default: Option<String>,
    pub no_git: bool,
}

pub fn normalize_path(value: &str) -> String {
    let replaced = value.replace('\\', "/");
    let trimmed = replaced.trim_matches('/');
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_slash = false;
    for ch in trimmed.chars() {
        if ch == '/' {
            if !prev_slash {
                out.push('/');
            }
            prev_slash = true;
        } else {
            out.push(ch);
            prev_slash = false;
        }
    }
    out
}

/// Depth-first file order with directories before sibling files (`review/kinds.ts::sortReviewPaths`).
pub fn sort_review_paths(paths: &[String]) -> Vec<String> {
    #[derive(Clone)]
    struct Node {
        name: String,
        path: String,
        is_file: bool,
        original: String,
    }
    let mut nodes: HashMap<String, Node> = HashMap::new();
    for raw in paths {
        let norm = normalize_path(raw);
        if norm.is_empty() {
            continue;
        }
        let parts: Vec<&str> = norm.split('/').collect();
        for (idx, name) in parts.iter().enumerate() {
            let p = parts[..=idx].join("/");
            if nodes.contains_key(&p) {
                continue;
            }
            let leaf = idx + 1 == parts.len();
            nodes.insert(
                p.clone(),
                Node {
                    name: (*name).to_owned(),
                    path: p.clone(),
                    is_file: leaf,
                    original: if leaf { raw.clone() } else { p },
                },
            );
        }
    }
    let mut children: HashMap<String, Vec<Node>> = HashMap::new();
    for node in nodes.into_values() {
        let parent = match node.path.rfind('/') {
            Some(i) => node.path[..i].to_owned(),
            None => String::new(),
        };
        children.entry(parent).or_default().push(node);
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| match (a.is_file, b.is_file) {
            (false, true) => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
    }
    let mut out = Vec::with_capacity(paths.len());
    let mut stack: Vec<Node> = children.remove("").unwrap_or_default().into_iter().rev().collect();
    while let Some(node) = stack.pop() {
        if node.is_file {
            out.push(node.original);
        } else if let Some(kids) = children.get(&node.path) {
            stack.extend(kids.iter().rev().cloned());
        }
    }
    out
}

pub fn filter_review_files(paths: &[String], query: &str) -> Vec<String> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return paths.to_vec();
    }
    paths
        .iter()
        .filter(|p| p.to_ascii_lowercase().contains(&q))
        .cloned()
        .collect()
}

/// Parses one `FileDiff.Info` JSON entry from `/api/vcs/diff` or `/api/session/:id/diff`.
pub fn parse_diff_entry(v: &Value) -> Option<FileDiff> {
    let raw_file = v["file"].as_str()?;
    let file = normalize_path(raw_file);
    if file.is_empty() {
        return None;
    }
    let status_str = v["status"].as_str().unwrap_or("modified");
    let patch = v["patch"].as_str().unwrap_or("");
    let trimmed = patch.trim_start();
    let parsed = if trimmed.starts_with("diff --git ") || trimmed.starts_with("--- ") {
        parse(patch).into_iter().next()
    } else if trimmed.starts_with("@@ ") {
        let old_hdr = if status_str == "added" {
            "/dev/null".to_owned()
        } else {
            format!("a/{file}")
        };
        let new_hdr = if status_str == "deleted" {
            "/dev/null".to_owned()
        } else {
            format!("b/{file}")
        };
        let full = format!("--- {old_hdr}\n+++ {new_hdr}\n{patch}");
        parse(&full).into_iter().next()
    } else {
        None
    };

    if let Some(mut fd) = parsed {
        match status_str {
            "added" => {
                fd.old_path = None;
                fd.new_path = Some(file);
                fd.status = FileStatus::Added;
            }
            "deleted" => {
                fd.old_path = Some(file);
                fd.new_path = None;
                fd.status = FileStatus::Deleted;
            }
            _ => {
                if fd.new_path.is_none() && fd.old_path.is_none() {
                    fd.old_path = Some(file.clone());
                    fd.new_path = Some(file);
                }
            }
        }
        return Some(fd);
    }

    let (old_p, new_p) = match status_str {
        "added" => (None, Some(file)),
        "deleted" => (Some(file), None),
        _ => (Some(file.clone()), Some(file)),
    };
    Some(FileDiff::between(old_p, new_p, "", ""))
}

pub fn parse_diff_list(v: &Value) -> (Vec<FileDiff>, Vec<ReviewFileMeta>) {
    let arr = v["data"].as_array().or_else(|| v.as_array());
    let Some(items) = arr else {
        return (Vec::new(), Vec::new());
    };
    let mut by_path: HashMap<String, (FileDiff, ReviewFileMeta)> = HashMap::new();
    let mut raw_paths = Vec::with_capacity(items.len());
    for item in items {
        let Some(fd) = parse_diff_entry(item) else { continue };
        let path = fd.path().to_owned();
        if path.is_empty() || by_path.contains_key(&path) {
            continue;
        }
        let additions = item["additions"]
            .as_u64()
            .map_or_else(|| fd.additions() as u32, |n| n as u32);
        let deletions = item["deletions"]
            .as_u64()
            .map_or_else(|| fd.deletions() as u32, |n| n as u32);
        let meta = ReviewFileMeta {
            path: path.clone(),
            status: fd.status,
            additions,
            deletions,
        };
        raw_paths.push(path.clone());
        by_path.insert(path, (fd, meta));
    }
    let sorted = sort_review_paths(&raw_paths);
    let mut files = Vec::with_capacity(sorted.len());
    let mut metas = Vec::with_capacity(sorted.len());
    for p in sorted {
        if let Some((fd, meta)) = by_path.remove(&p) {
            files.push(fd);
            metas.push(meta);
        }
    }
    (files, metas)
}

pub fn build_change_tree(metas: &[ReviewFileMeta]) -> TreeModel<FileNode<ReviewFileMeta>> {
    let mut tree = FileTree::new().build(metas.iter().map(|m| (m.path.as_str(), m.clone())));
    tree.expand_all();
    if let Some(first) = metas.first() {
        tree.select(&first.path);
    }
    tree
}

pub fn fetch_live_diffs(
    server: &Server,
    session_id: String,
    directory: String,
    mode: ChangeMode,
    revision: u64,
) -> FetchedDiffs {
    let loc = format!("location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
    let vcs_val = server.get(&format!("/api/vcs?{loc}")).ok();
    let branch_current = vcs_val
        .as_ref()
        .and_then(|v| v["data"]["branch"]["current"].as_str().or_else(|| v["branch"]["current"].as_str()))
        .map(str::to_owned);
    let branch_default = vcs_val
        .as_ref()
        .and_then(|v| v["data"]["branch"]["default"].as_str().or_else(|| v["branch"]["default"].as_str()))
        .map(str::to_owned);
    let provider = vcs_val
        .as_ref()
        .and_then(|v| v["data"]["provider"].as_str().or_else(|| v["provider"].as_str()));
    let no_git = vcs_val.is_some() && provider.is_none() && branch_current.is_none();

    let raw_diff = match mode {
        ChangeMode::Turn => server.get(&format!("/api/session/{}/diff", crate::switcher::encode(&session_id))),
        ChangeMode::Branch => server.get(&format!("/api/vcs/diff?{loc}&mode=branch")),
        ChangeMode::Git => server.get(&format!("/api/vcs/diff?{loc}&mode=working")),
    }
    .unwrap_or(Value::Null);

    FetchedDiffs {
        session_id,
        mode,
        revision,
        raw_diff,
        branch_current,
        branch_default,
        no_git,
    }
}

pub fn encode_fs_path(path: &str) -> String {
    path.split('/')
        .filter(|seg| !seg.is_empty())
        .map(crate::switcher::encode)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn fetch_live_file_text(server: &Server, directory: &str, path: &str) -> Option<String> {
    let encoded = encode_fs_path(path);
    let url = format!(
        "/api/fs/read/{encoded}?location%5Bdirectory%5D={}",
        crate::switcher::encode(directory)
    );
    let bytes = server.get_bytes(&url).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

pub fn fixture_diff_count(fixture: &Value, session_id: &str) -> usize {
    fixture["diffs"]
        .get(session_id)
        .and_then(|s| s.get("git"))
        .or_else(|| fixture["diffs"].get("git"))
        .or_else(|| fixture["diffs"].get("default"))
        .and_then(|v| v["data"].as_array().or_else(|| v.as_array()))
        .map_or(0, Vec::len)
}

pub fn load_fixture_diffs(
    fixture: &Value,
    session_id: &str,
    mode: ChangeMode,
) -> (Vec<FileDiff>, Vec<ReviewFileMeta>, Option<String>, Option<String>, HashMap<String, String>) {
    let branch_cur = fixture["vcs"]["branch"]["current"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| Some("fix/reconnect-watchdog".to_owned()));
    let branch_def = fixture["vcs"]["branch"]["default"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| Some("main".to_owned()));

    let key = match mode {
        ChangeMode::Git => "git",
        ChangeMode::Branch => "branch",
        ChangeMode::Turn => "turn",
    };
    let diff_val = fixture["diffs"]
        .get(session_id)
        .and_then(|s| s.get(key).or_else(|| s.get("git")))
        .or_else(|| fixture["diffs"].get(key))
        .or_else(|| fixture["diffs"].get("default"));

    let (files, meta) = match diff_val {
        Some(v) => parse_diff_list(v),
        None => (Vec::new(), Vec::new()),
    };

    let mut texts = HashMap::new();
    if let Some(obj) = fixture["file_contents"].as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                texts.insert(normalize_path(k), s.to_owned());
            }
        }
    }
    (files, meta, branch_cur, branch_def, texts)
}
