//! ⇧⌘O ("Search projects"), Desktop's project and worktree switcher (`new-session/location/dialog.tsx`).
//!
//! Lists this client's recent/used projects first, then any other server projects (`Other projects`),
//! existing Git worktrees (`sandboxes`) behind `→`, and `+ Add project` (`⌘O`) at the foot.
//! Selecting a project updates `draft_dir` on a New Session page, or opens a new session in that
//! project from anywhere else.

use crate::pickers::PickerHost as _;
use opencode_kit::model::{Project, project_sandboxes};
use rune::{
    Color, Cx, El, Key, KeyEvent, ScrollSpec, Span, Spring, TextEdit, Transition, Tx, col, el, icon, row, spacer, text,
    text_input,
};
use rune_theme_opencode::icons as ic;

use crate::mentions::list::Highlight;
use crate::palette::Pal;
use crate::projects::{clean_dir, folder_name};
use crate::switcher::{move_for, target};
use crate::view::{UI, UI_MED};
use crate::{App, Route};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StepMode {
    #[default]
    Projects,
    Others,
    Worktrees(String),
}

#[derive(Clone, Debug)]
struct SavedStep {
    query: String,
    active: String,
    others: bool,
}

#[derive(Default)]
pub struct ProjectSwitcher {
    pub open: bool,
    pub search: TextEdit,
    pub mode: StepMode,
    active: Highlight,
    restore_composer: bool,
    back: Option<SavedStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    Project { id: String, directory: String, used: bool, drillable: bool },
    Worktree { project_id: String, directory: String, main: bool },
    Other { count: usize },
    Add,
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
    let q = q.trim().to_lowercase();
    q.is_empty() || fields.iter().any(|f| f.to_lowercase().contains(&q))
}

fn highlighted(s: &str, q: &str, style: rune::TextStyle, color: Color, accent: Color) -> El {
    let t = text(s.to_owned()).font(style).color(color).truncate_fade();
    let lower = s.to_lowercase();
    let needle = q.trim().to_lowercase();
    match lower.find(&needle) {
        Some(at) if !needle.is_empty() && lower.len() == s.len() => {
            t.spans(vec![Span::new(at, at + needle.len()).color(accent)])
        }
        _ => t,
    }
}

impl ProjectSwitcher {
    pub fn activate(&mut self, key: String) {
        self.active.set(key, self.search.text.clone());
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
        match &self.project_switcher.mode {
            StepMode::Worktrees(pid) => {
                let Some(project) = self.project(pid) else { return Vec::new() };
                let main_short = self.tilde(&project.directory);
                let mut out = Vec::new();
                if matches_query(q, &[&project.name, &project.directory, &main_short]) {
                    out.push(SwitcherRow {
                        key: format!("wt:main:{}", project.id),
                        title: project.name.clone(),
                        subtitle: main_short,
                        section: "Worktrees",
                        kind: RowKind::Worktree {
                            project_id: project.id.clone(),
                            directory: project.directory.clone(),
                            main: true,
                        },
                    });
                }
                for dir in project_sandboxes(&project.id) {
                    let dir = clean_dir(&dir);
                    if dir.is_empty() || dir == clean_dir(&project.directory) {
                        continue;
                    }
                    let title = folder_name(&dir);
                    let short = self.tilde(&dir);
                    if matches_query(q, &[&title, &dir, &short]) {
                        out.push(SwitcherRow {
                            key: format!("wt:{dir}"),
                            title,
                            subtitle: short,
                            section: "Worktrees",
                            kind: RowKind::Worktree { project_id: project.id.clone(), directory: dir, main: false },
                        });
                    }
                }
                out
            }
            StepMode::Others => self
                .other_projects()
                .into_iter()
                .filter(|p| matches_query(q, &[&p.name, &p.directory, &self.tilde(&p.directory)]))
                .map(|p| project_row_item(p, self.tilde(&p.directory), false))
                .collect(),
            StepMode::Projects => {
                let used = self.used_projects();
                let others = self.other_projects();
                let other_count = others.len();
                let mut out: Vec<SwitcherRow> = used
                    .into_iter()
                    .filter(|p| matches_query(q, &[&p.name, &p.directory, &self.tilde(&p.directory)]))
                    .map(|p| project_row_item(p, self.tilde(&p.directory), true))
                    .collect();
                if !q.is_empty() {
                    out.extend(
                        others
                            .into_iter()
                            .filter(|p| matches_query(q, &[&p.name, &p.directory, &self.tilde(&p.directory)]))
                            .map(|p| project_row_item(p, self.tilde(&p.directory), false)),
                    );
                } else if other_count > 0 {
                    out.push(SwitcherRow {
                        key: "other".into(),
                        title: "Other projects".into(),
                        subtitle: other_count.to_string(),
                        section: "Projects",
                        kind: RowKind::Other { count: other_count },
                    });
                }
                out.push(SwitcherRow {
                    key: "+add".into(),
                    title: "Add project".into(),
                    subtitle: String::new(),
                    section: "Actions",
                    kind: RowKind::Add,
                });
                out
            }
        }
    }

