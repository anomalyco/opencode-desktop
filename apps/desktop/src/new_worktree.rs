//! A new session in a new worktree, as Desktop's new-session page makes one: New worktree in
//! the location dialog's worktree step (`new-session/location/dialog.tsx`), the branch it
//! starts from beside it (`new-session/workspace/selector.tsx`), and on Send the worktree
//! (`POST /api/worktree` from the project's checkout, `workspaces/create.ts`) and then the
//! session in it (`new-session/composer-adapter.ts`).

use std::collections::HashMap;

use crate::popover::Highlight;

use opencode_kit::client::{Prompt, Request, Write};
use opencode_kit::model::{project_git, project_sandboxes};
use opencode_kit::path;
use rune::{Align, Color, El, Key, KeyEvent, ScrollSpec, Side, TextEdit, Transition, Tx, col, el, icon, row, text, text_input};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use crate::keymap::Bind;
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
    /// The existing worktree a draft of each project last ran in, where Cycle session location
    /// goes after New worktree (`state.existing` in `workspace/controller.ts`).
    existing: HashMap<String, String>,
    pub picker: BranchPicker,
}

/// Where the open draft's session starts, as `cycleNewSessionWorktree` steps through it.
#[derive(Clone, Debug, PartialEq)]
enum Place {
    /// The project's own checkout (`main`).
    Local,
    /// A new worktree (`create`).
    New,
    Existing(String),
}

mod typeahead;

/// The branch a new worktree starts from: Desktop's `Typeahead` in a popover under the
/// `from <branch>` trigger (`new-session/workspace/branch-machine.ts`, `selector.tsx`). Every
/// open revalidates the checkout's list while the last one stays on screen; each typed query
/// asks the server (`GET /api/vcs/branch`, 50 at most), so a branch past the first 50 is found
/// by typing ([`typeahead`]). Escape or a pick hands focus back to the composer; a press
/// outside leaves it where it landed.
#[derive(Default)]
pub struct BranchPicker {
    popover: Popover,
    pub search: TextEdit,
    /// The checkout whose branches it lists.
    directory: String,
    /// The checkout's whole list as last listed (`initial`), for the next open.
    listed: Option<Vec<String>>,
    typeahead: typeahead::Typeahead,
    /// The lit row: the first until one's chosen, held while it's listed (`controller.ts`).
    active: Highlight,
}

/// `BranchPickerState`'s `popover`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Popover {
    #[default]
    Closed,
    Open,
}

/// Why the picker closed: Escape and a pick give the composer focus (`focus.composer`), a press
/// outside doesn't (`popover: "outside"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Close {
    Keys,
    Outside,
}

impl BranchPicker {
    pub fn is_open(&self) -> bool {
        self.popover == Popover::Open
    }

    /// The rows, and whether they are stale (dimmed while a query the rows don't match waits).
    fn rows(&self) -> (Vec<String>, bool) {
        (self.typeahead.shown.clone(), self.typeahead.stale)
    }
}

const TRIGGER: &str = "location-branch";

impl App {
    /// The open draft starts its session in a new worktree.
    pub fn draft_creates(&self) -> bool {
        matches!(&self.route, Route::Draft(id) if self.new_worktree.drafts.contains_key(id))
    }

    /// The branch the open draft's new worktree starts from, once one is picked.
    pub(crate) fn draft_base(&self) -> Option<&str> {
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
        let dir = self.draft_dir().to_owned();
        self.note_destination(&dir, "workspace");
        self.touch_project(project);
    }

