//! Opening and tracking projects (`⌘O`, `/api/location`, `/api/project`).
//!
//! Against the local OpenCode server, Desktop opens the native macOS folder sheet (`Tx::open_folder`),
//! resolves the chosen directory through `/api/location` so the server registers its project,
//! refreshes `/api/project`, and starts a new session in that folder.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use opencode_kit::client::{DEFAULT_SERVER, Request, Write};
use opencode_kit::model::{LocationInfo, Project, ProjectIcon, SessionInfo, Transcript, project_sandboxes};
use rune::{FileDialogOptions, Tx};

use crate::{App, Source, tabs};

#[derive(Default)]
pub struct ProjectsState {
    /// Projects this client has opened or used, most recent first.
    pub used: Vec<String>,
    /// Counter for sessions opened in fixture mode.
    opened: u32,
}

pub(crate) fn clean_dir(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed == "/" {
        return "/".into();
    }
    trimmed.trim_end_matches('/').to_owned()
}

pub(crate) fn folder_name(dir: &str) -> String {
    dir.rsplit('/').find(|s| !s.is_empty()).unwrap_or(dir).to_owned()
}

fn synthetic_project_id(dir: &str) -> String {
    let mut h = DefaultHasher::new();
    dir.hash(&mut h);
    format!("local_{:016x}", h.finish())
}

fn same_or_within(root: &str, dir: &str) -> bool {
    !root.is_empty() && (dir == root || dir.starts_with(&format!("{root}/")))
}

impl App {
    /// Seeds `project_state.used` from saved state, or from open tabs and loaded sessions.
    pub(crate) fn init_used_projects(&mut self, saved: Option<Vec<String>>) {
        let mut seen = HashSet::new();
        let mut used = Vec::new();
        fn push(seen: &mut HashSet<String>, used: &mut Vec<String>, id: &str) {
            if !id.is_empty() && seen.insert(id.to_owned()) {
                used.push(id.to_owned());
            }
        }
        if let Some(ids) = saved {
            for id in ids {
                push(&mut seen, &mut used, &id);
            }
        } else {
            let tab_projects: Vec<String> = self
                .tabs
                .iter()
                .filter_map(|t| {
                    self.session(&t.id)
                        .and_then(|s| self.project_for_session(s))
                        .or_else(|| self.project_at(&t.directory))
                        .map(|p| p.id.clone())
                })
                .collect();
            for id in tab_projects {
                push(&mut seen, &mut used, &id);
            }
            let session_projects: Vec<String> =
                self.sessions.iter().filter_map(|s| self.project_for_session(s).map(|p| p.id.clone())).collect();
            for id in session_projects {
                push(&mut seen, &mut used, &id);
            }
            if used.is_empty() {
                let all: Vec<String> = self.projects.iter().map(|p| p.id.clone()).collect();
                for id in all {
                    push(&mut seen, &mut used, &id);
                }
            }
        }
        self.project_state.used = used;
    }

