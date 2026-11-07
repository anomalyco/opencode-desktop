//! A new session in a new worktree, as Desktop's new-session page makes one: New worktree in
//! the location dialog's worktree step (`new-session/location/dialog.tsx`), the branch it
//! starts from beside it (`new-session/workspace/selector.tsx`), and on Send the worktree
//! (`POST /api/worktree` from the project's checkout, `workspaces/create.ts`) and then the
//! session in it (`new-session/composer-adapter.ts`).

use std::collections::HashMap;

use opencode_kit::client::{Prompt, Request, Write};
use opencode_kit::model::{project_git, project_sandboxes};
use opencode_kit::path;
use rune::{Align, Color, El, Key, KeyEvent, ScrollSpec, Side, TextEdit, Transition, Tx, col, el, icon, row, text, text_input};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use crate::palette::{Elevate as _, Pal};
use crate::view::UI;
use crate::{App, Route, Source};

#[derive(Default)]
pub struct NewWorktree {
    /// Drafts that start in a new worktree, by tab id, with the branch picked to start it from
    /// (`None`: the checkout's current branch).
    drafts: HashMap<String, Option<String>>,
    /// Each project's worktree inventory (`/api/worktree`), its own checkout left out.
    listed: HashMap<String, Vec<String>>,
    pub picker: BranchPicker,
}

/// The branch a new worktree starts from: Desktop's `Typeahead` in a popover under the
/// `from <branch>` trigger.
#[derive(Default)]
pub struct BranchPicker {
    pub open: bool,
    pub search: TextEdit,
    /// The checkout whose branches it lists.
    directory: String,
    /// What the server listed, and the search it listed them for.
    listed: (String, Vec<String>),
    failed: bool,
    active: Option<String>,
}

const TRIGGER: &str = "location-branch";

impl App {
    /// The open draft starts its session in a new worktree.
    pub fn draft_creates(&self) -> bool {
        matches!(&self.route, Route::Draft(id) if self.new_worktree.drafts.contains_key(id))
    }

    /// The branch the open draft's new worktree starts from, once one is picked.
    fn draft_base(&self) -> Option<&str> {
        let Route::Draft(id) = &self.route else { return None };
        self.new_worktree.drafts.get(id)?.as_deref()
    }

    /// New worktree in the location dialog: the session will start in a new worktree of
    /// `project`, from its checkout's current branch.
    pub fn choose_new_worktree(&mut self, project: &str, directory: String) {
        self.set_draft_dir(directory);
        crate::view::draft_location::mark_used(self);
        if let Route::Draft(id) = &self.route {
            self.new_worktree.drafts.insert(id.clone(), None);
        }
        self.touch_project(project);
    }

    /// The open draft goes back to starting where its folder is.
    pub(crate) fn forget_new_worktree(&mut self) {
        if let Route::Draft(id) = &self.route {
            self.new_worktree.drafts.remove(id);
        }
    }

    /// The write that starts session `session` from the open draft: in a new worktree of its
    /// project when the draft says so, else in its folder.
    pub(crate) fn start_write(&self, session: String, prompt: Prompt) -> Write {
        match self.draft_project().filter(|_| self.draft_creates()) {
            Some(project) => Write::StartInNewWorktree {
                session,
                project: project.id.clone(),
                from: project.directory.clone(),
                branch: self.draft_base().map(str::to_owned),
                prompt,
            },
            None => Write::StartAs { session, directory: self.draft_dir().to_owned(), prompt },
        }
    }

    /// Every folder other than its checkout that a session of `project` can run in: the
    /// server's inventory, then the project's sandboxes (`workspaceSelectionDirectories`).
    pub fn project_worktrees(&self, project: &str, root: &str) -> Vec<String> {
        let listed = self.new_worktree.listed.get(project).cloned().unwrap_or_default();
        let mut out: Vec<String> = Vec::new();
        for dir in listed.into_iter().chain(project_sandboxes(project)) {
            if path::trim(&dir) != path::trim(root) && !out.iter().any(|d| path::trim(d) == path::trim(&dir)) {
                out.push(dir);
            }
        }
        out
    }

    /// Asks the server for `project`'s worktrees (the worktree step opening, or one changing).
    pub fn list_worktrees(&self, project: &str) {
        let Some(root) = self.project(project).map(|p| p.directory.clone()) else { return };
        if let Source::Live { requests: Some(r), .. } = &self.source {
            let _ = r.send(Request::Worktrees { project: project.to_owned(), root });
        }
    }

