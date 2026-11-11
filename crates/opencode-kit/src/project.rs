//! Project and location parsing (`/api/project`, `/api/location`).
//!
//! `Project` keeps its five fields so existing callers construct it unchanged; a project's extra
//! worktree directories (`sandboxes`) live alongside it by project id, like `user_files`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::model::{Project, ProjectIcon};

/// What `GET /api/location` resolves a folder to: the folder itself, and the project it belongs
/// to (`project.id`, its checkout `project.directory`, and `project.canonical`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocationInfo {
    pub directory: String,
    pub project_id: String,
    pub project_directory: String,
    pub canonical: String,
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_owned()
}

fn int_of(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(0)
}

fn sandbox_store() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Non-canonical worktree directories the server reported on project `id` (`sandboxes`).
pub fn project_sandboxes(id: &str) -> Vec<String> {
    sandbox_store().lock().unwrap().get(id).cloned().unwrap_or_default()
}

/// Records the non-canonical worktree directories for project `id`.
pub fn set_project_sandboxes(id: &str, sandboxes: Vec<String>) {
    if id.is_empty() {
        return;
    }
    let mut store = sandbox_store().lock().unwrap();
    if sandboxes.is_empty() {
        store.remove(id);
    } else {
        store.insert(id.to_owned(), sandboxes);
    }
}

fn git_store() -> &'static Mutex<HashMap<String, bool>> {
    static STORE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Whether the server reported project `id` as a Git checkout (`vcs: "git"`): it can have
/// worktrees, and a new one can be made.
pub fn project_git(id: &str) -> bool {
    git_store().lock().unwrap().get(id).copied().unwrap_or(false)
}

/// The directories of `GET /api/worktree?projectID=`, other than the project's own `root`.
pub fn parse_worktrees(v: &Value, root: &str) -> Vec<String> {
    let items = v.get("data").unwrap_or(v).as_array().cloned().unwrap_or_default();
    let mut dirs: Vec<String> = Vec::new();
    for d in items.iter().filter_map(|i| i["directory"].as_str()) {
        if crate::path::trim(d) != crate::path::trim(root) && !dirs.iter().any(|x| crate::path::trim(x) == crate::path::trim(d)) {
            dirs.push(d.to_owned());
        }
    }
    dirs
}

/// Parses one `Location.PublicInfo` reply from `GET /api/location`.
pub fn parse_location(v: &Value) -> Option<LocationInfo> {
    let v = v.get("data").unwrap_or(v);
    let directory = str_of(&v["directory"]);
    let project = &v["project"];
    let project_directory = str_of(&project["directory"]);
    if directory.is_empty() && project_directory.is_empty() {
        return None;
    }
    let canonical = {
        let c = str_of(&project["canonical"]);
        if c.is_empty() {
            if project_directory.is_empty() { directory.clone() } else { project_directory.clone() }
        } else {
            c
        }
    };
    Some(LocationInfo {
        directory: if directory.is_empty() { project_directory.clone() } else { directory },
        project_id: str_of(&project["id"]),
        project_directory: if project_directory.is_empty() { canonical.clone() } else { project_directory },
        canonical,
    })
}

/// What a project's settings edit (Settings › Projects › a project): its name as given (empty
/// when it goes by its folder's) and its worktree startup script (`commands.start`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectMeta {
    pub name: String,
    pub start: String,
}

fn meta_store() -> &'static Mutex<HashMap<String, ProjectMeta>> {
    static STORE: OnceLock<Mutex<HashMap<String, ProjectMeta>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Project `id`'s name as given and startup script, as the server last reported them.
pub fn project_meta(id: &str) -> ProjectMeta {
    meta_store().lock().unwrap().get(id).cloned().unwrap_or_default()
}

/// Parses one `Project.Info` object from `/api/project` or a `project.updated` event.
pub fn parse_project(p: &Value) -> Option<Project> {
    let dir = p["canonical"].as_str().or_else(|| p["worktree"].as_str()).unwrap_or("").to_owned();
    if dir.is_empty() {
        return None;
    }
    let id = str_of(&p["id"]);
    let fallback = Some(crate::path::basename(&dir)).filter(|n| !n.is_empty()).unwrap_or(&dir);
    let name = p["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(fallback).to_owned();
    if let Some(items) = p["sandboxes"].as_array() {
        let mut sandboxes: Vec<String> = items
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty() && *s != dir)
            .map(str::to_owned)
            .collect();
        sandboxes.dedup();
        set_project_sandboxes(&id, sandboxes);
    }
    if !id.is_empty() {
        git_store().lock().unwrap().insert(id.clone(), p["vcs"].as_str() == Some("git"));
        let meta = ProjectMeta { name: p["name"].as_str().map(str::trim).unwrap_or("").to_owned(), start: p["commands"]["start"].as_str().unwrap_or("").to_owned() };
        meta_store().lock().unwrap().insert(id.clone(), meta);
    }
    Some(Project {
        id,
        name,
        directory: dir,
        // `time.active`, the order the server lists projects in (`desc(time_active), asc(id)`); a
        // server from before it had one has `time.updated`.
        active: Some(int_of(&p["time"]["active"])).filter(|a| *a > 0).unwrap_or_else(|| int_of(&p["time"]["updated"])),
        icon: ProjectIcon::parse(&p["icon"]),
    })
}