    /// The new-session page's location keys, while a draft is open and no dialog is:
    /// Choose base branch (`selector.tsx`, a new worktree's), Choose worktree (`view.tsx`)
    /// and Cycle session location (`new-session/commands.tsx`), each only where Desktop
    /// enables it.
    pub(crate) fn draft_location_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !matches!(self.route, Route::Draft(_)) || self.settings.open || self.project_switcher.open {
            return false;
        }
        if Bind::BranchChoose.matches_tx(ev, tx) {
            if !self.draft_creates() || self.sidebar_inbox.branches.get(self.draft_dir()).is_none() {
                return false;
            }
            if self.new_worktree.picker.is_open() { self.close_branch_picker(tx) } else { self.open_branch_picker(tx) }
            return true;
        }
        let Some(project) = self.draft_project().filter(|p| project_git(&p.id)).map(|p| (p.id.clone(), p.directory.clone())) else {
            return false;
        };
        if Bind::LocationSelect.matches_tx(ev, tx) {
            tx.instant();
            self.close_branch_picker(tx);
            self.open_project_worktrees(&project.0, tx);
            return true;
        }
        if Bind::LocationCycle.matches_tx(ev, tx) {
            tx.instant();
            self.cycle_location(&project.0, project.1);
            return true;
        }
        false
    }

    /// Local, then New worktree, then the worktree last used if the inventory still has it,
    /// then Local again (`cycleNewSessionWorktree`).
    fn cycle_location(&mut self, project: &str, root: String) {
        let place = self.draft_place(project, &root);
        if let Place::Existing(dir) = &place {
            self.note_existing_worktree(project, dir.clone());
        }
        let existing = self
            .new_worktree
            .existing
            .get(project)
            .filter(|d| self.project_worktrees(project, &root).iter().any(|w| path::trim(w) == path::trim(d)))
            .cloned();
        // A new target closes the picker (`retarget`).
        if self.new_worktree.picker.is_open() {
            self.new_worktree.picker.popover = Popover::Closed;
            self.new_worktree.picker.search.focused = false;
        }
        match (place, existing) {
            (Place::Local, _) => self.choose_new_worktree(project, root),
            (Place::New, Some(dir)) => self.set_draft_dir(dir),
            (Place::New, None) | (Place::Existing(_), _) => self.set_draft_dir(root),
        }
    }

    fn draft_place(&self, project: &str, root: &str) -> Place {
        let dir = self.draft_dir();
        if self.draft_creates() {
            return Place::New;
        }
        if self.project_worktrees(project, root).iter().any(|w| path::trim(w) == path::trim(dir)) {
            return Place::Existing(dir.to_owned());
        }
        Place::Local
    }

    /// A draft of `project` runs in its existing worktree `dir`: Cycle session location comes
    /// back to it after New worktree.
    pub(crate) fn note_existing_worktree(&mut self, project: &str, dir: String) {
        self.new_worktree.existing.insert(project.to_owned(), dir);
    }

    /// Remembers where a session of the project at `dir` last started (`workspaces.lastUsed`).
    pub(crate) fn note_destination(&mut self, dir: &str, place: &str) {
        let Some(id) = self.project_at(dir).map(|p| p.id.clone()) else { return };
        if self.settings.last_used.get(&id).map(String::as_str) != Some(place) {
            self.change_settings(|s| {
                s.last_used.insert(id, place.to_owned());
            });
        }
    }

    /// A new draft starts where Settings › Default environment says: its folder, a new
    /// worktree, or wherever its project's last session started.
    pub(crate) fn apply_default_destination(&mut self) {
        let Route::Draft(id) = self.route.clone() else { return };
        let Some(project) = self.project_at(self.draft_dir()) else { return };
        let new = match self.settings.default_destination {
            crate::settings::Destination::New => true,
            crate::settings::Destination::Local => false,
            crate::settings::Destination::LastUsed => self.settings.last_used.get(&project.id).is_some_and(|p| p == "workspace"),
        };
        if new {
            self.new_worktree.drafts.insert(id, None);
        }
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

    /// Membership doesn't need the picker's owned, deduplicated list.
    pub(crate) fn has_worktree_at(&self, project: &crate::Project, dir: &str) -> bool {
        let root = path::trim(&project.directory);
        let matches = |s: &String| path::trim(s.trim()) == dir && path::trim(s) != root;
        self.new_worktree.listed.get(&project.id).is_some_and(|dirs| dirs.iter().any(matches))
            || project_sandboxes(&project.id).iter().any(matches)
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
                let search = data["search"].as_str().unwrap_or_default();
                if data["directory"].as_str() != Some(picker.directory.as_str()) {
                    return false;
                }
                let failed = data["failed"].as_bool() == Some(true);
                let branches: Option<Vec<String>> = (!failed).then(|| data["branches"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect());
                picker.typeahead.step(typeahead::Event::Answered { query: search, items: branches.clone() });
                // The checkout's whole list: what the next open starts on, and what an open
                // picker on the empty query shows once it's revalidated (a failed one keeps the last).
                if search.is_empty() && let Some(list) = branches {
                    picker.typeahead.step(typeahead::Event::Synced(&list));
                    picker.listed = Some(list);
                }
                let keys: Vec<&str> = picker.typeahead.shown.iter().map(String::as_str).collect();
                picker.active.step(crate::popover::HighlightEvent::Rows(&keys));
                picker.is_open()
            }
            _ => false,
        }
    }

    /// Opens the branch picker on the open draft's checkout.
    pub fn open_branch_picker(&mut self, tx: &mut Tx) {
        tx.instant();
        let directory = self.draft_dir().to_owned();
        let picker = &mut self.new_worktree.picker;
        // A new checkout drops the last one's list (`retarget`).
        if picker.directory != directory {
            picker.listed = None;
            picker.directory = directory;
        }
        picker.popover = Popover::Open;
        picker.search = TextEdit::new("");
        picker.search.focused = true;
        picker.active.clear();
        let initial = picker.listed.clone();
        let effects = picker.typeahead.step(typeahead::Event::Opened { initial: initial.as_deref(), failed: false, now: tx.clock() });
        self.composer.focused = false;
        self.branch_effects(effects, tx);
        // Every open revalidates the list, keeping the last on screen meanwhile (`branch-machine.ts:159-172`).
        if initial.is_some() {
            self.ask_branches(String::new());
        }
    }

    pub fn close_branch_picker(&mut self, tx: &mut Tx) {
        self.close_branch_picker_for(Close::Keys, tx);
    }

    fn close_branch_picker_for(&mut self, why: Close, tx: &mut Tx) {
        tx.instant();
        let picker = &mut self.new_worktree.picker;
        picker.popover = Popover::Closed;
        picker.search.focused = false;
        if why == Close::Keys && self.stage.is_none() {
            self.composer.focused = true;
        }
    }

    /// The typeahead's timers.
    pub(crate) fn poll_branch_picker(&mut self, now: f64, tx: &mut Tx) -> bool {
        if !self.new_worktree.picker.typeahead.pending() {
            return false;
        }
        let before = self.new_worktree.picker.typeahead.phase.clone();
        let effects = self.new_worktree.picker.typeahead.step(typeahead::Event::Tick { now });
        self.branch_effects(effects, tx);
        before != self.new_worktree.picker.typeahead.phase
    }

    fn branch_effects(&mut self, effects: rune::machine::Effects<typeahead::Effect>, tx: &mut Tx) {
        for effect in effects {
            match effect {
                typeahead::Effect::WakeAt(due) => tx.wake_at(due),
                typeahead::Effect::Request(query) => self.ask_branches(query),
            }
        }
    }

    /// Asks the server for the checkout's branches matching `search`.
    fn ask_branches(&mut self, search: String) {
        if let Source::Live { requests: Some(r), .. } = &self.source {
            let _ = r.send(Request::Branches { directory: self.new_worktree.picker.directory.clone(), search });
        }
    }

    fn branch_rows(&self) -> Vec<String> {
        self.new_worktree.picker.rows().0
    }

    fn active_branch(&self, rows: &[String]) -> Option<String> {
        self.new_worktree.picker.active.resolve(rows.iter().map(String::as_str))
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
                self.new_worktree.picker.active.set(rows[i].clone());
            }
            Key::Enter => {
                // The user's choice never rests on a row the pending query hasn't confirmed.
                if let Some(i) = at.filter(|_| !self.new_worktree.picker.typeahead.stale) {
                    self.choose_branch(rows[i].clone(), tx);
                }
            }
            _ => return false,
        }
        true
    }

    fn branch_search_changed(&mut self, tx: &mut Tx) {
        tx.instant();
        let picker = &mut self.new_worktree.picker;
        let input = picker.search.text.clone();
        let effects = picker.typeahead.step(typeahead::Event::Input { input: &input, now: tx.clock() });
        let keys: Vec<&str> = picker.typeahead.shown.iter().map(String::as_str).collect();
        picker.active.step(crate::popover::HighlightEvent::Rows(&keys));
        self.branch_effects(effects, tx);
    }
}

