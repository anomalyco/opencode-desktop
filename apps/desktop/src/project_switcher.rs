//! ⇧⌘O ("Search projects"), Desktop's project and worktree switcher (`new-session/location/dialog.tsx`),
//! which the new-session page's project and worktree buttons open too.
//!
//! Lists this client's recent/used projects first, then any other server projects (`Other projects`),
//! a Git project's worktrees behind its chevron (its checkout, New worktree, then the server's
//! inventory), and `+ Add project` (`⌘O`) last. Selecting a project updates the open draft's
//! folder on a New Session page, or opens a new session in that project from anywhere else.

use crate::pickers::PickerHost as _;
use opencode_kit::model::{Project, project_git};
use rune::{Color, Cx, El, Key, KeyEvent, Span, Spring, TextEdit, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::mentions::list::Highlight;
use crate::palette::{Elevate as _, Pal};
use crate::projects::folder_name;
use crate::switcher::{move_for, target};
use crate::view::{UI, UI_MED};
use crate::{App, Route};
use opencode_i18n::t;

mod location;
mod branches;
mod servers;
pub(crate) use servers::poll;

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
    Worktrees { project: String, server: Option<opencode_kit::servers::ServerKey>, direct: bool, base: Option<String> },
    Branches { project: String, server: Option<opencode_kit::servers::ServerKey>, direct: bool, directory: String, list: branches::List },
}

#[derive(Default)]
pub struct ProjectSwitcher {
    pub open: bool,
    pub search: TextEdit,
    pub mode: StepMode,
    /// The steps to step back through, innermost last.
    history: Vec<location::Frame>,
    branch_requests: rune::machine::Seq,
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
    Project { id: String, directory: String, used: bool, drillable: bool, server: Option<opencode_kit::servers::ServerKey> },
    Worktree { project_id: String, directory: String, main: bool, server: Option<opencode_kit::servers::ServerKey> },
    /// New worktree: the session starts in a worktree made from the project's checkout.
    Create { project_id: String, directory: String, server: Option<opencode_kit::servers::ServerKey> },
    Base { directory: String },
    Branch,
    Other { count: usize },
    Add,
    /// A server Add project adds to (the Servers step).
    Server { key: opencode_kit::servers::ServerKey },
}

#[derive(Clone, Debug)]
pub struct SwitcherRow {
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub section: &'static str,
    pub kind: RowKind,
}

fn matches_query(q: &str, fields: &[&str]) -> bool {
    let fold = crate::language::search_fold();
    q.trim().is_empty() || fields.iter().any(|f| fold.contains(f, q))
}

fn highlighted(s: &str, q: &str, style: rune::TextStyle, color: Color, accent: Color) -> El {
    let t = text(s.to_owned()).font(style).color(color).truncate();
    match crate::language::search_fold().find(s, q.trim()).first() {
        Some(r) => t.spans(vec![Span::new(r.start, r.end).color(accent)]),
        None => t,
    }
}

impl ProjectSwitcher {
    pub fn activate(&mut self, key: String) {
        self.active.set(key);
    }

    /// The pointer moved on row `key` to `at`: it takes the row only when it moved.
    fn point(&mut self, key: &str, at: [f32; 2]) -> bool {
        if self.pointer.replace(at) == Some(at) || self.active.pointed(key) {
            return false;
        }
        self.active.step(crate::popover::HighlightEvent::Pointer(key.to_owned()));
        true
    }

    pub fn query(&self) -> &str {
        self.search.text.trim()
    }

    pub(crate) fn restore_composer(&self) -> bool {
        self.restore_composer
    }
}