    pub fn project_switcher_active(&self, rows: &[SwitcherRow]) -> Option<String> {
        self.project_switcher.active.resolve(&self.project_switcher.search.text, rows.iter().map(|r| r.key.as_str()))
    }

    pub fn toggle_project_switcher(&mut self, tx: &mut Tx) {
        tx.instant();
        if self.project_switcher.open {
            self.close_project_switcher(true);
        } else {
            self.open_project_switcher();
        }
    }

    pub fn open_project_switcher(&mut self) {
        if self.project_switcher.open {
            return;
        }
        let restore = self.composer.focused
            || (self.switcher.open && self.switcher.restore_composer())
            || self.pickers.open.is_some();
        self.close_picker(false);
        self.close_switcher(false);
        self.project_state.home_menu_open = false;
        self.settings.open = false;
        self.search.focused = false;
        self.composer.focused = false;
        self.project_switcher.open = true;
        self.project_switcher.restore_composer = restore;
        self.project_switcher.mode = StepMode::Projects;
        self.project_switcher.back = None;
        self.project_switcher.search = TextEdit::new("");
        self.project_switcher.search.focused = true;
        self.project_switcher.active.clear();
    }

    pub fn close_project_switcher(&mut self, restore: bool) {
        if !self.project_switcher.open {
            return;
        }
        self.project_switcher.open = false;
        self.project_switcher.search.focused = false;
        self.project_switcher.search.preedit = None;
        self.project_switcher.mode = StepMode::Projects;
        self.project_switcher.back = None;
        if restore && self.project_switcher.restore_composer && self.stage.is_none() {
            self.composer.focused = true;
        }
    }

    pub fn drill_project_worktrees(&mut self, project_id: &str, tx: &mut Tx) {
        if project_sandboxes(project_id).is_empty() {
            return;
        }
        tx.instant();
        let active = format!("project:{project_id}");
        self.project_switcher.back = Some(SavedStep {
            query: self.project_switcher.search.text.clone(),
            active,
            others: self.project_switcher.mode == StepMode::Others,
        });
        self.project_switcher.mode = StepMode::Worktrees(project_id.to_owned());
        self.project_switcher.search = TextEdit::new("");
        self.project_switcher.search.focused = true;
        self.project_switcher.active.clear();
    }

    pub fn project_switcher_back(&mut self, tx: &mut Tx) {
        tx.instant();
        let saved = self.project_switcher.back.take();
        let (query, active, others) = match (&self.project_switcher.mode, saved) {
            (StepMode::Others, _) => (String::new(), "other".to_owned(), false),
            (_, Some(s)) => (s.query, s.active, s.others),
            _ => (String::new(), String::new(), false),
        };
        self.project_switcher.mode = if others { StepMode::Others } else { StepMode::Projects };
        self.project_switcher.search = TextEdit::new(&query);
        self.project_switcher.search.anchor = query.len();
        self.project_switcher.search.focus = query.len();
        self.project_switcher.search.focused = true;
        if active.is_empty() {
            self.project_switcher.active.clear();
        } else {
            self.project_switcher.active.set(active, query);
        }
    }

