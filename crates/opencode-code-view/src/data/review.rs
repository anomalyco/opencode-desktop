//! Review panel data model: server & fixture diff loading, off-main-thread patch parsing,
//! depth-first directory-first path sorting (`review/kinds.ts`), and file filtering.

use std::collections::{HashMap, HashSet};

use opencode_kit::client::Server;
use opencode_kit::wire_error::WireError;
use rune_kit::diff::{DiffMode, FileDiff, FileStatus, parse};
use rune_kit::{FileNode, FileTree, TreeModel};
use serde_json::Value;

use crate::{DiffDocState, ExpandMode, PreparedDiffDoc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChangeMode {
    #[default]
    Git,
    Branch,
    Turn,
}

impl ChangeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Git => opencode_i18n::t!("ui.sessionReview.title.git"),
            Self::Branch => opencode_i18n::t!("ui.sessionReview.title.branch"),
            Self::Turn => opencode_i18n::t!("ui.sessionReview.title.lastTurn"),
        }
    }

    pub fn empty_label(self) -> &'static str {
        match self {
            Self::Git => opencode_i18n::t!("plugin.review.empty.git"),
            Self::Branch => opencode_i18n::t!("plugin.review.empty.branch"),
            Self::Turn => opencode_i18n::t!("plugin.review.noChanges"),
        }
    }

    /// The contract's name for it (`DiffSource`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Branch => "branch",
            Self::Turn => "turn",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "git" => Some(Self::Git),
            "branch" => Some(Self::Branch),
            "turn" => Some(Self::Turn),
            _ => None,
        }
    }
}