    /// Asks for the worktrees of each Git project not listed yet, so a folder in one resolves
    /// to its project (`project_at`), as Desktop's location lookup does.
    pub(crate) fn list_unlisted_worktrees(&self) {
        for p in self.projects.iter().filter(|p| project_git(&p.id) && !self.new_worktree.listed.contains_key(&p.id)) {
            self.list_worktrees(&p.id);
        }
    }

    /// `worktree.listed`, `worktree.updated` and `vcs.branch.listed`.
    pub(crate) fn apply_worktree_event(&mut self, ty: &str, data: &Value) -> bool {
        let project = data["projectID"].as_str().unwrap_or_default();
        match ty {
            "worktree.listed" => {
                let dirs = data["directories"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                self.new_worktree.listed.insert(project.to_owned(), dirs);
                true
            }
            "worktree.updated" => {
                self.list_worktrees(project);
                if let Source::Live { requests: Some(r), .. } = &self.source {
                    let _ = r.send(Request::Projects);
                }
                false
            }
            "vcs.branch.listed" => {
                let picker = &mut self.new_worktree.picker;
                if data["directory"].as_str() != Some(picker.directory.as_str()) || data["search"].as_str() != Some(picker.search.text.as_str()) {
                    return false;
                }
                picker.failed = data["failed"].as_bool() == Some(true);
                let branches = data["branches"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                picker.listed = (picker.search.text.clone(), branches);
                picker.open
            }
            _ => false,
        }
    }

    /// Opens the branch picker on the open draft's checkout.
    pub fn open_branch_picker(&mut self, tx: &mut Tx) {
        tx.instant();
        let directory = self.draft_dir().to_owned();
        let picker = &mut self.new_worktree.picker;
        picker.open = true;
        picker.search = TextEdit::new("");
        picker.search.focused = true;
        picker.failed = false;
        picker.active = None;
        if picker.directory != directory {
            picker.listed = (String::new(), Vec::new());
            picker.directory = directory;
        }
        self.composer.focused = false;
        self.ask_branches();
    }

    pub fn close_branch_picker(&mut self, tx: &mut Tx) {
        tx.instant();
        let picker = &mut self.new_worktree.picker;
        picker.open = false;
        picker.search.focused = false;
        if self.stage.is_none() {
            self.composer.focused = true;
        }
    }

    fn ask_branches(&self) {
        let picker = &self.new_worktree.picker;
        if let Source::Live { requests: Some(r), .. } = &self.source {
            let _ = r.send(Request::Branches { directory: picker.directory.clone(), search: picker.search.text.clone() });
        }
    }

    /// The branches shown: the latest listing, narrowed by what's typed since (`match`).
    fn branch_rows(&self) -> Vec<String> {
        let picker = &self.new_worktree.picker;
        let q = picker.search.text.to_lowercase();
        picker.listed.1.iter().filter(|b| b.to_lowercase().contains(&q)).cloned().collect()
    }

    fn active_branch(&self, rows: &[String]) -> Option<String> {
        let picker = &self.new_worktree.picker;
        picker.active.clone().filter(|a| rows.contains(a)).or_else(|| rows.first().cloned())
    }

    fn choose_branch(&mut self, branch: String, tx: &mut Tx) {
        if let Route::Draft(id) = &self.route {
            self.new_worktree.drafts.insert(id.clone(), Some(branch));
        }
        self.close_branch_picker(tx);
    }

    fn branch_picker_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let rows = self.branch_rows();
        let at = self.active_branch(&rows).and_then(|a| rows.iter().position(|r| *r == a));
        let step = |d: isize| at.map_or(0, |i| (i as isize + d).rem_euclid(rows.len().max(1) as isize) as usize);
        match ev.key {
            Key::Escape => self.close_branch_picker(tx),
            Key::Down | Key::Up if !rows.is_empty() => {
                tx.instant();
                let i = step(if ev.key == Key::Down { 1 } else { -1 });
                tx.scroll_into_view_in("branch-picker-scroll", format!("branch-{}", rows[i]));
                self.new_worktree.picker.active = Some(rows[i].clone());
            }
            Key::Enter => {
                if let Some(i) = at {
                    self.choose_branch(rows[i].clone(), tx);
                }
            }
            _ => return false,
        }
        true
    }

    fn branch_search_changed(&mut self, tx: &mut Tx) {
        tx.instant();
        self.new_worktree.picker.active = None;
        self.ask_branches();
    }
}

/// The branch beside a new worktree (`selector.tsx:159-176`): `from <branch>` after the
/// branch-out glyph and before a small chevron, muted until the pointer is over it or its
/// picker is open, its 4 pt corners lit with the hover overlay (the pressed one while open).
pub fn trigger(s: &App, p: &Pal, current: &str) -> El {
    let open = s.new_worktree.picker.open;
    let branch = s.draft_base().unwrap_or(current);
    row()
        .key(TRIGGER)
        .role("button")
        .label("Choose branch")
        .hover_group()
        .h(28.0)
        .max_w(220.0)
        .shrink()
        .min_w(0.0)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .fill(if open { p.pressed } else { Color::TRANSPARENT })
        .hover(p.hover)
        .on_press_tx(|s: &mut App, tx: &mut Tx| if s.new_worktree.picker.open { s.close_branch_picker(tx) } else { s.open_branch_picker(tx) })
        .child(icon(&ic::BRANCH_OUT, 16.0).color(p.icon_muted))
        .child(text(format!("from {branch}")).font(UI).color(if open { p.text } else { p.muted }).group_color(p.text).truncate_fade())
        .child(icon(&ic::CHEVRON_DOWN, 14.0).ml(-2.0).color(p.icon_muted))
}

/// The open picker under its trigger, and the layer that closes it.
pub fn picker(s: &App, p: &Pal) -> Option<[El; 2]> {
    if !s.new_worktree.picker.open || !matches!(s.route, Route::Draft(_)) {
        return None;
    }
    let picker = &s.new_worktree.picker;
    let rows = s.branch_rows();
    let active = s.active_branch(&rows);
    let selected = s.draft_base().map(str::to_owned).or_else(|| s.sidebar_inbox.branches.get(s.draft_dir()).map(str::to_owned));
    // The search row: 32 tall, 12 in and 8 from the end, the glyph 8 before the field.
    let search = row()
        .key("branch-picker-search")
        .h(32.0)
        .pl(12.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted))
        .child(
            text_input(&picker.search, UI)
                .key("branch-picker-input")
                .single_line()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .color(p.text)
                .placeholder("Search branches", p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.new_worktree.picker.search)
                .on_input_change(App::branch_search_changed)
                .on_input_key(|s: &mut App, ev, tx| s.branch_picker_key(ev, tx)),
        )
        .children((!picker.search.text.is_empty()).then(|| {
            el().key("branch-picker-clear")
                .role("button")
                .label("Clear filter")
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    s.new_worktree.picker.search = TextEdit::new("");
                    s.new_worktree.picker.search.focused = true;
                    s.branch_search_changed(tx);
                })
                .child(icon(&ic::CIRCLE_XMARK, 14.0).color(p.icon_faint))
        }));
    let mut list = col().key("branch-picker-rows").w_full().padding(4.0);
    let list_h = 8.0 + if rows.is_empty() { 36.0 } else { 28.0 * rows.len() as f32 };
    if rows.is_empty() {
        let words = if picker.failed { "Couldn't load branches" } else { "No results found" };
        list = list.child(row().key("branch-picker-empty").h(36.0).mx(4.0).px(8.0).py(4.0).center_y().child(text(words).font(UI).color(p.faint)));
    }
    for b in &rows {
        let (pick, hover) = (b.clone(), b.clone());
        list = list.child(
            row()
                .key(format!("branch-{b}"))
                .role("option")
                .label(b.clone())
                .w_full()
                .h(28.0)
                .px(8.0)
                .gap(8.0)
                .center_y()
                .radius(4.0)
                .fill(if active.as_deref() == Some(b.as_str()) { p.hover } else { Color::TRANSPARENT })
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                    if inside {
                        tx.instant();
                        s.new_worktree.picker.active = Some(hover.clone());
                    }
                })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.choose_branch(pick.clone(), tx))
                .child(text(b.clone()).font(UI).color(p.text).truncate_fade().shrink().min_w(0.0))
                .child(rune::spacer())
                .children((selected.as_deref() == Some(b.as_str())).then(|| icon(&ic::CHECK, 14.0).color(p.icon_muted))),
        );
    }
    // `w-[243px] rounded-lg bg-layer-01`, `--v2-elevation-floating`, 4 pt under the trigger.
    let surface = col()
        .key("branch-picker")
        .role("dialog")
        .label("Choose branch")
        .w(243.0)
        .radius(8.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(rune::Spring::NONE)
        .on_press(|_: &mut App| {})
        .child(search)
        .child(el().key("branch-picker-divider").h(0.5).w_full().fill(p.border))
        .child(col().key("branch-picker-scroll").w_full().h(list_h.min(224.0)).scroll(ScrollSpec { stick_bottom: false }).child(list));
    let scrim = el()
        .key("branch-picker-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .transition(Transition::none())
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.close_branch_picker(tx));
    Some([scrim, surface.anchored_fit(TRIGGER.to_owned(), Side::Bottom, Align::Start, 4.0)])
}
