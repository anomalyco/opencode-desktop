//! Opening and tracking projects (`⌘O`, `/api/location`, `/api/project`).
//!
//! Against the local OpenCode server, Desktop opens the native macOS folder sheet (`Tx::open_folder`),
//! resolves the chosen directory through `/api/location` so the server registers its project,
//! refreshes `/api/project`, and starts a new session in that folder.

use crate::pickers::PickerHost as _;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use opencode_kit::client::{DEFAULT_SERVER, Request, Write};
use opencode_kit::model::{LocationInfo, Project, ProjectIcon, SessionInfo, Transcript};
use opencode_kit::path;
use rune::{FileDialogOptions, Tx};

use crate::{App, Source, tabs};

pub mod home;
pub mod remote_picker;

#[derive(Default)]
pub struct ProjectsState {
    /// Projects this client has opened or used, most recent first.
    pub used: Vec<String>,
    /// Whether Home's compact project filter popover is open.
    pub home_menu_open: bool,
    /// The narrow Home's projects dropdown's width: its popover takes it (`sameWidth`).
    pub trigger_w: f32,
    /// An Import session upload is on its way.
    pub importing: bool,
    /// Counter for sessions opened in fixture mode.
    opened: u32,
    /// The organized sidebar (`sidebar-organized.tsx`): Projects shut, the projects open to
    /// their sessions, all projects shown, and how many sessions each open project shows.
    pub organize_collapsed: bool,
    pub organize_expanded: HashSet<String>,
    pub organize_all: bool,
    pub organize_limits: std::collections::HashMap<String, usize>,
    /// The session last opened from the organized list, which it highlights while current.
    pub organize_selected: Option<String>,
    /// The organized list's project order (`sidebar.organize`'s `order`): frozen once the list
    /// shows, then newly used projects join at its end and drags move them.
    pub organize_order: Vec<String>,
    /// A project row being dragged to a new place, and each project block's height for it.
    pub organize_drag: Option<crate::view::sidebar_organized::ProjectDrag>,
    pub organize_heights: std::collections::HashMap<String, f32>,
    /// A drag just ended on a project row, whose release then doesn't toggle it.
    pub organize_dropped: bool,
}

pub(crate) fn clean_dir(raw: &str) -> String {
    path::trim(raw.trim()).to_owned()
}

pub(crate) fn folder_name(dir: &str) -> String {
    Some(path::basename(dir)).filter(|n| !n.is_empty()).unwrap_or(dir).to_owned()
}