/// Percent-encodes a query parameter value or path segment (RFC 3986 unreserved characters kept).
pub fn encode_uri_component(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewFileMeta {
    pub path: String,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    /// Its place in the server's list, the order the narrow Changes view keeps (`mobile.tsx`
    /// passes `review.diffs()` unsorted; only the wide tree sorts).
    pub listed: usize,
}

pub struct ReviewFetchSpec {
    pub session_id: String,
    pub directory: String,
    pub mode: ChangeMode,
    pub revision: u64,
    pub diff_mode: DiffMode,
    pub expand_mode: ExpandMode,
    pub collapsed: HashSet<String>,
    pub active_path: Option<String>,
    pub find_query: Option<String>,
    pub narrow_open: Vec<String>,
}

pub struct PreparedReview {
    pub diffs: PreparedDiffDoc,
    pub meta: Vec<ReviewFileMeta>,
    pub listed: Vec<String>,
    pub tree: TreeModel<FileNode<ReviewFileMeta>>,
    pub folders: Vec<String>,
    pub count: usize,
    pub active_path: Option<String>,
    pub narrow: Vec<(String, PreparedDiffDoc)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchedVcs {
    pub branch_current: Option<String>,
    pub branch_default: Option<String>,
    pub lacks_provider: bool,
}

pub struct FetchedDiffs {
    pub session_id: String,
    pub directory: String,
    pub server_base: String,
    pub mode: ChangeMode,
    pub revision: u64,
    /// The prepared diff state, or why it couldn't be read.
    pub changes: Result<PreparedReview, WireError>,
    pub branch_current: Option<String>,
    pub branch_default: Option<String>,
    pub no_git: bool,
}

/// The counted rows and the previewable files are different sets: Git returns one `dir/` row
/// for an untracked folder, not every file below it (`review/model.ts`, `untracked: "folders"`).
pub struct ChangeList {
    pub files: Vec<FileDiff>,
    pub meta: Vec<ReviewFileMeta>,
    pub folders: Vec<String>,
    pub count: usize,
    pub more: bool,
}

impl ChangeList {
    pub fn read(v: Value) -> Self {
        let rows = v["data"].as_array().or_else(|| v.as_array()).map_or(&[][..], Vec::as_slice);
        let more = rows.iter().any(|r| r["more"] == true);
        let folders = rows
            .iter()
            .filter(|r| r["more"] != true)
            .filter_map(|r| r["file"].as_str())
            .filter(|p| p.ends_with('/'))
            .map(normalize_path)
            .filter(|p| !p.is_empty())
            .collect();
        let (files, meta) = parse_diff_list(&v);
        Self { files, meta, folders, count: rows.len() - usize::from(more), more }
    }
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
            _ => rune::text::compare_names(&a.name, &b.name),
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
    let parsed = if trimmed.starts_with("@@ ") {
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
        // Any file header rune-kit reads: git's, or the `Index:` and `===` lines jsdiff writes
        // before its `---`/`+++` (the server's hg and capped patches, a tool's edit).
        parse(patch).into_iter().next()
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
        return Some(if full_context_header(patch) { fd.fold(REVIEW_CONTEXT) } else { fd });
    }

    let (old_p, new_p) = match status_str {
        "added" => (None, Some(file)),
        "deleted" => (Some(file), None),
        _ => (Some(file.clone()), Some(file)),
    };
    Some(FileDiff::between(old_p, new_p, "", ""))
}

/// Unchanged lines kept around each change when a whole-file patch folds, as Desktop's review
/// splits one (`hunkContext` in session-ui's `session-diff.ts`).
const REVIEW_CONTEXT: u32 = 4;

/// Whether the patch comes from a producer that writes full context (a git header, or jsdiff's
/// `---`/`+++` lines ending in a tab), so a single hunk from its first line is the whole file:
/// Desktop's `completePatch` test.
fn full_context_header(patch: &str) -> bool {
    let header = patch.find("\n@@ ").map_or(patch, |i| &patch[..i]);
    let tabbed = |l: &str, p: &str| l.starts_with(p) && l.trim_end_matches('\r').ends_with('\t');
    header.trim_start().starts_with("diff --git ")
        || header.lines().zip(header.lines().skip(1)).any(|(a, b)| tabbed(a, "--- ") && tabbed(b, "+++ "))
}

pub fn parse_diff_list(v: &Value) -> (Vec<FileDiff>, Vec<ReviewFileMeta>) {
    let arr = v["data"].as_array().or_else(|| v.as_array());
    let Some(items) = arr else {
        return (Vec::new(), Vec::new());
    };
    let mut by_path: HashMap<String, (FileDiff, ReviewFileMeta)> = HashMap::new();
    let mut raw_paths = Vec::with_capacity(items.len());
    for item in items {
        if item["more"] == true || item["file"].as_str().is_some_and(|p| p.ends_with('/')) {
            continue;
        }
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
            listed: raw_paths.len(),
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

/// The changed files' paths in the order the server listed them (newest change first).
pub fn listed_paths(v: &Value) -> Vec<String> {
    let items = v["data"].as_array().or_else(|| v.as_array()).map_or(&[][..], Vec::as_slice);
    items
        .iter()
        .filter_map(|i| i["file"].as_str())
        .map(normalize_path)
        .filter(|p| !p.is_empty())
        .collect()
}

/// Builds the change tree with every folder open except those in `collapsed`, so a refetch keeps
/// the folders the user closed (`collapsed_folders` of the old tree).
pub fn build_change_tree(
    metas: &[ReviewFileMeta],
    collapsed: &HashSet<String>,
) -> TreeModel<FileNode<ReviewFileMeta>> {
    let mut tree = FileTree::new()
        .compact_folders(false)
        .build(metas.iter().map(|m| (m.path.as_str(), m.clone())));
    tree.expand_all();
    for id in collapsed {
        tree.set_expanded(id, false);
    }
    if let Some(first) = metas.first() {
        tree.select(&first.path);
    }
    tree
}

/// Inserts untracked folder placeholders into `tree` and re-sorts folders before sibling files.
pub fn insert_folder_nodes<'a, T>(
    tree: &mut TreeModel<FileNode<T>>,
    folders: impl IntoIterator<Item = &'a str>,
) {
    let mut any = false;
    for path in folders {
        any = true;
        if tree.get(path).is_some() {
            continue;
        }
        let mut parent = None;
        for end in path.match_indices('/').map(|(i, _)| i).chain(std::iter::once(path.len())) {
            let id = &path[..end];
            let name = id.rsplit('/').next().unwrap_or(id);
            tree.insert(
                parent,
                id,
                FileNode { dir: true, path: id.to_owned(), name: name.to_owned(), data: None },
            );
            parent = Some(id);
        }
    }
    if any {
        tree.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| rune::text::compare_names(&a.name, &b.name)));
    }
}

pub fn collapsed_folders<T>(tree: &TreeModel<T>) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut stack: Vec<String> = tree.children(None).map(str::to_owned).collect();
    while let Some(id) = stack.pop() {
        if tree.has_children(&id) {
            if !tree.is_expanded(&id) {
                out.insert(id.clone());
            }
            stack.extend(tree.children(Some(&id)).map(str::to_owned));
        }
    }
    out
}

/// Prepares inline single-file diffs off the UI thread for each open narrow-review path.
pub fn prepare_narrow_paths(paths: &[String], files: &[FileDiff]) -> Vec<(String, PreparedDiffDoc)> {
    paths
        .iter()
        .filter_map(|path| {
            files
                .iter()
                .find(|f| f.path() == path)
                .map(|f| (path.clone(), DiffDocState::inline_prepared(f.clone())))
        })
        .collect()
}

/// Reads branch metadata and provider presence for `directory` (`/api/vcs`), independent of
/// `/api/vcs/diff` so review diffs never wait on VCS metadata (`packages/client/src/solid/data.ts:1596`,
/// `packages/gui-plugins/src/review/model.ts:183, 241-253`).
pub fn fetch_live_vcs(server: &Server, directory: &str) -> Option<FetchedVcs> {
    if directory.is_empty() {
        return None;
    }
    let loc = format!("location%5Bdirectory%5D={}", encode_uri_component(directory));
    let vcs_val = server.get(&format!("/api/vcs?{loc}")).ok()?;
    let branch_current = vcs_val["data"]["branch"]["current"]
        .as_str()
        .or_else(|| vcs_val["branch"]["current"].as_str())
        .map(str::to_owned);
    let branch_default = vcs_val["data"]["branch"]["default"]
        .as_str()
        .or_else(|| vcs_val["branch"]["default"].as_str())
        .map(str::to_owned);
    let provider = vcs_val["data"]["provider"]
        .as_str()
        .or_else(|| vcs_val["provider"].as_str());
    let lacks_provider = provider.is_none() && branch_current.is_none();
    Some(FetchedVcs { branch_current, branch_default, lacks_provider })
}