/// The branch beside a new worktree (`selector.tsx:159-176`): `from <branch>` after the
/// branch-out glyph and before a small chevron, muted until the pointer is over it or its
/// picker is open, its 4 pt corners lit with the hover overlay (the pressed one while open).
pub fn trigger(s: &App, p: &Pal, current: &str) -> El {
    let open = s.new_worktree.picker.is_open();
    let branch = s.draft_base().unwrap_or(current);
    row()
        .key(TRIGGER)
        .role("button")
        .label(opencode_i18n::t!("rune.branch.choose"))
        .help(opencode_i18n::t!("command.session.branch.choose")).help_keys(Bind::BranchChoose.keycaps())
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
        .on_press_tx(|s: &mut App, tx: &mut Tx| if s.new_worktree.picker.is_open() { s.close_branch_picker(tx) } else { s.open_branch_picker(tx) })
        .child(icon(&ic::BRANCH_OUT, 16.0).color(p.icon_muted))
        .child(text(format!("from {branch}")).font(UI).color(if open { p.text } else { p.muted }).group_color(p.text).truncate_fade())
        .child(icon(&ic::CHEVRON_DOWN, 14.0).ml(-2.0).color(p.icon_muted))
}

/// The open picker under its trigger, and the layer that closes it.
pub fn picker(s: &App, p: &Pal) -> Option<[El; 2]> {
    if !s.new_worktree.picker.is_open() || !matches!(s.route, Route::Draft(_)) {
        return None;
    }
    let picker = &s.new_worktree.picker;
    let (rows, stale) = picker.rows();
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
                .placeholder(opencode_i18n::t!("session.new.workspace.branch.search.placeholder"), p.faint)
                .caret(p.text, p.selection).selection_style(p.field_selection)
                .on_edit(|s: &mut App| &mut s.new_worktree.picker.search)
                .on_input_change(App::branch_search_changed)
                .on_input_key(|s: &mut App, ev, tx| s.branch_picker_key(ev, tx)),
        )
        .children((!picker.search.text.is_empty()).then(|| {
            el().key("branch-picker-clear")
                .role("button")
                .label(opencode_i18n::t!("ui.list.clearFilter"))
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
        let words = if picker.typeahead.failed() { opencode_i18n::t!("rune.branch.loadFailed") } else { opencode_i18n::t!("palette.empty") };
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
                // Rows the pending query hasn't confirmed (`opacity-50`).
                .opacity(if stale { 0.5 } else { 1.0 })
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                    if inside {
                        tx.instant();
                        s.new_worktree.picker.active.step(crate::popover::HighlightEvent::Pointer(hover.clone()));
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
        .label(opencode_i18n::t!("rune.branch.choose"))
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
        .focusable(false)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.close_branch_picker_for(Close::Outside, tx));
    Some([scrim, surface.anchored_fit(TRIGGER.to_owned(), Side::Bottom, Align::Start, 4.0)])
}
