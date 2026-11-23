//! The session list a launch draws before its server answers: the projects and the newest
//! sessions the app's own server last listed, kept in the app's saved state (the boot record, so
//! the first frame has them with no file read of its own) and put back before the first frame.
//! Home, the sidebar's lists and the tabs' avatars then show what they showed, not placeholder
//! rows or a spinner, for as long as the server takes; its first answer replaces them, and a
//! session it no longer lists goes then.
//!
//! It stands in for a server that is there: one that took the launch's connection
//! (`boot::reached`: the head start's, asked from the top of `main`, has usually been taken by
//! the time the state is read) and hasn't answered. With no server running the window is what
//! it was without the list: Home's setup card, not a list that shows and then gives way to it.
//!
//! OpenCode Desktop keeps no such copy: its window shows a splash until its server is ready
//! (`packages/desktop/src/renderer/startup/machine.ts`), and Home's list shows placeholder rows
//! while it loads (`packages/app/src/home/sessions/load-machine.ts`).
//!
//! What it is not: live. Nothing in it says a session is running (that comes from the server's
//! `session/active`, which a launch always reads), so a session that was streaming when the app
//! quit shows as a plain row until the server says otherwise.
//!
//! Written when the first reads land and as a window hides, closes or quits: never per event.

use std::collections::HashSet;

use opencode_kit::model::{Project, SessionInfo};
use serde_json::{Value, json};

use crate::App;

/// Desktop's global storage name: what every window shares, all of it in the boot record.
const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "rune.launch.list";
const VERSION: u64 = 1;
/// More root sessions than a first screen lists (the sidebar's Recent shows 30).
const SESSIONS: usize = 60;
/// A project's icon is kept when it is this small (a data URL of a favicon): a larger one comes
/// back with the server's answer, over the letter avatar.
const ICON_MAX: usize = 4096;

/// The sessions on screen that came from the saved list: they stand until the server's own list
/// lands (`App::apply_first`), which keeps none of them it doesn't name.
#[derive(Debug)]
pub struct SavedList {
    sessions: HashSet<String>,
}

impl SavedList {
    /// The server's list is in: the saved one's sessions leave `sessions`, for the list to add
    /// back those it still has.
    pub(crate) fn retire(self, sessions: &mut Vec<SessionInfo>) {
        sessions.retain(|s| !self.sessions.contains(&s.id));
    }
}

/// Before the first frame of a window whose server answers after it, or as soon after as the
/// server has taken the launch's connection: the saved list. True when it was put back.
pub(crate) fn restore(s: &mut App) -> bool {
    if !s.persists() || !crate::servers::runtime::active(s).is_sidecar() || !s.sessions.is_empty() || s.launch.saved_list.is_some() || !super::super::boot::reached() {
        return false;
    }
    let Some(saved) = crate::storage::json(GLOBAL, KEY).filter(|v| v["v"].as_u64() == Some(VERSION)) else { return false };
    let projects = opencode_kit::model::parse_projects(&saved["projects"]);
    let sessions = opencode_kit::model::parse_sessions(&saved["sessions"]);
    if sessions.is_empty() {
        return false;
    }
    rune::marks::mark("state.list");
    let ids = sessions.iter().map(|x| x.id.clone()).collect();
    *s.projects = projects;
    s.sessions_listed(sessions);
    let used = match &*s.source {
        crate::Source::Live { server, .. } => crate::tabs::load_projects(&server.base).unwrap_or_default(),
        crate::Source::Fixture => Vec::new(),
    };
    s.init_used_projects(Some(used));
    s.sort_projects();
    s.launch.saved_list = Some(SavedList { sessions: ids });
    true
}

/// Keeps what the app's own server lists now, for the next launch. Only a list the server gave:
/// a window still showing the saved one writes nothing.
pub(crate) fn save(s: &App) {
    let own = crate::servers::runtime::active(s);
    if !s.persists() || !own.is_sidecar() || s.launch.saved_list.is_some() || !matches!(s.launch.phase(own.as_str()), Some(super::super::Phase::Connected)) {
        return;
    }
    let tabs: HashSet<&str> = s.tabs.iter().filter(|t| !t.is_draft()).map(|t| t.id.as_str()).collect();
    let mut roots: Vec<&SessionInfo> = s.sessions.iter().filter(|x| x.parent.is_none() && x.archived.is_none()).collect();
    roots.sort_by_key(|x| (std::cmp::Reverse(s.recent_page.time(x)), &x.id));
    let newest: HashSet<&str> = roots.iter().take(SESSIONS).map(|x| x.id.as_str()).collect();
    // In the list's own order, so a launch sorts what the server sorted.
    let sessions: Vec<Value> = s.sessions.iter().filter(|x| newest.contains(x.id.as_str()) || tabs.contains(x.id.as_str())).map(session_json).collect();
    if sessions.is_empty() {
        return;
    }
    let projects: Vec<Value> = s.projects.iter().map(project_json).collect();
    let value = json!({ "v": VERSION, "projects": projects, "sessions": sessions });
    // An unchanged list writes nothing (`storage::set`).
    crate::storage::set_json(GLOBAL, KEY, &value);
}

/// A session as the server lists it (`opencode_kit::model::parse_session` reads it back).
fn session_json(x: &SessionInfo) -> Value {
    let mut time = json!({ "created": x.created, "updated": x.updated });
    for (key, at) in [("idle", x.idle), ("viewed", x.viewed), ("busy", x.busy), ("archived", x.archived)] {
        if let Some(at) = at {
            time[key] = at.into();
        }
    }
    let mut v = json!({
        "id": x.id,
        "projectID": x.project,
        "title": x.title,
        "location": { "directory": x.directory },
        "time": time,
        "model": { "id": x.model.id, "providerID": x.model.provider, "variant": x.model.variant },
        "cost": x.cost,
        "agent": x.agent,
    });
    if let Some(parent) = &x.parent {
        v["parentID"] = parent.as_str().into();
    }
    if let Some(outcome) = &x.outcome {
        v["outcome"] = outcome.as_str().into();
    }
    if let Some(message) = &x.revert {
        v["revert"] = json!({ "messageID": message });
    }
    if !x.metadata.is_null() {
        v["metadata"] = x.metadata.clone();
    }
    v
}

/// A project as the server lists it (`opencode_kit::project::parse_project` reads it back, with
/// what it keeps beside the list: its version control, sandboxes, name and start command).
fn project_json(p: &Project) -> Value {
    let meta = opencode_kit::model::project_meta(&p.id);
    let small = |url: &str| if url.len() <= ICON_MAX { url } else { "" }.to_owned();
    let mut v = json!({
        "id": p.id,
        "worktree": p.directory,
        "time": { "active": p.active },
        "icon": { "url": small(&p.icon.url), "override": small(&p.icon.override_url), "color": p.icon.color },
        "sandboxes": opencode_kit::model::project_sandboxes(&p.id),
        "commands": { "start": meta.start },
    });
    if !meta.name.is_empty() {
        v["name"] = meta.name.into();
    }
    // `git`, or any other the server named: the list says only whether there is one.
    if opencode_kit::model::project_git(&p.id) {
        v["vcs"] = "git".into();
    } else if opencode_kit::project::directory_vcs(&p.directory) == Some(true) {
        v["vcs"] = "other".into();
    }
    v
}
