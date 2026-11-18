//! ⇧⌘O ("Search projects"), Desktop's project and worktree switcher (`new-session/location/dialog.tsx`),
//! which the new-session page's project and worktree buttons open too.
//!
//! Lists this client's recent/used projects first, then any other server projects (`Other projects`),
//! a Git project's worktrees behind its chevron (its checkout, New worktree, then the server's
//! inventory), and `+ Add project` (`⌘O`) last. Selecting a project updates the open draft's
//! folder on a New Session page, or opens a new session in that project from anywhere else.

pub mod branches;
mod location;
pub mod servers;
pub mod view;

use opencode_i18n::t;
use opencode_kit::model::{Project, project_git};
use opencode_kit::servers::ServerKey;
use opencode_search::query::{Highlight, HighlightEvent};
use rune::machine::{Attempt, Seq};
use rune::{El, Key, KeyEvent, PointerEvent, TextEdit, Tx};
use session_view::palette::Pal;

pub use branches::{List as BranchList, apply_answer as apply_branch_answer, open_git_location};
pub use servers::{Kind as ServerKind, Listed as ListedServer, poll};
pub use view::view;

use crate::entries::{move_for, target};
use crate::{PROJECT_OVERLAY_ID, folder_name, search_fold};

/// Object-safe host boundary for the project, worktree, server, and base-branch location switcher.
pub trait ProjectSwitcherHost {
    fn project_switcher(&self) -> &ProjectSwitcher;
    fn project_switcher_mut(&mut self) -> &mut ProjectSwitcher;

    // Projects & worktrees on the active server
    fn project(&self, id: &str) -> Option<&Project>;
    fn used_projects(&self) -> Vec<&Project>;
    fn other_projects(&self) -> Vec<&Project>;
    fn project_worktrees(&self, id: &str, root: &str) -> Vec<String>;

    // Route & draft state
    fn is_draft_route(&self) -> bool;
    fn is_stage(&self) -> bool {
        false
    }
    fn open_draft_tab(&mut self);
    fn set_composer_focused(&mut self, focused: bool);
    /// Focuses the composer after picking a project/worktree when appropriate.
    fn auto_focus_composer(&mut self, tx: &mut Tx);
    fn draft_base(&self) -> Option<&str> {
        None
    }
    fn draft_creates(&self) -> bool {
        false
    }
    fn draft_project(&self) -> Option<&Project> {
        None
    }
    fn open_branch_picker(&mut self, _tx: &mut Tx) {}
    fn close_branch_picker(&mut self, _tx: &mut Tx) {}
    fn choose_new_worktree(&mut self, project_id: &str, directory: String);
    fn set_draft_base(&mut self, base: Option<String>);
    fn set_draft_dir(&mut self, directory: String);
    fn note_existing_worktree(&mut self, project_id: &str, directory: String);
    fn touch_project(&mut self, project_id: &str);
    fn sort_projects(&mut self);
    fn open_project_dir(&mut self, directory: &str, open_session: bool, tx: &mut Tx);
    fn open_project_dialog(&mut self, tx: &mut Tx);

    // Branch cache & async branch search
    fn enqueue_branch_dir(&mut self, _directory: String) {}
    fn checkout_branch(&self, _directory: &str) -> Option<&str> {
        None
    }
    /// Starts an off-thread `/api/vcs/branches` search on `server` (`None` = active server) and
    /// delivers the result via [`apply_branch_answer`]. Returns `false` if offline/unavailable.
    fn fetch_branches(&mut self, _server: Option<&ServerKey>, _directory: String, _query: String, _attempt: Attempt) -> bool {
        false
    }

    // Multi-server
    fn active_server(&self) -> ServerKey {
        ServerKey::sidecar()
    }
    /// Reads all ready, healthy servers when more than one is listed (`Vec::new()` for a single server).
    fn read_listed_servers(&mut self, _drilled_project: Option<&str>) -> Vec<servers::Listed> {
        Vec::new()
    }
    /// Requests the worktree inventory (and enqueues checkout branch lookup) for `project_id` on `server`.
    fn list_worktrees_on(&mut self, _server: Option<&ServerKey>, _project_id: &str) {}
    /// Moves the active draft or opens a new session in `directory` on `server`.
    fn select_server_location(&mut self, _server: &ServerKey, _directory: String, _tx: &mut Tx) {}
    /// Opens the remote folder picker on `server` for adding a project there.
    fn pick_remote_project_folder(&mut self, _server: &ServerKey, _title: String, _tx: &mut Tx) {}