/// Fetches and prepares only the diff list (`/api/vcs/diff` or `/api/session/:id/diff`),
/// without blocking on `/api/vcs`.
pub fn fetch_live_diff_only(server: &Server, spec: ReviewFetchSpec) -> FetchedDiffs {
    let loc = format!("location%5Bdirectory%5D={}", encode_uri_component(&spec.directory));
    let raw_diff = match spec.mode {
        ChangeMode::Turn => server.get(&format!("/api/session/{}/diff", encode_uri_component(&spec.session_id))),
        ChangeMode::Branch => server.get(&format!("/api/vcs/diff?{loc}&mode=branch&untracked=folders")),
        ChangeMode::Git => server.get(&format!("/api/vcs/diff?{loc}&mode=working&untracked=folders")),
    };

    let changes = raw_diff.map(|v| {
        let parsed = ChangeList::read(v);
        let mut by_server: Vec<_> = parsed.meta.iter().collect();
        by_server.sort_unstable_by_key(|m| m.listed);
        let listed: Vec<String> = by_server.into_iter().map(|m| m.path.clone()).collect();
        let mut tree = build_change_tree(&parsed.meta, &spec.collapsed);
        insert_folder_nodes(&mut tree, parsed.folders.iter().map(String::as_str));
        let active_path = spec
            .active_path
            .filter(|p| tree.get(p).is_some_and(|n| !n.dir))
            .or_else(|| listed.first().cloned());
        if let Some(p) = &active_path {
            tree.select(p);
        }
        let narrow = prepare_narrow_paths(&spec.narrow_open, &parsed.files);
        let diffs = DiffDocState::prepared(
            parsed.files,
            spec.diff_mode,
            spec.expand_mode,
            active_path.as_deref(),
            spec.find_query,
        );
        PreparedReview {
            diffs,
            meta: parsed.meta,
            listed,
            tree,
            folders: parsed.folders,
            count: parsed.count,
            active_path,
            narrow,
        }
    });

    FetchedDiffs {
        session_id: spec.session_id,
        directory: spec.directory,
        server_base: server.base.clone(),
        mode: spec.mode,
        revision: spec.revision,
        changes,
        branch_current: None,
        branch_default: None,
        no_git: false,
    }
}

/// Fetches `/api/vcs` and `/api/vcs/diff` concurrently on native targets (sequentially on wasm32)
/// and combines them into one [`FetchedDiffs`].
pub fn fetch_live_diffs(server: &Server, spec: ReviewFetchSpec) -> FetchedDiffs {
    #[cfg(not(target_arch = "wasm32"))]
    let (vcs, mut diff) = std::thread::scope(|scope| {
        let dir = spec.directory.clone();
        let srv = server.clone();
        let vcs_handle = scope.spawn(move || fetch_live_vcs(&srv, &dir));
        let diff = fetch_live_diff_only(server, spec);
        let vcs = vcs_handle.join().ok().flatten();
        (vcs, diff)
    });
    #[cfg(target_arch = "wasm32")]
    let (vcs, mut diff) = {
        let vcs = fetch_live_vcs(server, &spec.directory);
        let diff = fetch_live_diff_only(server, spec);
        (vcs, diff)
    };

    if let Some(vcs) = vcs {
        // The location may acquire its VCS provider between these two reads. A working-tree
        // listing with real changes must not be obscured by an earlier no-provider result.
        diff.no_git = vcs.lacks_provider && diff.changes.as_ref().is_ok_and(|p| p.count == 0);
        diff.branch_current = vcs.branch_current;
        diff.branch_default = vcs.branch_default;
    }
    diff
}

pub fn encode_fs_path(path: &str) -> String {
    path.split('/')
        .filter(|seg| !seg.is_empty())
        .map(encode_uri_component)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn fetch_live_file_text(server: &Server, directory: &str, path: &str) -> Option<String> {
    let encoded = encode_fs_path(path);
    let url = format!(
        "/api/fs/read/{encoded}?location%5Bdirectory%5D={}",
        encode_uri_component(directory)
    );
    let bytes = server.get_bytes(&url).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// A fixture's changes for mode: the parsed files, their facts, the paths as listed, the
/// branches, and the fixture's file texts.
#[allow(clippy::type_complexity)]
pub fn load_fixture_diffs(
    fixture: &Value,
    session_id: &str,
    mode: ChangeMode,
) -> (Vec<FileDiff>, Vec<ReviewFileMeta>, Vec<String>, Option<String>, Option<String>, HashMap<String, String>) {
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
    let listed = diff_val.map(listed_paths).unwrap_or_default();

    let mut texts = HashMap::new();
    if let Some(obj) = fixture["file_contents"].as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                texts.insert(normalize_path(k), s.to_owned());
            }
        }
    }
    (files, meta, listed, branch_cur, branch_def, texts)
}
