//! The location dialog across servers (`new-session/project/controller.ts`, `selector.tsx`,
//! `location/dialog.tsx`): once more than one server is listed, its projects are every ready
//! server's, used ones first then by recent activity, each tagged with its server unless it's the
//! app's own; a query also matches a server's name; and Add project first asks which server
//! (the Servers step). Picking another server's project moves the new session there.

use std::collections::HashMap;

use opencode_kit::model::{Project, project_git};
use opencode_kit::servers::{ServerConn, ServerKey};
use rune::{El, Tx, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::{RowKind, SwitcherRow, matches_query, StepMode};
use crate::palette::Pal;
use crate::projects::folder_name;
use crate::{App, Route};
use opencode_i18n::t;

/// How a server's row and tag mark it (`PromptServer.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The app's own: no tag, the monitor.
    Current,
    /// Added by address: the globe.
    Remote,
    /// A plugin's (WSL, SSH): the terminal.
    Plugin,
}

/// A ready server, as the dialog lists it.
#[derive(Clone, Debug)]
pub struct Listed {
    pub key: ServerKey,
    pub name: String,
    /// "This computer" beside the app's own.
    pub hint: Option<String>,
    pub kind: Kind,
    pub used: Vec<Project>,
    pub others: Vec<Project>,
    /// Each drilled project's worktrees.
    pub worktrees: HashMap<String, Vec<String>>,
    pub checkout_branch: Option<(String, String)>,
}

impl Listed {
    pub fn project(&self, id: &str) -> Option<&Project> {
        self.used.iter().chain(&self.others).find(|p| p.id == id)
    }
}

/// The ready servers that aren't down, read through each one's half, when more than one is
/// listed (`controller.ts:48-64`); none with one server.
pub fn read(s: &mut App) -> Vec<Listed> {
    let all = crate::servers::registry::list(s);
    if all.len() <= 1 || !s.live() {
        return Vec::new();
    }
    let drilled = match &s.project_switcher.mode {
        StepMode::Worktrees { project, .. } | StepMode::Branches { project, .. } => Some(project.clone()),
        _ => None,
    };
    let mut out = Vec::new();
    for e in all.into_iter().filter(|e| e.conn.http().is_some()) {
        let key = e.key();
        if opencode_kit::servers::health::is_failed(crate::servers::health::get(&key).as_ref()) {
            continue;
        }
        let kind = match &e.conn {
            ServerConn::Sidecar { .. } => Kind::Current,
            ServerConn::Plugin { .. } => Kind::Plugin,
            ServerConn::Http { .. } => Kind::Remote,
        };
        let drilled = drilled.clone();
        let read = move |s: &mut App| {
            let used: Vec<Project> = s.used_projects().into_iter().cloned().collect();
            let others: Vec<Project> = s.other_projects().into_iter().cloned().collect();
            let worktrees = drilled.iter().filter_map(|id| s.project(id).map(|p| (id.clone(), s.project_worktrees(id, &p.directory)))).collect();
            let branch = drilled.as_ref().and_then(|id| s.project(id)).and_then(|p| s.sidebar_inbox.branches.get(&p.directory).map(|b| (p.directory.clone(), b.to_owned())));
            (used, others, worktrees, branch)
        };
        let Some((used, others, worktrees, checkout_branch)) = crate::servers::runtime::with_server(s, &key, read) else { continue };
        out.push(Listed { name: crate::servers::registry::label(&e.conn), hint: crate::servers::registry::hint(&e.conn), key, kind, used, others, worktrees, checkout_branch });
    }
    if out.len() <= 1 { Vec::new() } else { out }
}

/// Each poll while the dialog is open: what each server lists now.
pub fn poll(s: &mut App) -> bool {
    if !s.project_switcher.open || s.project_switcher.servers.is_empty() {
        return false;
    }
    let next = read(s);
    let changed = next.len() != s.project_switcher.servers.len()
        || next.iter().zip(&s.project_switcher.servers).any(|(a, b)| a.key != b.key || a.used.len() != b.used.len() || a.others.len() != b.others.len() || a.worktrees != b.worktrees || a.checkout_branch != b.checkout_branch);
    s.project_switcher.servers = next;
    changed
}

/// The server step's rows (`serverEntries`).
pub fn server_rows(s: &App, q: &str) -> Vec<SwitcherRow> {
    s.project_switcher
        .servers
        .iter()
        .filter(|l| matches_query(q, &[&l.name]))
        .map(|l| SwitcherRow { key: format!("server:{}", l.key), title: l.name.clone(), subtitle: l.hint.clone().unwrap_or_default(), section: "Servers", kind: RowKind::Server { key: l.key.clone() } })
        .collect()
}