    // Lifecycle, overlays, visuals & shortcuts
    /// Prepares the shell before opening the project switcher (closes pickers, switcher, settings,
    /// home menu, blurs composer/search) and returns whether composer focus should be restored.
    fn before_project_switcher_open(&mut self) -> bool {
        false
    }
    /// Restores composer focus after closing the project switcher when `restore_composer` is true.
    fn after_project_switcher_close(&mut self, _restore_composer: bool) {}
    /// Registers an opened dialog overlay (`PROJECT_OVERLAY_ID`) with `OverlayStack`.
    fn on_overlay_opened(&mut self, id: &'static str, return_focus: Option<String>);
    /// Removes a closed dialog overlay from `OverlayStack`.
    fn on_overlay_closed(&mut self, id: &'static str);
    fn project_badge(&self, p: &Pal, project: Option<&Project>, name: &str) -> El;
    fn zoom(&self) -> f32 {
        1.0
    }
    fn spellcheck(&self) -> bool {
        false
    }
    fn handle_shortcut(&mut self, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
        false
    }
}

/// The dialog's step (`LocationDialogState.type`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StepMode {
    #[default]
    Projects,
    Others,
    /// Add project with several servers: which one (`servers`).
    Servers,
    /// A project's worktrees, on `server` (None: the window's); `direct` when the dialog opened
    /// on them.
    Worktrees { project: String, server: Option<ServerKey>, direct: bool, base: Option<String> },
    Branches { project: String, server: Option<ServerKey>, direct: bool, directory: String, list: branches::List },
}

#[derive(Default)]
pub struct ProjectSwitcher {
    pub open: bool,
    pub search: TextEdit,
    pub mode: StepMode,
    /// The steps to step back through, innermost last.
    history: Vec<location::Frame>,
    pub(crate) branch_requests: Seq,
    /// The lit row: the first until one's chosen, which holds while the step lists it
    /// (`dialog.tsx:212-217`); typing keeps it.
    active: Highlight,
    /// The pointer's last point on a row: only a pointer that moved takes one (`dialog.tsx:395-399`).
    pointer: Option<[f32; 2]>,
    restore_composer: bool,
    /// Every ready server's projects, when more than one is listed (`servers.rs`).
    pub(crate) servers: Vec<servers::Listed>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// `server`: another server's (None: the window's own).
    Project { id: String, directory: String, used: bool, drillable: bool, server: Option<ServerKey> },
    Worktree { project_id: String, directory: String, main: bool, server: Option<ServerKey> },
    /// New worktree: the session starts in a worktree made from the project's checkout.
    Create { project_id: String, directory: String, server: Option<ServerKey> },
    Base { directory: String },
    Branch,
    Other { count: usize },
    Add,
    /// A server Add project adds to (the Servers step).
    Server { key: ServerKey },
}

#[derive(Clone, Debug)]
pub struct SwitcherRow {
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub section: &'static str,
    pub kind: RowKind,
}

pub(super) fn matches_query(q: &str, fields: &[&str]) -> bool {
    let fold = search_fold();
    q.trim().is_empty() || fields.iter().any(|f| fold.contains(f, q))
}

impl ProjectSwitcher {
    pub fn activate(&mut self, key: String) {
        self.active.set(key);
    }

    /// The pointer moved on row `key` to `at`: it takes the row only when it moved.
    pub(crate) fn point(&mut self, key: &str, at: [f32; 2]) -> bool {
        if self.pointer.replace(at) == Some(at) || self.active.pointed(key) {
            return false;
        }
        self.active.step(HighlightEvent::Pointer(key.to_owned()));
        true
    }

    pub fn query(&self) -> &str {
        self.search.text.trim()
    }

