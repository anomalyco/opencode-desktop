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
    Some(Project {
        id,
        name,
        directory: dir,
        active: int_of(&p["time"]["active"]).max(int_of(&p["time"]["updated"])),
        icon: ProjectIcon::parse(&p["icon"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_location_and_project_metadata() {
        let loc = parse_location(&json!({
            "directory": "/Users/demo/code/worktrees/seer/open-project",
            "project": {
                "id": "proj_seer",
                "directory": "/Users/demo/code/worktrees/seer/open-project",
                "canonical": "/Users/demo/code/seer"
            }
        }))
        .unwrap();
        assert_eq!(loc.project_id, "proj_seer");
        assert_eq!(loc.canonical, "/Users/demo/code/seer");
        assert_eq!(loc.directory, "/Users/demo/code/worktrees/seer/open-project");

        let proj = parse_project(&json!({
            "id": "proj_seer",
            "canonical": "/Users/demo/code/seer",
            "name": "Seer",
            "time": { "updated": 10, "active": 25 },
            "sandboxes": ["/Users/demo/code/seer", "/Users/demo/code/worktrees/seer/open-project"]
        }))
        .unwrap();
        assert_eq!(proj.name, "Seer");
        assert_eq!(proj.directory, "/Users/demo/code/seer");
        assert_eq!(proj.active, 25);
        assert_eq!(project_sandboxes("proj_seer"), vec!["/Users/demo/code/worktrees/seer/open-project"]);
    }
}