    /// Marks project `id` as used most recently by this client and saves the list in live mode.
    pub(crate) fn touch_project(&mut self, id: &str) {
        if id.is_empty() {
            return;
        }
        let now = self.now_ms;
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
            p.active = p.active.max(now);
        }
        self.project_state.used.retain(|x| x != id);
        self.project_state.used.insert(0, id.to_owned());
        self.save_used_projects();
    }

    pub(crate) fn save_used_projects(&self) {
        if let Source::Live { server, .. } = &self.source
            && self.persists()
        {
            tabs::save_projects(&server.base, &self.project_state.used);
        }
    }

    /// Resolves the project that owns `dir`: exact root or worktree sandbox first, then longest
    /// containing project root.
    pub fn project_at(&self, dir: &str) -> Option<&Project> {
        let dir = clean_dir(dir);
        if dir.is_empty() {
            return None;
        }
        self.projects
            .iter()
            .find(|p| p.directory == dir || project_sandboxes(&p.id).iter().any(|s| clean_dir(s) == dir))
            .or_else(|| self.projects.iter().filter(|p| same_or_within(&p.directory, &dir)).max_by_key(|p| p.directory.len()))
    }

    /// Resolves a session's project (`projectForSession` in Desktop): directory match first so
    /// `"global"` non-git folders keep their own identity, then `session.project`.
    pub fn project_for_session(&self, s: &SessionInfo) -> Option<&Project> {
        self.project_at(&s.directory).or_else(|| (!s.project.is_empty() && s.project != "global").then(|| self.project(&s.project)).flatten())
    }

    /// Opens the native folder chooser (`⌘O`, "Open project") and opens the chosen folder.
    pub fn open_project_dialog(&mut self, tx: &mut Tx) {
        let restore_composer = self.composer.focused
            || (self.switcher.open && self.switcher.restore_composer())
            || self.pickers.open.is_some();
        tx.instant();
        self.close_picker(false);
        self.close_switcher(false);
        self.settings.open = false;
        self.menus.on = None;
        self.activity_open = false;
        let options = FileDialogOptions { title: "Open project".into(), ..Default::default() };
        tx.open_folder(options, move |s: &mut App, result, tx| match result {
            Ok(Some(path)) => s.open_project_dir(&path.to_string_lossy(), true, tx),
            Ok(None) => {
                if restore_composer && s.stage.is_none() {
                    s.composer.focused = true;
                }
            }
            Err(e) => s.status = e.to_string(),
        });
    }

    /// Adds `raw_dir` as a project and, when `start_session` is true, opens a new session in it.
    pub fn open_project_dir(&mut self, raw_dir: &str, start_session: bool, tx: &mut Tx) {
        let dir = clean_dir(raw_dir);
        if dir.is_empty() {
            return;
        }
        self.draft_dir = dir.clone();
        if self.live() && self.stage.is_none() {
            if self.write(Write::OpenProject { directory: dir, start_session }) {
                self.status = "Opening project…".into();
            }
            return;
        }
        tx.instant();
        let id = self.ensure_local_project(&dir, None);
        self.touch_project(&id);
        self.sort_projects();
        if start_session {
            let ses = self.make_fixture_session(&id, &dir);
            let sid = ses.id.clone();
            self.transcripts.entry(sid.clone()).or_insert_with(|| Transcript { session: sid.clone(), messages: vec![] });
            self.sessions.insert(0, ses);
            self.sort_projects();
            self.open_now(&sid, tx);
        }
    }

    /// Applies `Incoming::ProjectOpened` from the server write worker.
    pub(crate) fn apply_project_opened(
        &mut self,
        location: LocationInfo,
        projects: Vec<Project>,
        session: Option<SessionInfo>,
        tx: &mut Tx,
    ) {
        tx.instant();
        self.projects = projects;
        let dir = clean_dir(if location.directory.is_empty() { &location.project_directory } else { &location.directory });
        let root = clean_dir(if location.canonical.is_empty() { &dir } else { &location.canonical });
        let project_id = if !location.project_id.is_empty()
            && location.project_id != "global"
            && self.projects.iter().any(|p| p.id == location.project_id)
        {
            location.project_id.clone()
        } else {
            self.ensure_local_project(if root == "/" { &dir } else { &root }, Some(&location.project_id))
        };
        if !dir.is_empty() {
            self.draft_dir = dir.clone();
        }
        self.touch_project(&project_id);
        let server_label = match &self.source {
            Source::Live { server, .. } => server.base.trim_start_matches("http://").to_owned(),
            Source::Fixture => DEFAULT_SERVER.trim_start_matches("http://").to_owned(),
        };
        self.status = format!("Live · {server_label}");
        if let Some(mut info) = session {
            if info.project.is_empty() || info.project == "global" {
                info.project.clone_from(&project_id);
            }
            if info.directory.is_empty() {
                info.directory = dir;
            }
            let sid = info.id.clone();
            self.transcripts.entry(sid.clone()).or_insert_with(|| Transcript { session: sid.clone(), messages: vec![] });
            match self.sessions.iter_mut().find(|s| s.id == sid) {
                Some(old) => *old = info,
                None => self.sessions.insert(0, info),
            }
            self.sort_projects();
            self.open_now(&sid, tx);
        } else {
            self.sort_projects();
        }
    }

    /// Refreshes `/api/project` on a live server.
    #[allow(dead_code)]
    pub(crate) fn refresh_projects(&self) {
        if let Source::Live { requests: Some(r), .. } = &self.source {
            let _ = r.send(Request::Projects);
        }
    }

    fn ensure_local_project(&mut self, dir: &str, preferred_id: Option<&str>) -> String {
        if let Some(existing) = self.project_at(dir) {
            return existing.id.clone();
        }
        let id = preferred_id
            .filter(|id| !id.is_empty() && *id != "global")
            .map(str::to_owned)
            .unwrap_or_else(|| synthetic_project_id(dir));
        let name = folder_name(dir);
        self.projects.push(Project {
            id: id.clone(),
            name,
            directory: dir.to_owned(),
            active: self.now_ms,
            icon: ProjectIcon::default(),
        });
        id
    }

    fn make_fixture_session(&mut self, project_id: &str, directory: &str) -> SessionInfo {
        self.project_state.opened += 1;
        let selection = self.selection();
        SessionInfo {
            id: format!("ses_fxopen{:04}aaaaaaaaaaaaaa", self.project_state.opened),
            parent: None,
            project: project_id.to_owned(),
            title: String::new(),
            directory: directory.to_owned(),
            updated: self.now_ms,
            model: selection.model,
            cost: 0.0,
            idle: None,
            viewed: Some(self.now_ms),
            agent: selection.agent,
            created: self.now_ms,
            outcome: None,
            revert: None,
        }
    }
}

#[cfg(test)]
#[path = "projects_tests.rs"]
mod tests;