    pub fn restore_composer(&self) -> bool {
        self.restore_composer
    }
}

pub fn rows(host: &dyn ProjectSwitcherHost) -> Vec<SwitcherRow> {
    let ps = host.project_switcher();
    let q = ps.query();
    if let StepMode::Branches { list, .. } = &ps.mode {
        return branches::rows(list, q);
    }
    if !ps.servers.is_empty() {
        return match &ps.mode {
            StepMode::Worktrees { project, server, .. } => {
                let server = server.clone().unwrap_or_else(|| host.active_server());
                branches::worktree_rows(host, servers::worktree_rows(host, &server, project), q)
            }
            StepMode::Servers => servers::server_rows(host, q),
            mode => servers::project_rows(host, q, *mode == StepMode::Others),
        };
    }
    match &ps.mode {
        StepMode::Branches { list, .. } => branches::rows(list, q),
        StepMode::Servers => Vec::new(),
        StepMode::Worktrees { project: pid, .. } => {
            let Some(project) = host.project(pid) else { return Vec::new() };
            let (id, root) = (project.id.clone(), project.directory.clone());
            // The checkout and New worktree, then the worktrees (`dialog.tsx:156-173`).
            let main = SwitcherRow {
                key: format!("wt:main:{id}"),
                title: project.name.clone(),
                subtitle: root.clone(),
                section: "Worktrees",
                kind: RowKind::Worktree { project_id: id.clone(), directory: root.clone(), main: true, server: None },
            };
            let create = SwitcherRow {
                key: format!("wt:create:{id}"),
                title: t!("workspace.new").into(),
                subtitle: t!("session.new.workspace.new.tooltip").into(),
                section: "Worktrees",
                kind: RowKind::Create { project_id: id.clone(), directory: root.clone(), server: None },
            };
            let worktrees = host.project_worktrees(&id, &root).into_iter().map(|dir| SwitcherRow {
                key: format!("wt:{dir}"),
                title: folder_name(&dir),
                subtitle: dir.clone(),
                section: "Worktrees",
                kind: RowKind::Worktree { project_id: id.clone(), directory: dir, main: false, server: None },
            });
            branches::worktree_rows(host, [main, create].into_iter().chain(worktrees).collect(), q)
        }
        StepMode::Others => host
            .other_projects()
            .into_iter()
            .filter(|p| matches_query(q, &[&p.name, &p.directory]))
            .map(|p| project_row_item(p, false))
            .collect(),
        StepMode::Projects => {
            let used = host.used_projects();
            let others = host.other_projects();
            let other_count = others.len();
            let mut out: Vec<SwitcherRow> = used
                .into_iter()
                .filter(|p| matches_query(q, &[&p.name, &p.directory]))
                .map(|p| project_row_item(p, true))
                .collect();
            if !q.is_empty() {
                out.extend(others.into_iter().filter(|p| matches_query(q, &[&p.name, &p.directory])).map(|p| project_row_item(p, false)));
            } else if other_count > 0 {
                out.push(SwitcherRow {
                    key: "other".into(),
                    title: t!("session.new.location.otherProjects").into(),
                    subtitle: other_count.to_string(),
                    section: "Projects",
                    kind: RowKind::Other { count: other_count },
                });
            }
            out.push(SwitcherRow {
                key: "+add".into(),
                title: t!("session.new.project.add").into(),
                subtitle: String::new(),
                section: "Actions",
                kind: RowKind::Add,
            });
            out
        }
    }
}

pub fn active(host: &dyn ProjectSwitcherHost, rows: &[SwitcherRow]) -> Option<String> {
    host.project_switcher().active.resolve(rows.iter().map(|r| r.key.as_str()))
}

pub fn toggle(host: &mut dyn ProjectSwitcherHost, tx: &mut Tx) {
    tx.instant();
    if host.project_switcher().open {
        close(host, true);
        return;
    }
    // Outside a new session's page it opens one and searches there, so the pick is that
    // draft's project (`titlebar.tsx:478-492`); the page's own command searches in place.
    if !host.is_draft_route() && !host.is_stage() {
        host.open_draft_tab();
        if !session_view::style::touch_controls() {
            host.set_composer_focused(true);
        }
    }
    open_with_focus(host, tx.focused().map(str::to_owned));
}

pub fn open(host: &mut dyn ProjectSwitcherHost) {
    open_with_focus(host, None);
}

pub fn open_with_focus(host: &mut dyn ProjectSwitcherHost, return_focus: Option<String>) {
    if host.project_switcher().open {
        return;
    }
    let restore = host.before_project_switcher_open();
    {
        let ps = host.project_switcher_mut();
        ps.open = true;
        ps.restore_composer = restore;
        ps.mode = StepMode::Projects;
        ps.history.clear();
        ps.search = TextEdit::new("");
        ps.search.focused = true;
        ps.active.clear();
        ps.pointer = None;
    }
    let listed = servers::read(host);
    host.project_switcher_mut().servers = listed;
    host.on_overlay_opened(PROJECT_OVERLAY_ID, return_focus);
}

/// Opens the dialog on `project`'s worktrees, Choose worktree's way
/// (`createLocationDialogState(drilled)`): Escape there closes it, and ← or Backspace with
/// nothing typed step back to the projects with `project` lit.
pub fn open_worktrees(host: &mut dyn ProjectSwitcherHost, project: &str, tx: &mut Tx) {
    open_with_focus(host, tx.focused().map(str::to_owned));
    host.list_worktrees_on(None, project);
    tx.instant();
    let base = host.draft_base().map(str::to_owned);
    let ps = host.project_switcher_mut();
    ps.history = vec![location::Frame {
        mode: StepMode::Projects,
        query: String::new(),
        active: Some(format!("project:{project}")),
    }];
    ps.mode = StepMode::Worktrees { project: project.to_owned(), server: None, direct: true, base };
}

pub fn close(host: &mut dyn ProjectSwitcherHost, restore: bool) {
    if !host.project_switcher().open {
        return;
    }
    let should_restore = {
        let ps = host.project_switcher_mut();
        ps.open = false;
        ps.search.focused = false;
        ps.search.cancel_composition();
        ps.mode = StepMode::Projects;
        ps.history.clear();
        ps.servers.clear();
        restore && ps.restore_composer
    };
    host.after_project_switcher_close(should_restore);
    host.on_overlay_closed(PROJECT_OVERLAY_ID);
}

/// Steps into `project_id`'s worktrees (a Git project's), asking the server for them.
pub fn drill_worktrees(host: &mut dyn ProjectSwitcherHost, project_id: &str, tx: &mut Tx) {
    if !drillable(host, project_id) {
        return;
    }
    host.list_worktrees_on(None, project_id);
    let active = Some(format!("project:{project_id}"));
    location_event(host, location::Event::Drilled { project: project_id.to_owned(), server: None, active }, tx);
}

/// A row's chevron: steps into `project_id`'s worktrees on `server`; `key` is lit again on
/// the way back.
pub(crate) fn drill_server_worktrees(host: &mut dyn ProjectSwitcherHost, server: Option<ServerKey>, project_id: &str, key: String, tx: &mut Tx) {
    if server.is_none() && !drillable(host, project_id) {
        return;
    }
    host.list_worktrees_on(server.as_ref(), project_id);
    location_event(host, location::Event::Drilled { project: project_id.to_owned(), server, active: Some(key) }, tx);
}

pub fn back(host: &mut dyn ProjectSwitcherHost, tx: &mut Tx) {
    location_event(host, location::Event::Back, tx);
}

/// Runs `event` through the step machine and its effects; true when it took the key.
pub(super) fn location_event(host: &mut dyn ProjectSwitcherHost, event: location::Event, tx: &mut Tx) -> bool {
    let (effects, handled) = {
        let ps = host.project_switcher_mut();
        location::step(&mut ps.mode, &ps.search.text, &mut ps.history, event)
    };
    for effect in effects {
        tx.instant();
        match effect {
            location::Effect::Query(query) => {
                let ps = host.project_switcher_mut();
                ps.search = TextEdit::new(&query);
                ps.search.anchor = query.len();
                ps.search.focus = query.len();
            }
            location::Effect::Focus(lit) => {
                let ps = host.project_switcher_mut();
                ps.search.focused = true;
                match lit {
                    Some(key) => ps.active.set(key),
                    None => ps.active.clear(),
                }
            }
            location::Effect::Close => close(host, true),
            location::Effect::CreateFrom { project, server, directory, branch } => {
                close(host, false);
                if let Some(server) = server.filter(|k| *k != host.active_server()) {
                    host.select_server_location(&server, directory.clone(), tx);
                }
                host.choose_new_worktree(&project, directory);
                host.set_draft_base(Some(branch));
                host.auto_focus_composer(tx);
            }
        }
    }
    handled
}

pub fn choose(host: &mut dyn ProjectSwitcherHost, key: &str, tx: &mut Tx) {
    let Some(row) = rows(host).into_iter().find(|r| r.key == key) else { return };
    let worktree = matches!(row.kind, RowKind::Worktree { main: false, .. });
    let base = match &host.project_switcher().mode {
        StepMode::Worktrees { base, .. } => base.clone(),
        _ => None,
    };
    let active_server = host.active_server();
    // Another server's row: the new session moves there first (`selectProject`).
    let elsewhere = match &row.kind {
        RowKind::Project { server: Some(k), .. } | RowKind::Worktree { server: Some(k), .. } | RowKind::Create { server: Some(k), .. } if *k != active_server => Some(k.clone()),
        _ => None,
    };
    if let Some(server) = elsewhere {
        let (RowKind::Project { directory, .. } | RowKind::Worktree { directory, .. } | RowKind::Create { directory, .. }) = &row.kind else {
            return;
        };
        tx.instant();
        close(host, false);
        host.select_server_location(&server, directory.clone(), tx);
        if !matches!(row.kind, RowKind::Project { .. }) {
            // The worktree step's own choices (New worktree, a worktree) run there now.
            choose_here(host, row, worktree, base, tx);
            return;
        }
        host.auto_focus_composer(tx);
        return;
    }
    choose_here(host, row, worktree, base, tx);
}

fn choose_here(host: &mut dyn ProjectSwitcherHost, row: SwitcherRow, worktree: bool, base: Option<String>, tx: &mut Tx) {
    match row.kind {
        RowKind::Base { directory } => branches::open_base(host, directory, tx),
        RowKind::Branch => {
            location_event(host, location::Event::BranchChosen(row.title), tx);
        }
        RowKind::Other { .. } => {
            let lit = active(host, &rows(host));
            location_event(host, location::Event::Others { active: lit }, tx);
        }
        // With several servers Add project asks which one first (`choosesServer`).
        RowKind::Add if !host.project_switcher().servers.is_empty() => {
            location_event(host, location::Event::Servers { active: Some("+add".into()) }, tx);
        }
        RowKind::Add => {
            host.open_project_dialog(tx);
        }
        RowKind::Server { key } => {
            tx.instant();
            close(host, false);
            host.pick_remote_project_folder(&key, t!("command.project.open").to_owned(), tx);
        }
        RowKind::Create { project_id, directory, .. } => {
            tx.instant();
            close(host, false);
            if !host.is_draft_route() {
                host.open_draft_tab();
            }
            host.choose_new_worktree(&project_id, directory);
            host.set_draft_base(base);
            host.sort_projects();
            host.auto_focus_composer(tx);
        }
        RowKind::Project { id, directory, .. } | RowKind::Worktree { project_id: id, directory, .. } => {
            tx.instant();
            host.enqueue_branch_dir(directory.clone());
            if worktree {
                host.note_existing_worktree(&id, directory.clone());
            }
            host.touch_project(&id);
            host.sort_projects();
            close(host, false);
            if host.is_draft_route() {
                host.set_draft_dir(directory);
                host.auto_focus_composer(tx);
            } else {
                host.open_project_dir(&directory, true, tx);
            }
        }
    }
}

pub fn key(host: &mut dyn ProjectSwitcherHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !host.project_switcher().open {
        return false;
    }
    if host.project_switcher().search.is_composing() {
        return false;
    }
    let (text_len, at_end, at_start) = {
        let search = &host.project_switcher().search;
        let len = search.text.len();
        (len, search.anchor == len && search.focus == len, search.anchor == 0 && search.focus == 0)
    };
    let plain = !ev.mods.shift && !ev.mods.cmd && !ev.mods.alt;
    // Right to left, ← goes forward (`machine.ts:152-153`).
    let rtl = opencode_i18n::direction() == opencode_i18n::Direction::Rtl;
    let pressed = match ev.key {
        Key::Right if plain => if rtl { location::Pressed::Backward } else { location::Pressed::Forward },
        Key::Left if plain => if rtl { location::Pressed::Forward } else { location::Pressed::Backward },
        Key::Backspace => location::Pressed::Backspace,
        Key::Escape => location::Pressed::Escape,
        _ => location::Pressed::Other,
    };
    if pressed != location::Pressed::Other {
        let list_rows = rows(host);
        if pressed == location::Pressed::Forward && at_end && active(host, &list_rows).as_deref() == Some("base") {
            choose(host, "base", tx);
            return true;
        }
        let multi = !host.project_switcher().servers.is_empty();
        let lit = active(host, &list_rows).and_then(|a| list_rows.into_iter().find(|r| r.key == a)).map(|r| match r.kind {
            RowKind::Project { id, drillable: d, server: Some(server), .. } => location::Lit::Project { drillable: d, key: r.key, id, server: Some(server) },
            RowKind::Project { id, .. } => location::Lit::Project { drillable: drillable(host, &id), key: r.key, id, server: None },
            RowKind::Other { .. } => location::Lit::Other,
            RowKind::Add if multi => location::Lit::Add,
            _ => location::Lit::Row,
        });
        let k = location::KeyDown { key: pressed, at_start, at_end, empty: text_len == 0, active: lit };
        if let Some(location::Lit::Project { id, drillable: true, server, .. }) = &k.active
            && pressed == location::Pressed::Forward
            && at_end
        {
            let (id, server) = (id.clone(), server.clone());
            host.list_worktrees_on(server.as_ref(), &id);
        }
        if location_event(host, location::Event::Key(k), tx) {
            return true;
        }
    }

    if let Some(mv) = move_for(ev) {
        let list_rows = rows(host);
        let lit = active(host, &list_rows);
        let from = list_rows.iter().position(|r| Some(&r.key) == lit.as_ref()).unwrap_or(0);
        let sections: Vec<&str> = list_rows.iter().map(|r| r.section).collect();
        if let Some(i) = target(&sections, from, mv) {
            let key = list_rows[i].key.clone();
            tx.instant();
            if i == 0 {
                tx.scroll_to_y("project-switcher-scroll", 0.0);
            } else {
                tx.scroll_into_view_in("project-switcher-scroll", format!("psw-{key}"));
            }
            host.project_switcher_mut().activate(key);
        }
        return true;
    }

    match ev.key {
        Key::Enter if !ev.mods.shift => {
            let list_rows = rows(host);
            if let Some(key) = active(host, &list_rows) {
                choose(host, &key, tx);
            }
            true
        }
        Key::Escape => {
            tx.instant();
            close(host, true);
            true
        }
        _ => false,
    }
}

fn project_row_item(p: &Project, used: bool) -> SwitcherRow {
    SwitcherRow {
        key: format!("project:{}", p.id),
        title: p.name.clone(),
        subtitle: p.directory.clone(),
        section: if used { "Projects" } else { "Other projects" },
        kind: RowKind::Project { id: p.id.clone(), directory: p.directory.clone(), used, drillable: project_git(&p.id), server: None },
    }
}

/// A Git project steps into its worktrees (`dialog.tsx:185`), as does one the server already
/// knows worktrees of.
fn drillable(host: &dyn ProjectSwitcherHost, project_id: &str) -> bool {
    project_git(project_id) || host.project(project_id).is_some_and(|p| !host.project_worktrees(project_id, &p.directory).is_empty())
}

// --- Thin non-generic callback helpers invoked by `project/view.rs` ---

pub(crate) fn key_or_shortcut(host: &mut dyn ProjectSwitcherHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    key(host, ev, tx) || host.handle_shortcut(ev, tx)
}

pub(crate) fn branch_search(host: &mut dyn ProjectSwitcherHost, tx: &mut Tx) {
    branches::search(host, tx);
}

pub(crate) fn row_pointer_move(host: &mut dyn ProjectSwitcherHost, key: &str, ev: PointerEvent, tx: &mut Tx) {
    if !host.project_switcher_mut().point(key, [ev.window_x, ev.window_y]) {
        return tx.live_only();
    }
    tx.instant();
}