    pub fn project_switcher_choose(&mut self, key: &str, tx: &mut Tx) {
        let Some(row) = self.project_switcher_rows().into_iter().find(|r| r.key == key) else { return };
        match row.kind {
            RowKind::Other { .. } => {
                tx.instant();
                self.project_switcher.mode = StepMode::Others;
                self.project_switcher.search = TextEdit::new("");
                self.project_switcher.search.focused = true;
                self.project_switcher.active.clear();
            }
            RowKind::Add => {
                self.open_project_dialog(tx);
            }
            RowKind::Project { id, directory, .. } | RowKind::Worktree { project_id: id, directory, .. } => {
                tx.instant();
                self.touch_project(&id);
                self.sort_projects();
                self.close_project_switcher(false);
                if self.route == Route::Draft {
                    self.draft_dir = directory;
                    if self.stage.is_none() {
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
        if self.project_switcher.search.preedit.is_some() {
            return false;
        }
        let text_len = self.project_switcher.search.text.len();
        let at_end = self.project_switcher.search.anchor == text_len && self.project_switcher.search.focus == text_len;
        let at_start = self.project_switcher.search.anchor == 0 && self.project_switcher.search.focus == 0;
        let stepped = self.project_switcher.mode != StepMode::Projects;

        if ev.key == Key::Right && !ev.mods.shift && !ev.mods.cmd && !ev.mods.alt && at_end {
            let rows = self.project_switcher_rows();
            if let Some(active) = self.project_switcher_active(&rows)
                && let Some(r) = rows.iter().find(|r| r.key == active)
            {
                match &r.kind {
                    RowKind::Project { id, drillable: true, .. } => {
                        let id = id.clone();
                        self.drill_project_worktrees(&id, tx);
                        return true;
                    }
                    RowKind::Other { .. } => {
                        self.project_switcher_choose("other", tx);
                        return true;
                    }
                    _ => {}
                }
            }
        }

        if stepped
            && ((ev.key == Key::Left && !ev.mods.shift && !ev.mods.cmd && !ev.mods.alt && at_start)
                || (ev.key == Key::Backspace && text_len == 0)
                || ev.key == Key::Escape)
        {
            self.project_switcher_back(tx);
            return true;
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

fn project_row_item(p: &Project, subtitle: String, used: bool) -> SwitcherRow {
    let drillable = !project_sandboxes(&p.id).is_empty();
    SwitcherRow {
        key: format!("project:{}", p.id),
        title: p.name.clone(),
        subtitle,
        section: if used { "Projects" } else { "Other projects" },
        kind: RowKind::Project { id: p.id.clone(), directory: p.directory.clone(), used, drillable },
    }
}

fn draw_row(s: &App, p: &Pal, r: &SwitcherRow, active: bool, q: &str) -> El {
    let key = r.key.clone();
    let hover = r.key.clone();
    let mut main = row().key("main").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y();
    match &r.kind {
        RowKind::Project { id, .. } => {
            main = main
                .child(crate::icons::badge(s, p, s.project(id), &r.title))
                .child(highlighted(&r.title, q, UI_MED, p.text, p.accent))
                .child(highlighted(&r.subtitle, q, UI, p.faint, p.accent));
        }
        RowKind::Worktree { project_id, main: true, .. } => {
            main = main
                .child(crate::icons::badge(s, p, s.project(project_id), &r.title))
                .child(highlighted(&r.title, q, UI_MED, p.text, p.accent))
                .child(highlighted(&r.subtitle, q, UI, p.faint, p.accent));
        }
        RowKind::Worktree { main: false, .. } => {
            main = main
                .child(icon(&ic::OUTLINE_WORKTREE, 16.0).color(p.icon_muted))
                .child(highlighted(&r.title, q, UI_MED, p.text, p.accent))
                .child(highlighted(&r.subtitle, q, UI, p.faint, p.accent));
        }
        RowKind::Other { .. } => {
            main = main
                .child(icon(&ic::FOLDER, 16.0).color(p.icon_muted))
                .child(text(r.title.clone()).font(UI_MED).color(p.text))
                .child(text(r.subtitle.clone()).font(UI).color(p.faint));
        }
        RowKind::Add => {
            main = main
                .child(icon(&ic::PLUS, 16.0).color(p.icon_muted))
                .child(text(r.title.clone()).font(UI_MED).color(p.text));
        }
    }

    let mut line = row()
        .key(format!("psw-{}", r.key))
        .role("option")
        .label(r.title.clone())
        .w_full()
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .fill(if active { p.row_active } else { Color::TRANSPARENT })
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                tx.instant();
                s.project_switcher.activate(hover.clone());
            }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.project_switcher_choose(&key, tx))
        .child(main);

    match &r.kind {
        RowKind::Add => {
            line = line.child(spacer()).child(text(crate::keymap::Bind::OpenProject.label()).font(UI).color(p.faint));
        }
        RowKind::Other { .. } => {
            line = line.child(spacer()).child(icon(&ic::CHEVRON_RIGHT, 16.0).color(p.icon_muted));
        }
        RowKind::Project { id, drillable: true, .. } => {
            let pid = id.clone();
            line = line.child(spacer()).child(
                el().key(format!("psw-drill-{pid}"))
                    .role("button")
                    .label("Worktrees")
                    .size(22.0, 22.0)
                    .radius(4.0)
                    .center()
                    .hover(p.hover)
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.drill_project_worktrees(&pid, tx))
                    .child(icon(&ic::CHEVRON_RIGHT, 16.0).color(p.icon_muted)),
            );
        }
        _ => {}
    }
    line
}

pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let [win_w, win_h] = cx
        .layout_size("window")
        .or_else(|| cx.layout_size("zoomed"))
        .unwrap_or([1280.0 / s.zoom.max(1e-3), 820.0 / s.zoom.max(1e-3)]);
    let w = (win_w - 24.0).min(480.0);
    let max_h = (win_h - 96.0).min(420.0).max(200.0);
    let q = s.project_switcher.query();
    let rows = s.project_switcher_rows();
    let active = s.project_switcher_active(&rows);

    let placeholder = match &s.project_switcher.mode {
        StepMode::Worktrees(_) => "Search worktrees",
        _ => "Search projects",
    };
    let step_title = match &s.project_switcher.mode {
        StepMode::Projects => None,
        StepMode::Others => Some("Other projects"),
        StepMode::Worktrees(_) => Some("Run session in"),
    };

    let search = row()
        .key("project-switcher-field")
        .h(34.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.search_bg)
        .hover(p.layer2)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            text_input(&s.project_switcher.search, UI.lh(20.0))
                .key("project-switcher-input")
                .grow(1.0)
                .shrink()
                .h(20.0)
                .color(p.text)
                .placeholder(placeholder, p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.project_switcher.search)
                .on_input_key(|s: &mut App, ev, tx| s.project_switcher_key(ev, tx) || s.shortcut(ev, tx)),
        );

    let mut list = col().key("project-switcher-results").w_full().px(6.0).pt(4.0).pb(6.0).gap(1.0);
    let mut content_h = 10.0_f32;
    let mut footer: Option<El> = None;

    if let Some(title) = step_title {
        list = list.child(
            row()
                .key("project-switcher-back")
                .role("button")
                .label("Back to projects")
                .h(28.0)
                .px(8.0)
                .gap(4.0)
                .center_y()
                .radius(4.0)
                .hover(p.hover)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.project_switcher_back(tx))
                .child(text(title).font(UI).color(p.muted)),
        );
        content_h += 29.0;
    }

    let mut shown_other_heading = false;
    let mut main_rows = 0usize;
    for (idx, r) in rows.iter().enumerate() {
        if r.kind == RowKind::Add {
            footer = Some(draw_row(s, p, r, active.as_deref() == Some(r.key.as_str()), q));
            continue;
        }
        if !q.is_empty()
            && s.project_switcher.mode == StepMode::Projects
            && r.section == "Other projects"
            && !shown_other_heading
        {
            shown_other_heading = true;
            list = list.child(
                row().key("psw-other-heading").h(26.0).px(10.0).center_y().child(text("Other projects").font(UI).color(p.muted)),
            );
            content_h += 27.0;
        }
        if idx == 1 && matches!(r.kind, RowKind::Worktree { main: false, .. }) {
            list = list.child(el().key("psw-wt-divider").h(0.5).w_full().my(3.0).fill(p.border_muted));
            content_h += 7.0;
        }
        list = list.child(draw_row(s, p, r, active.as_deref() == Some(r.key.as_str()), q));
        content_h += 33.0;
        main_rows += 1;
    }

    if main_rows == 0 && footer.is_none() {
        list = list.child(
            col().key("project-switcher-empty").w_full().h(72.0).center().child(text("No matching projects").font(UI).color(p.muted)),
        );
        content_h += 72.0;
    }

    let footer_h = if footer.is_some() { 45.0 } else { 0.0 };
    let scroll_max = (max_h - 48.0 - footer_h).max(96.0);
    let scroll_h = content_h.min(scroll_max);
    let total_h = 46.5 + scroll_h + footer_h;

    let mut dialog = col()
        .key("project-switcher")
        .role("dialog")
        .label("Search projects")
        .w(w)
        .h(total_h)
        .radius(12.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(8.0, 16.0, p.shadow_far)
        .shadow(4.0, 8.0, p.shadow)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| s.close_project_switcher(true))
        .child(col().key("project-switcher-search").w_full().padding(6.0).child(search))
        .child(el().key("project-switcher-divider").h(0.5).w_full().fill(p.border))
        .child(
            col()
                .key("project-switcher-scroll")
                .role("listbox")
                .w_full()
                .h(scroll_h)
                .scroll(ScrollSpec { stick_bottom: false })
                .clip_fade(0.0, 0.0, 0.0, 16.0)
                .child(list),
        );

    if let Some(add_row) = footer {
        dialog = dialog
            .child(el().key("project-switcher-add-divider").h(0.5).w_full().fill(p.border_muted))
            .child(col().key("project-switcher-footer").w_full().px(6.0).py(6.0).child(add_row));
    }

    let y = ((win_h - total_h) * 0.38).max(32.0);
    let scrim = el().key("project-switcher-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog.absolute((win_w - w) / 2.0, y)]
}
