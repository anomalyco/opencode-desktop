//! ⇧⌘O ("Search projects"), Desktop's project and worktree switcher (`new-session/location/dialog.tsx`),
//! which the new-session page's project and worktree buttons open too.
//!
//! Lists this client's recent/used projects first, then any other server projects (`Other projects`),
//! a Git project's worktrees behind its chevron (its checkout, New worktree, then the server's
//! inventory), and `+ Add project` (`⌘O`) last. Selecting a project updates the open draft's
//! folder on a New Session page, or opens a new session in that project from anywhere else.

use crate::pickers::PickerHost as _;
use opencode_kit::model::{Project, project_git};
use rune::{Color, Cx, El, Key, KeyEvent, ScrollSpec, Span, Spring, TextEdit, Transition, Tx, col, el, icon, row, text, text_input};
use rune_theme_opencode::icons as ic;

use crate::mentions::list::Highlight;
use crate::palette::{Elevate as _, Pal};
use crate::projects::folder_name;
use crate::switcher::{move_for, target};
use crate::view::{UI, UI_MED};
use crate::{App, Route};
use opencode_i18n::t;

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
    /// New worktree: the session starts in a worktree made from the project's checkout.
    Create { project_id: String, directory: String },
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
    let t = text(s.to_owned()).font(style).color(color).truncate();
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
                let (id, root) = (project.id.clone(), project.directory.clone());
                // The checkout and New worktree, then the worktrees (`dialog.tsx:156-173`).
                let main = SwitcherRow {
                    key: format!("wt:main:{id}"),
                    title: project.name.clone(),
                    subtitle: root.clone(),
                    section: "Worktrees",
                    kind: RowKind::Worktree { project_id: id.clone(), directory: root.clone(), main: true },
                };
                let create = SwitcherRow {
                    key: format!("wt:create:{id}"),
                    title: t!("workspace.new").into(),
                    subtitle: t!("session.new.workspace.new.tooltip").into(),
                    section: "Worktrees",
                    kind: RowKind::Create { project_id: id.clone(), directory: root.clone() },
                };
                let worktrees = self.project_worktrees(&id, &root).into_iter().map(|dir| SwitcherRow {
                    key: format!("wt:{dir}"),
                    title: folder_name(&dir),
                    subtitle: dir.clone(),
                    section: "Worktrees",
                    kind: RowKind::Worktree { project_id: id.clone(), directory: dir, main: false },
                });
                [main, create].into_iter().chain(worktrees).filter(|r| matches_query(q, &[&r.title, &r.subtitle])).collect()
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
        self.project_switcher.search.cancel_composition();
        self.project_switcher.mode = StepMode::Projects;
        self.project_switcher.back = None;
        if restore && self.project_switcher.restore_composer && self.stage.is_none() {
            self.composer.focused = true;
        }
    }

    /// Steps into `project_id`'s worktrees (a Git project's), asking the server for them.
    pub fn drill_project_worktrees(&mut self, project_id: &str, tx: &mut Tx) {
        if !drillable(self, project_id) {
            return;
        }
        tx.instant();
        self.list_worktrees(project_id);
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
        let worktree = matches!(row.kind, RowKind::Worktree { main: false, .. });
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
            RowKind::Create { project_id, directory } => {
                tx.instant();
                self.close_project_switcher(false);
                if !matches!(self.route, Route::Draft(_)) {
                    self.open_draft_tab();
                }
                self.choose_new_worktree(&project_id, directory);
                self.sort_projects();
                if self.stage.is_none() {
                    self.composer.focused = true;
                }
            }
            RowKind::Project { id, directory, .. } | RowKind::Worktree { project_id: id, directory, .. } => {
                tx.instant();
                if worktree {
                    self.note_existing_worktree(&id, directory.clone());
                }
                self.touch_project(&id);
                self.sort_projects();
                self.close_project_switcher(false);
                if matches!(self.route, Route::Draft(_)) {
                    self.set_draft_dir(directory);
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
        if self.project_switcher.search.is_composing() {
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

fn project_row_item(p: &Project, used: bool) -> SwitcherRow {
    SwitcherRow {
        key: format!("project:{}", p.id),
        title: p.name.clone(),
        subtitle: p.directory.clone(),
        section: if used { "Projects" } else { "Other projects" },
        kind: RowKind::Project { id: p.id.clone(), directory: p.directory.clone(), used, drillable: project_git(&p.id) },
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
    let title_ink = if option { p.muted } else { p.text };
    let title = highlighted(&r.title, q, TITLE, title_ink, p.accent);
    let description = |t: &str| highlighted(t, q, UI, p.faint, p.accent).grow(1.0).shrink().min_w(0.0);
    // The name keeps up to 55% of its text (a worktree's title 60%) while the folder gives way:
    // the row less its insets, glyph and chevron.
    let text_w = w - 12.0 - 12.0 - 24.0 - if matches!(r.kind, RowKind::Project { drillable: true, .. }) { 24.0 } else { 0.0 };
    let named = |t: El, share: f32| row().key("name").max_w(text_w * share).center_y().child(t.shrink().min_w(0.0));
    let main = row().key("main").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y();
    let main = match &r.kind {
        RowKind::Project { id, .. } | RowKind::Worktree { project_id: id, main: true, .. } => main
            .child(crate::icons::badge(s, p, s.project(id), &r.title))
            .child(row().key("text").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().child(named(title, 0.55)).child(description(&r.subtitle))),
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
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                tx.instant();
                s.project_switcher.activate(hover.clone());
            }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.project_switcher_choose(&key, tx))
        .child(main);
    match &r.kind {
        RowKind::Add => line.child(crate::keymap::keycaps(p, crate::keymap::Bind::OpenProject)),
        RowKind::Other { .. } => line.child(drill(p, "psw-drill-other", t!("session.new.location.otherProjects"), |s: &mut App, tx: &mut Tx| s.project_switcher_choose("other", tx))),
        RowKind::Project { id, drillable: true, .. } => {
            let pid = id.clone();
            line.child(drill(p, &format!("psw-drill-{id}"), t!("session.new.location.worktrees"), move |s: &mut App, tx: &mut Tx| s.drill_project_worktrees(&pid, tx)))
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

/// The dialog (`dialog.css:5-60`): 480 wide on bg-base with `--v2-elevation-overlay` and 12 pt
/// corners, as tall as it needs up to 480, its top halfway down a 480-tall box but at least 48
/// from the window's. The search field (28 tall, transparent, the placeholder in the base ink at
/// half strength), a hairline 8 below it, and the rows 4 apart under it.
pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let [win_w, win_h] = cx
        .layout_size("window")
        .or_else(|| cx.layout_size("zoomed"))
        .unwrap_or([1280.0 / s.zoom.max(1e-3), 820.0 / s.zoom.max(1e-3)]);
    let w = (win_w - 24.0).min(480.0);
    let max_h = (win_h - 96.0).min(480.0);
    let q = s.project_switcher.query();
    let rows = s.project_switcher_rows();
    let active = s.project_switcher_active(&rows);

    let (placeholder, step_title) = match &s.project_switcher.mode {
        StepMode::Projects => (t!("session.new.project.search"), None),
        StepMode::Others => (t!("session.new.project.search"), Some(t!("session.new.location.otherProjects"))),
        StepMode::Worktrees(_) => (t!("session.new.workspace.search.placeholder"), Some(t!("session.new.workspace.runIn"))),
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
            text_input(&s.project_switcher.search, UI)
                .key("project-switcher-input")
                .single_line()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .color(p.text)
                .placeholder(placeholder, p.text.alpha(p.text.a * 0.5))
                .caret(p.text, p.selection).selection_style(p.field_selection)
                .on_edit(|s: &mut App| &mut s.project_switcher.search)
                .on_input_key(|s: &mut App, ev, tx| s.project_switcher_key(ev, tx) || s.shortcut(ev, tx)),
        );

    let mut list = col().key("project-switcher-results").w_full().px(6.0).pb(6.0).gap(4.0);
    let mut content_h = 6.0_f32;
    if let Some(title) = step_title {
        list = list.child(
            row()
                .key("project-switcher-back")
                .role("button")
                .label(t!("session.new.location.back"))
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
        list = list.child(col().key("project-switcher-empty").w_full().h(120.0).center().child(text(t!("palette.empty")).font(UI).color(p.muted)));
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
            col()
                .key("project-switcher-scroll")
                .role("listbox")
                .w_full()
                .h(scroll_h)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        );
    let y = ((win_h - 480.0) / 2.0).max(48.0);
    let scrim = el().key("project-switcher-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog.absolute((win_w - w) / 2.0, y)]
}