/// The projects step across servers, or the other projects (`projectEntries`).
pub fn project_rows(s: &App, q: &str, others: bool) -> Vec<SwitcherRow> {
    let mut all: Vec<(&Listed, &Project, bool)> = Vec::new();
    for l in &s.project_switcher.servers {
        all.extend(l.used.iter().map(|p| (l, p, true)));
        all.extend(l.others.iter().map(|p| (l, p, false)));
    }
    // Used first, then by recent activity (`selector.tsx:60-66`).
    all.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.active.cmp(&a.1.active)));
    let other_count = all.iter().filter(|(_, _, used)| !used).count();
    let mut out: Vec<SwitcherRow> = all
        .into_iter()
        .filter(|(_, _, used)| if others { !used } else { *used || !q.is_empty() })
        .filter(|(l, p, _)| matches_query(q, &[&p.name, &p.directory, if l.kind == Kind::Current { "" } else { &l.name }]))
        .map(|(l, p, used)| SwitcherRow {
            key: format!("project:{}:{}", l.key, p.directory),
            title: p.name.clone(),
            subtitle: p.directory.clone(),
            section: if used { "Projects" } else { "Other projects" },
            kind: RowKind::Project { id: p.id.clone(), directory: p.directory.clone(), used, drillable: project_git(&p.id) || l.worktrees.get(&p.id).is_some_and(|w| !w.is_empty()), server: Some(l.key.clone()) },
        })
        .collect();
    if others {
        return out;
    }
    if q.is_empty() && other_count > 0 {
        out.push(SwitcherRow { key: "other".into(), title: t!("session.new.location.otherProjects").into(), subtitle: other_count.to_string(), section: "Projects", kind: RowKind::Other { count: other_count } });
    }
    out.push(SwitcherRow { key: "+add".into(), title: t!("session.new.project.add").into(), subtitle: String::new(), section: "Actions", kind: RowKind::Add });
    out
}

/// A drilled project's checkout, New worktree and worktrees, on its server.
pub fn worktree_rows(s: &App, server: &ServerKey, pid: &str) -> Vec<SwitcherRow> {
    let Some(l) = s.project_switcher.servers.iter().find(|l| l.key == *server) else { return Vec::new() };
    let Some(project) = l.project(pid) else { return Vec::new() };
    let (id, root) = (project.id.clone(), project.directory.clone());
    let at = Some(server.clone());
    let main = SwitcherRow { key: format!("wt:main:{id}"), title: project.name.clone(), subtitle: root.clone(), section: "Worktrees", kind: RowKind::Worktree { project_id: id.clone(), directory: root.clone(), main: true, server: at.clone() } };
    let create = SwitcherRow { key: format!("wt:create:{id}"), title: t!("workspace.new").into(), subtitle: t!("session.new.workspace.new.tooltip").into(), section: "Worktrees", kind: RowKind::Create { project_id: id.clone(), directory: root.clone(), server: at.clone() } };
    let worktrees = l.worktrees.get(&id).into_iter().flatten().map(|dir| SwitcherRow { key: format!("wt:{dir}"), title: folder_name(dir), subtitle: dir.clone(), section: "Worktrees", kind: RowKind::Worktree { project_id: id.clone(), directory: dir.clone(), main: false, server: at.clone() } });
    [main, create].into_iter().chain(worktrees).collect()
}

/// A project of `server`'s: the dialog's project for the badge and the worktree check.
pub fn project<'a>(s: &'a App, server: &ServerKey, id: &str) -> Option<&'a Project> {
    s.project_switcher.servers.iter().find(|l| l.key == *server).and_then(|l| l.project(id))
}

/// `ServerTag`: a subtle badge with the server's icon and name, for any server but the app's own
/// (`dialog.tsx:540-552`, `.new-session-location-tag`).
pub fn tag(s: &App, p: &Pal, server: &ServerKey) -> Option<El> {
    let l = s.project_switcher.servers.iter().find(|l| l.key == *server).filter(|l| l.kind != Kind::Current)?;
    Some(
        row()
            .key("tag")
            .h(16.0)
            .max_w(160.0)
            .px(4.0)
            .gap(3.0)
            .radius(2.0)
            .center_y()
            .fill(p.layer3)
            .child(icon(glyph(l.kind), 12.0).color(p.muted))
            .child(text(l.name.clone()).font(rune::TextStyle::new(11.0, 530.0).lh(16.0).tracking(0.05)).color(p.muted).truncate().shrink().min_w(0.0)),
    )
}

/// `serverIcon`.
pub fn glyph(kind: Kind) -> &'static rune::Icon {
    match kind {
        Kind::Current => &ic::MONITOR,
        Kind::Plugin => &ic::TERMINAL,
        Kind::Remote => &ic::GLOBE,
    }
}

/// The server's kind, for its row's glyph.
pub fn kind(s: &App, server: &ServerKey) -> Kind {
    s.project_switcher.servers.iter().find(|l| l.key == *server).map_or(Kind::Current, |l| l.kind)
}

/// The new session's location moves to `dir` on `server` (`selectProject` → `tabs.updateDraft`):
/// the open draft takes the server and folder in place; anywhere else the folder opens there
/// with a new session.
pub fn select(s: &mut App, server: &ServerKey, dir: String, tx: &mut Tx) {
    if let Route::Draft(id) = &*s.route
        && let Some(tab) = s.tabs.iter_mut().find(|t| t.id == *id)
    {
        tab.server = server.clone();
        crate::servers::runtime::switch(s, server);
        s.set_draft_dir(dir);
        s.save_tabs();
        return;
    }
    crate::servers::runtime::switch(s, server);
    s.open_project_dir(&dir, true, tx);
}

/// Add project on `server` (the Servers step's pick, `project.add`): its folder picker, then the
/// folder becomes a project there and the new session's location.
pub fn add(s: &mut App, server: &ServerKey, tx: &mut Tx) {
    s.close_project_switcher(false);
    let key = server.clone();
    crate::projects::remote_picker::pick(s, server, t!("command.project.open").to_owned(), tx, move |s: &mut App, dir: String, tx: &mut Tx| {
        if matches!(*s.route, Route::Draft(_)) {
            crate::servers::runtime::with_server(s, &key, |s| s.open_project_dir(&dir, false, tx));
            select(s, &key, dir, tx);
            return;
        }
        select(s, &key, dir, tx);
    });
}