impl App {
    pub fn project_switcher_rows(&self) -> Vec<SwitcherRow> {
        let q = self.project_switcher.query();
        if let StepMode::Branches { list, .. } = &self.project_switcher.mode {
            return branches::rows(list, q);
        }
        if !self.project_switcher.servers.is_empty() {
            return match &self.project_switcher.mode {
                StepMode::Worktrees { project, server, .. } => {
                    let server = server.clone().unwrap_or_else(|| crate::servers::runtime::active(self).clone());
                    self.location_worktree_rows(servers::worktree_rows(self, &server, project), q)
                }
                StepMode::Servers => servers::server_rows(self, q),
                mode => servers::project_rows(self, q, *mode == StepMode::Others),
            };
        }
        match &self.project_switcher.mode {
            StepMode::Branches { list, .. } => branches::rows(list, q),
            StepMode::Servers => Vec::new(),
            StepMode::Worktrees { project: pid, .. } => {
                let Some(project) = self.project(pid) else { return Vec::new() };
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
                let worktrees = self.project_worktrees(&id, &root).into_iter().map(|dir| SwitcherRow {
                    key: format!("wt:{dir}"),
                    title: folder_name(&dir),
                    subtitle: dir.clone(),
                    section: "Worktrees",
                    kind: RowKind::Worktree { project_id: id.clone(), directory: dir, main: false, server: None },
                });
                self.location_worktree_rows([main, create].into_iter().chain(worktrees).collect(), q)
            }
            StepMode::Others => self
                .other_projects()
                .into_iter()
                .filter(|p| matches_query(q, &[&p.name, &p.directory]))
                .map(|p| project_row_item(p, false))
                .collect(),
            StepMode::Projects => {
                let used = self.used_projects();
                let others = self.other_projects();
                let other_count = others.len();
                let mut out: Vec<SwitcherRow> = used
                    .into_iter()
                    .filter(|p| matches_query(q, &[&p.name, &p.directory]))
                    .map(|p| project_row_item(p, true))
                    .collect();
                if !q.is_empty() {
                    out.extend(
                        others
                            .into_iter()
                            .filter(|p| matches_query(q, &[&p.name, &p.directory]))
                            .map(|p| project_row_item(p, false)),
                    );
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

    pub fn project_switcher_active(&self, rows: &[SwitcherRow]) -> Option<String> {
        self.project_switcher.active.resolve(rows.iter().map(|r| r.key.as_str()))
    }

    pub fn toggle_project_switcher(&mut self, tx: &mut Tx) {
        tx.instant();
        if self.project_switcher.open {
            self.close_project_switcher(true);
            return;
        }
        // Outside a new session's page it opens one and searches there, so the pick is that
        // draft's project (`titlebar.tsx:478-492`); the page's own command searches in place.
        if !matches!(*self.route, Route::Draft(_)) && self.stage.is_none() {
            self.open_draft_tab();
            if !session_view::style::touch_controls() {
                self.composer.focused = true;
            }
        }
        self.open_project_switcher();
    }

    pub fn open_project_switcher(&mut self) {
        if self.project_switcher.open {
            return;
        }
        let restore = self.composer.focused
            || (self.switcher.open && self.switcher.restore_composer())
            || self.pickers.open().is_some();
        self.close_picker(false);
        self.close_switcher(false);
        self.project_state.home_menu_open = false;
        self.settings.open = false;
        self.search.focused = false;
        self.composer.focused = false;
        self.project_switcher.open = true;
        self.project_switcher.restore_composer = restore;
        self.project_switcher.mode = StepMode::Projects;
        self.project_switcher.history.clear();
        self.project_switcher.search = TextEdit::new("");
        self.project_switcher.search.focused = true;
        self.project_switcher.active.clear();
        self.project_switcher.pointer = None;
        self.project_switcher.servers = servers::read(self);
    }

    /// Opens the dialog on `project`'s worktrees, Choose worktree's way
    /// (`createLocationDialogState(drilled)`): Escape there closes it, and ← or Backspace with
    /// nothing typed step back to the projects with `project` lit.
    pub fn open_project_worktrees(&mut self, project: &str, tx: &mut Tx) {
        self.open_project_switcher();
        self.list_worktrees_on(None, project);
        tx.instant();
        let base = self.draft_base().map(str::to_owned);
        let ps = &mut *self.project_switcher;
        ps.history = vec![location::Frame { mode: StepMode::Projects, query: String::new(), active: Some(format!("project:{project}")) }];
        ps.mode = StepMode::Worktrees { project: project.to_owned(), server: None, direct: true, base };
    }

    pub fn close_project_switcher(&mut self, restore: bool) {
        if !self.project_switcher.open {
            return;
        }
        self.project_switcher.open = false;
        self.project_switcher.search.focused = false;
        self.project_switcher.search.cancel_composition();
        self.project_switcher.mode = StepMode::Projects;
        self.project_switcher.history.clear();
        self.project_switcher.servers.clear();
        if restore && self.project_switcher.restore_composer && self.stage.is_none() {
            self.composer.focused = true;
        }
    }

    /// Steps into `project_id`'s worktrees (a Git project's), asking the server for them.
    pub fn drill_project_worktrees(&mut self, project_id: &str, tx: &mut Tx) {
        if !drillable(self, project_id) {
            return;
        }
        self.list_worktrees_on(None, project_id);
        let active = Some(format!("project:{project_id}"));
        self.location_event(location::Event::Drilled { project: project_id.to_owned(), server: None, active }, tx);
    }

    /// Asks `server` (None: the window's own) for project `id`'s worktrees.
    fn list_worktrees_on(&mut self, server: Option<&opencode_kit::servers::ServerKey>, id: &str) {
        let key = server.cloned().unwrap_or_else(|| crate::servers::runtime::active(self).clone());
        crate::servers::runtime::with_server(self, &key, |s| {
            s.list_worktrees(id);
            if let Some(dir) = s.project(id).map(|p| p.directory.clone()) {
                s.sidebar_inbox.branches.enqueue([dir]);
            }
        });
    }

    /// A row's chevron: steps into `project_id`'s worktrees on `server`; `key` is lit again on
    /// the way back.
    fn drill_server_worktrees(&mut self, server: Option<opencode_kit::servers::ServerKey>, project_id: &str, key: String, tx: &mut Tx) {
        if server.is_none() && !drillable(self, project_id) {
            return;
        }
        self.list_worktrees_on(server.as_ref(), project_id);
        self.location_event(location::Event::Drilled { project: project_id.to_owned(), server, active: Some(key) }, tx);
    }

    pub fn project_switcher_back(&mut self, tx: &mut Tx) {
        self.location_event(location::Event::Back, tx);
    }

    /// Runs `event` through the step machine and its effects; true when it took the key.
    fn location_event(&mut self, event: location::Event, tx: &mut Tx) -> bool {
        let ps = &mut *self.project_switcher;
        let (effects, handled) = location::step(&mut ps.mode, &ps.search.text, &mut ps.history, event);
        for effect in effects {
            tx.instant();
            match effect {
                location::Effect::Query(query) => {
                    let ps = &mut *self.project_switcher;
                    ps.search = TextEdit::new(&query);
                    ps.search.anchor = query.len();
                    ps.search.focus = query.len();
                }
                location::Effect::Focus(active) => {
                    let ps = &mut *self.project_switcher;
                    ps.search.focused = true;
                    match active {
                        Some(key) => ps.active.set(key),
                        None => ps.active.clear(),
                    }
                }
                location::Effect::Close => self.close_project_switcher(true),
            }
        }
        handled
    }

    pub fn project_switcher_choose(&mut self, key: &str, tx: &mut Tx) {
        let Some(row) = self.project_switcher_rows().into_iter().find(|r| r.key == key) else { return };
        let worktree = matches!(row.kind, RowKind::Worktree { main: false, .. });
        let base = match &self.project_switcher.mode { StepMode::Worktrees { base, .. } => base.clone(), _ => None };
        // Another server's row: the new session moves there first (`selectProject`).
        let elsewhere = match &row.kind {
            RowKind::Project { server: Some(k), .. } | RowKind::Worktree { server: Some(k), .. } | RowKind::Create { server: Some(k), .. } if k != crate::servers::runtime::active(self) => Some(k.clone()),
            _ => None,
        };
        if let Some(server) = elsewhere {
            let (RowKind::Project { directory, .. } | RowKind::Worktree { directory, .. } | RowKind::Create { directory, .. }) = &row.kind else { return };
            tx.instant();
            self.close_project_switcher(false);
            servers::select(self, &server, directory.clone(), tx);
            if !matches!(row.kind, RowKind::Project { .. }) {
                // The worktree step's own choices (New worktree, a worktree) run there now.
                return self.project_switcher_choose_here(row, worktree, base, tx);
            }
            if self.stage.is_none() && crate::auto_focus_composer(tx) {
                self.composer.focused = true;
            }
            return;
        }
        self.project_switcher_choose_here(row, worktree, base, tx);
    }

    fn project_switcher_choose_here(&mut self, row: SwitcherRow, worktree: bool, base: Option<String>, tx: &mut Tx) {
        match row.kind {
            RowKind::Base { directory } => self.location_base(directory, tx),
            RowKind::Branch => { self.location_event(location::Event::BranchChosen(row.title), tx); }
            RowKind::Other { .. } => {
                let active = self.project_switcher_active(&self.project_switcher_rows());
                self.location_event(location::Event::Others { active }, tx);
            }
            // With several servers Add project asks which one first (`choosesServer`).
            RowKind::Add if !self.project_switcher.servers.is_empty() => {
                self.location_event(location::Event::Servers { active: Some("+add".into()) }, tx);
            }
            RowKind::Add => {
                self.open_project_dialog(tx);
            }
            RowKind::Server { key } => {
                tx.instant();
                servers::add(self, &key, tx);
            }
            RowKind::Create { project_id, directory, .. } => {
                tx.instant();
                self.close_project_switcher(false);
                if !matches!(*self.route, Route::Draft(_)) {
                    self.open_draft_tab();
                }
                self.choose_new_worktree(&project_id, directory);
                self.set_draft_base(base);
                self.sort_projects();
                if self.stage.is_none() && crate::auto_focus_composer(tx) {
                    self.composer.focused = true;
                }
            }
            RowKind::Project { id, directory, .. } | RowKind::Worktree { project_id: id, directory, .. } => {
                tx.instant();
                self.sidebar_inbox.branches.enqueue([directory.clone()]);
                if worktree {
                    self.note_existing_worktree(&id, directory.clone());
                }
                self.touch_project(&id);
                self.sort_projects();
                self.close_project_switcher(false);
                if matches!(*self.route, Route::Draft(_)) {
                    self.set_draft_dir(directory);
                    if self.stage.is_none() && crate::auto_focus_composer(tx) {
                        self.composer.focused = true;
                    }
                } else {
                    self.open_project_dir(&directory, true, tx);
                }
            }
        }
    }

    pub fn project_switcher_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.project_switcher.open {
            return false;
        }
        if self.project_switcher.search.is_composing() {
            return false;
        }
        let search = &self.project_switcher.search;
        let text_len = search.text.len();
        let at_end = search.anchor == text_len && search.focus == text_len;
        let at_start = search.anchor == 0 && search.focus == 0;
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
            let rows = self.project_switcher_rows();
            if pressed == location::Pressed::Forward && at_end && self.project_switcher_active(&rows).as_deref() == Some("base") {
                self.project_switcher_choose("base", tx);
                return true;
            }
            let active = self.project_switcher_active(&rows).and_then(|a| rows.into_iter().find(|r| r.key == a)).map(|r| match r.kind {
                RowKind::Project { id, drillable: d, server: Some(server), .. } => location::Lit::Project { drillable: d, key: r.key, id, server: Some(server) },
                RowKind::Project { id, .. } => location::Lit::Project { drillable: drillable(self, &id), key: r.key, id, server: None },
                RowKind::Other { .. } => location::Lit::Other,
                RowKind::Add if !self.project_switcher.servers.is_empty() => location::Lit::Add,
                _ => location::Lit::Row,
            });
            let k = location::KeyDown { key: pressed, at_start, at_end, empty: text_len == 0, active };
            if let Some(location::Lit::Project { id, drillable: true, server, .. }) = &k.active
                && pressed == location::Pressed::Forward
                && at_end
            {
                let (id, server) = (id.clone(), server.clone());
                self.list_worktrees_on(server.as_ref(), &id);
            }
            if self.location_event(location::Event::Key(k), tx) {
                return true;
            }
        }

        if let Some(mv) = move_for(ev) {
            let rows = self.project_switcher_rows();
            let active = self.project_switcher_active(&rows);
            let from = rows.iter().position(|r| Some(&r.key) == active.as_ref()).unwrap_or(0);
            let sections: Vec<&str> = rows.iter().map(|r| r.section).collect();
            if let Some(i) = target(&sections, from, mv) {
                let key = rows[i].key.clone();
                tx.instant();
                if i == 0 {
                    tx.scroll_to_y("project-switcher-scroll", 0.0);
                } else {
                    tx.scroll_into_view_in("project-switcher-scroll", format!("psw-{key}"));
                }
                self.project_switcher.activate(key);
            }
            return true;
        }

        match ev.key {
            Key::Enter if !ev.mods.shift => {
                let rows = self.project_switcher_rows();
                if let Some(key) = self.project_switcher_active(&rows) {
                    self.project_switcher_choose(&key, tx);
                }
                true
            }
            Key::Escape => {
                tx.instant();
                self.close_project_switcher(true);
                true
            }
            _ => false,
        }
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
fn drillable(s: &App, project_id: &str) -> bool {
    project_git(project_id) || s.project(project_id).is_some_and(|p| !s.project_worktrees(project_id, &p.directory).is_empty())
}

/// A row (`palette-menu-row` at `data-size="compact"`, `dialog.css:97-121`): 28 tall, 6 in, 6 pt
/// corners, the hover overlay while lit; its glyph, then its title (13 at 530) and its folder
/// or description in faint, both cut with an ellipsis, and a chevron where it steps in.
fn draw_row(s: &App, p: &Pal, r: &SwitcherRow, active: bool, q: &str, w: f32) -> El {
    let key = r.key.clone();
    let hover = r.key.clone();
    let option = matches!(r.kind, RowKind::Other { .. } | RowKind::Add);
    let multi = !s.project_switcher.servers.is_empty();
    let title_ink = if option { p.muted } else { p.text };
    let title = highlighted(&r.title, q, TITLE, title_ink, p.accent);
    let description = |t: &str| highlighted(t, q, UI, p.faint, p.accent).grow(1.0).shrink().min_w(0.0);
    // The name keeps up to 55% of its text (a worktree's title 60%) while the folder gives way:
    // the row less its insets, glyph and chevron.
    let text_w = w - 12.0 - 12.0 - 24.0 - if matches!(r.kind, RowKind::Project { drillable: true, .. }) { 24.0 } else { 0.0 };
    let named = |t: El, share: f32| row().key("name").max_w(text_w * share).center_y().child(t.shrink().min_w(0.0));
    let main = row().key("main").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y();
    let main = match &r.kind {
        RowKind::Project { id, server, .. } | RowKind::Worktree { project_id: id, main: true, server, .. } => {
            // Another server's project shows its server's tag after its name (`ServerTag`).
            let (project, tag) = match server {
                Some(k) => (servers::project(s, k, id), servers::tag(s, p, k)),
                None => (s.project(id), None),
            };
            let branch = (matches!(r.kind, RowKind::Worktree { main: true, .. }) && tag.is_none()).then(|| s.location_checkout(server.as_ref(), &r.subtitle)).flatten();
            let name = row().key("name").max_w(text_w * 0.55).gap(6.0).center_y().child(title.shrink().min_w(0.0)).children(tag).children(branch.map(|b| row().gap(4.0).center_y().child(icon(&ic::BRANCH,12.0).color(p.icon_muted)).child(text(b.to_owned()).font(UI).color(p.muted).truncate())));
            main.child(crate::icons::badge(s, p, project, &r.title)).child(row().key("text").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().child(name).child(description(&r.subtitle)))
        }
        RowKind::Base { .. } => main.child(icon(&ic::BRANCH, 16.0).color(p.icon_muted)).child(title).child(description(&r.subtitle)).child(crate::keymap::keycaps(p, crate::keymap::Bind::BranchChoose)),
        RowKind::Branch => main.child(icon(&ic::BRANCH_OUT, 16.0).color(p.icon_muted)).child(title.dir(rune::Direction::Auto)),
        RowKind::Server { key } => main
            .child(icon(servers::glyph(servers::kind(s, key)), 16.0).color(p.icon_muted))
            .child(row().key("text").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().child(title.shrink().min_w(0.0)).children((!r.subtitle.is_empty()).then(|| description(&r.subtitle)))),
        RowKind::Worktree { main: false, .. } | RowKind::Create { .. } => {
            let glyph = if matches!(r.kind, RowKind::Create { .. }) { &ic::PLUS } else { &ic::OUTLINE_WORKTREE };
            main.child(icon(glyph, 16.0).color(p.icon_muted))
                .child(row().key("text").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().child(named(title, 0.6)).child(description(&r.subtitle)))
        }
        RowKind::Other { count } => main
            .child(icon(&ic::MENU, 16.0).color(p.icon_muted))
            .child(title.shrink().min_w(0.0))
            .child(text(count.to_string()).font(UI.features("\"tnum\"")).color(p.faint)),
        RowKind::Add => main.child(icon(&ic::PLUS, 16.0).color(p.icon_muted)).child(title.shrink().min_w(0.0)),
    };
    let line = row()
        .key(format!("psw-{}", r.key))
        .role("option")
        .label(r.title.clone())
        .w_full()
        .h(28.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .fill(if active { p.hover } else { Color::TRANSPARENT })
        // Only a moving pointer takes the row (`dialog.tsx:395-399`), so rows the keys scroll
        // under a still one leave the highlight alone.
        .on_pointer_move_local(move |s: &mut App, ev: rune::PointerEvent, tx: &mut Tx| {
            if !s.project_switcher.point(&hover, [ev.window_x, ev.window_y]) {
                return tx.live_only();
            }
            tx.instant();
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.project_switcher_choose(&key, tx))
        .child(main);
    match &r.kind {
        RowKind::Add if multi => line.child(crate::keymap::keycaps(p, crate::keymap::Bind::OpenProject)).child(drill(p, "psw-drill-servers", t!("session.new.location.servers"), |s: &mut App, tx: &mut Tx| s.project_switcher_choose("+add", tx))),
        RowKind::Add => line.child(crate::keymap::keycaps(p, crate::keymap::Bind::OpenProject)),
        RowKind::Other { .. } => line.child(drill(p, "psw-drill-other", t!("session.new.location.otherProjects"), |s: &mut App, tx: &mut Tx| s.project_switcher_choose("other", tx))),
        RowKind::Project { id, drillable: true, server, .. } => {
            let (pid, server, key) = (id.clone(), server.clone(), r.key.clone());
            line.child(drill(p, &format!("psw-drill-{id}"), t!("session.new.location.worktrees"), move |s: &mut App, tx: &mut Tx| s.drill_server_worktrees(server.clone(), &pid, key.clone(), tx)))
        }
        _ => line,
    }
}

/// `new-session-location-drill`: a 20 pt chevron, faint, that lights with its own hover.
fn drill(p: &Pal, key: &str, label: &str, on: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    el().key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .size(20.0, 20.0)
        .mx(-2.0)
        .radius(4.0)
        .center()
        .hover_group()
        .hover(p.hover)
        .on_press_tx(on)
        .child(icon(&ic::CHEVRON_RIGHT, 16.0).mirror_in_rtl().color(p.icon_faint).group_color(p.icon))
}

/// A row's title: `palette-menu-title`, 13 at 530.
const TITLE: rune::TextStyle = UI_MED;

/// The dialog (`<Dialog size="large" fit>`, `new-session/location/dialog.tsx`): large, on bg-base
/// with `--v2-elevation-overlay` and 12 pt corners, as tall as it needs up to the size's height,
/// growing down from the size's top edge. The search field (28 tall, transparent, the placeholder in the base ink at
/// half strength), a hairline 8 below it, and the rows 4 apart under it.
pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let [win_w, win_h] = cx
        .layout_size("window")
        .or_else(|| cx.layout_size("zoomed"))
        .unwrap_or([1280.0 / s.zoom.max(1e-3), 820.0 / s.zoom.max(1e-3)]);
    let size = crate::widgets::DialogSize::Large;
    let w = size.width(win_w);
    let max_h = size.height(win_h);
    let q = s.project_switcher.query();
    let rows = s.project_switcher_rows();
    let active = s.project_switcher_active(&rows);

    let (placeholder, step_title) = match &s.project_switcher.mode {
        StepMode::Projects => (t!("session.new.project.search"), None),
        StepMode::Others => (t!("session.new.project.search"), Some(t!("session.new.location.otherProjects"))),
        StepMode::Servers => (t!("session.new.location.servers.search"), Some(t!("session.new.location.servers"))),
        StepMode::Branches { .. } => (t!("session.new.workspace.branch.search.placeholder"), Some(t!("session.new.workspace.runIn"))),
        StepMode::Worktrees { .. } => (t!("session.new.workspace.search.placeholder"), Some(t!("session.new.workspace.runIn"))),
    };
    let search = row()
        .key("project-switcher-field")
        .h(28.0)
        .pl(6.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            session_view::style::field(&s.project_switcher.search, UI)
                .key("project-switcher-input")
                .single_line()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .color(p.text)
                .placeholder(placeholder, p.text.alpha(p.text.a * 0.5))
                .caret(p.text, p.selection).selection_style(p.field_selection)
                .on_edit(|s: &mut App| &mut s.project_switcher.search)
                .on_input_change(|s: &mut App, tx: &mut Tx| s.location_branch_search(tx))
                .on_input_key(|s: &mut App, ev, tx| s.project_switcher_key(ev, tx) || s.shortcut(ev, tx)),
        );

    let mut list = col().key("project-switcher-results").w_full().px(6.0).pb(6.0).gap(4.0);
    let mut content_h = 6.0_f32;
    if let Some(title) = step_title {
        list = list.child(
            row()
                .key("project-switcher-back")
                .role("button")
                .label_static(t!("session.new.location.back"))
                .h(28.0)
                .px(6.0)
                .center_y()
                .radius(2.0)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.project_switcher_back(tx))
                .child(text(title).font(UI).color(p.muted)),
        );
        content_h += 32.0;
    }
    if rows.is_empty() {
        let waiting = matches!(s.project_switcher.mode, StepMode::Branches { list: branches::List::Loading(_), .. });
        list = list.child(col().key("project-switcher-empty").w_full().h(120.0).center().children((!waiting).then(|| text(t!("palette.empty")).font(UI).color(p.muted))));
        content_h += 120.0;
    }
    let mut shown_other_heading = false;
    for (idx, r) in rows.iter().enumerate() {
        if !q.is_empty() && s.project_switcher.mode == StepMode::Projects && r.section == "Other projects" && !shown_other_heading {
            shown_other_heading = true;
            list = list.child(row().key("psw-other-heading").h(28.0).px(6.0).center_y().child(text(t!("session.new.location.otherProjects")).font(UI).color(p.muted)));
            content_h += 32.0;
        }
        // A hairline across the dialog before Add project, and between New worktree and the
        // worktrees (`dialog.tsx:371-382`).
        let after_actions = idx > 0 && matches!(r.kind, RowKind::Worktree { main: false, .. }) && !matches!(rows[idx - 1].kind, RowKind::Worktree { main: false, .. });
        if idx > 0 && (r.kind == RowKind::Add || after_actions) {
            list = list.child(el().key(format!("psw-divider-{idx}")).h(0.5).my(2.0).mx(-6.0).fill(p.border_muted));
            content_h += 8.5;
        }
        list = list.child(draw_row(s, p, r, active.as_deref() == Some(r.key.as_str()), q, w));
        content_h += 32.0;
    }
    content_h -= 4.0;

    // The search block (6 above, the field) and the divider (9 between 4 and 4).
    let head_h = 6.0 + 28.0 + 17.0;
    let scroll_h = content_h.min(max_h - head_h).max(0.0);
    let total_h = head_h + scroll_h;
    let dialog = col()
        .key("project-switcher")
        .role("dialog")
        .label(placeholder)
        .w(w)
        .h(total_h)
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| s.close_project_switcher(true))
        .child(col().key("project-switcher-search").w_full().pt(6.0).px(6.0).child(search))
        .child(el().key("project-switcher-divider").h(0.5).w_full().mt(8.0).mb(8.5).fill(p.border))
        .child(
            // `MenuScroll` (`new-session/location/dialog.tsx:335`): its thumb shows while the list scrolls.
            session_view::scroller::scroller("project-switcher-scroll").bar(session_view::scroller::Bar::MenuScroll).build::<App>(p, col().role("listbox").w_full().h(scroll_h), list),
        );
    let y = size.top(win_h);
    let scrim = el().key("project-switcher-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog.absolute((win_w - w) / 2.0, y)]
}