fn synthetic_project_id(dir: &str) -> String {
    let mut h = DefaultHasher::new();
    dir.hash(&mut h);
    format!("local_{:016x}", h.finish())
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

    /// Projects this client has used, in the server's order (most recently active first), one per
    /// directory: using any project of a folder lists that folder's current one
    /// (`projects.used()` in `runtime/server/runtime.tsx`).
    pub fn used_projects(&self) -> Vec<&Project> {
        let used_dirs: HashSet<String> = self
            .project_state
            .used
            .iter()
            .filter_map(|id| self.projects.iter().find(|p| &p.id == id))
            .map(|p| clean_dir(&p.directory))
            .collect();
        let mut by_activity: Vec<&Project> = self.projects.iter().collect();
        by_activity.sort_by_key(|p| std::cmp::Reverse(p.active));
        let mut seen_dirs = HashSet::new();
        by_activity
            .into_iter()
            .filter(|p| {
                let dir = clean_dir(&p.directory);
                used_dirs.contains(&dir) && seen_dirs.insert(dir)
            })
            .collect()
    }

    /// The organized sidebar's projects (`sidebar-organized.tsx`'s `projects`): those in its order
    /// first, then the other used projects, most recently active first.
    pub fn organized_projects(&self) -> Vec<&Project> {
        let used = self.used_projects();
        let order = &self.project_state.organize_order;
        let mut list: Vec<&Project> = order.iter().filter_map(|id| used.iter().find(|p| &p.id == id).copied()).collect();
        list.extend(used.iter().filter(|p| !order.contains(&p.id)).copied());
        list
    }

    /// Keeps the organized list's order once it shows, so a project's activity no longer moves
    /// it and a newly used one joins at the end (`sidebar-organized.tsx`: "Freeze the initial
    /// order"). It waits for the server's projects.
    pub(crate) fn freeze_organize_order(&mut self) {
        let shown = self.tab_layout() == crate::layout_state::TabLayout::Vertical
            && self.settings.sidebar_organize == crate::settings::Organize::Projects
            && !self.project_state.organize_collapsed;
        if !shown || self.projects.is_empty() {
            return;
        }
        let ids: Vec<String> = self.organized_projects().iter().map(|p| p.id.clone()).collect();
        if ids != self.project_state.organize_order {
            self.project_state.organize_order = ids;
        }
    }

    /// Server projects this client hasn't added or has hidden from its used list.
    pub fn other_projects(&self) -> Vec<&Project> {
        let used_dirs: HashSet<String> = self.used_projects().into_iter().map(|p| clean_dir(&p.directory)).collect();
        let mut seen = HashSet::new();
        self.projects
            .iter()
            .filter(|p| {
                let d = clean_dir(&p.directory);
                !used_dirs.contains(&d) && seen.insert(d)
            })
            .collect()
    }

    /// Removes project `id` from this client's used list (`HomeProjects` Close); the server keeps it.
    pub fn hide_project(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let dir = self.project(id).map(|p| clean_dir(&p.directory));
        let drop_ids: HashSet<String> = self
            .projects
            .iter()
            .filter(|p| p.id == id || dir.as_deref().is_some_and(|d| clean_dir(&p.directory) == d))
            .map(|p| p.id.clone())
            .collect();
        self.project_state.used.retain(|x| !drop_ids.contains(x) && x != id);
        self.project_state.home_menu_open = false;
        self.save_used_projects();
    }

    /// A Home project row: selects project `id` of the server the window works against, or, when
    /// it's selected, goes back to all of the server's projects (`project.toggled`).
    pub fn toggle_home_project(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let server = crate::servers::runtime::active(self).clone();
        if let Some(dir) = self.project(id).map(|p| p.directory.clone()) {
            home::toggle(self, &server, &dir);
        }
        self.project_state.home_menu_open = false;
    }

    /// All projects: Home focuses the server it shows (`server.focused`).
    pub fn clear_home_project(&mut self, tx: &mut Tx) {
        tx.instant();
        let server = home::server(self);
        home::focus(self, &server);
        self.project_state.home_menu_open = false;
    }

    /// Resolves the project that owns `dir`: exact root or worktree sandbox first, then longest
    /// containing project root.
    pub fn project_at(&self, dir: &str) -> Option<&Project> {
        self.project_exactly_at(dir, "").or_else(|| self.project_containing(dir))
    }

    /// Resolves a session's project (`projectForSession` in Desktop): a project rooted exactly at
    /// its directory first (so `"global"` folders and moved repos whose sessions keep an old
    /// project ID resolve as the server does now), then `session.project`, and only then a
    /// project whose root merely contains the directory, so a parent folder like `~/code` that
    /// was once opened never claims every repo under it.
    pub fn project_for_session(&self, s: &SessionInfo) -> Option<&Project> {
        let by_id = || (!s.project.is_empty() && s.project != "global").then(|| self.project(&s.project)).flatten();
        self.project_exactly_at(&s.directory, &s.project).or_else(by_id).or_else(|| self.project_containing(&s.directory))
    }

    /// The project rooted at `dir` or with it as a sandbox, preferring `id` when several are.
    fn project_exactly_at(&self, dir: &str, id: &str) -> Option<&Project> {
        let dir = path::trim(dir.trim());
        if dir.is_empty() {
            return None;
        }
        // Its checkout, or one of its worktrees: a sandbox, or one the server's inventory lists.
        let at = |p: &&Project| p.directory == dir || self.has_worktree_at(p, dir);
        self.project(id).filter(at).or_else(|| self.projects.iter().find(at))
    }

    fn project_containing(&self, dir: &str) -> Option<&Project> {
        let dir = clean_dir(dir);
        self.projects.iter().filter(|p| path::contains(&p.directory, &dir)).max_by_key(|p| p.directory.len())
    }

    /// Opens the native folder chooser (`⌘O`, "Open project") and opens the chosen folder.
    pub fn open_project_dialog(&mut self, tx: &mut Tx) {
        let restore_composer = self.composer.focused
            || (self.switcher.open && self.switcher.restore_composer())
            || self.pickers.open.is_some();
        tx.instant();
        self.close_picker(false);
        self.close_switcher(false);
        self.close_project_switcher(false);
        self.project_state.home_menu_open = false;
        self.settings.open = false;
        self.menus.on = None;
        crate::subagents::shells::set_open(self, false);
        self.todo_open = false;
        let options = FileDialogOptions { title: opencode_i18n::t!("command.project.open").into(), ..Default::default() };
        tx.open_folder(options, move |s: &mut App, result, tx| match result {
            Ok(Some(path)) => s.open_project_dir(&path.to_string_lossy(), true, tx),
            Ok(None) => {
                if restore_composer && s.stage.is_none() {
                    s.composer.focused = true;
                }
            }
            Err(e) => crate::status_toast::fail(s, opencode_i18n::t!("rune.status.folderPickerFailed"), e.to_string()),
        });
    }

    /// Adds `raw_dir` as a project and, when `start_session` is true, opens a new session in it.
    pub fn open_project_dir(&mut self, raw_dir: &str, start_session: bool, tx: &mut Tx) {
        let dir = clean_dir(raw_dir);
        if dir.is_empty() {
            return;
        }
        self.set_draft_dir(dir.clone());
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
            self.set_draft_dir(dir.clone());
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
            ..Default::default()
        }
    }
}

#[cfg(test)]
#[path = "projects_tests.rs"]
mod tests;
