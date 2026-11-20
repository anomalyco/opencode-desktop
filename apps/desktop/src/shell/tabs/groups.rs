//! Desktop host for `opencode-feature-tab-groups`: the window's tab groups
//! (`packages/app/src/shell/titlebar/tab-groups-ui.tsx`'s `TabGroupsProvider`), kept in the
//! window's `tabs.groups` row, and what changes them. The sidebar draws them
//! (`shell/view/sidebar/tab_groups.rs`).

use std::cell::RefCell;
use std::collections::HashMap;

pub use opencode_feature_tab_groups::model::{self, Color, Doc, Group, Kind, ManualGroup, Meta, Mode};
pub use opencode_feature_tab_groups::{Rename, RenameEffect, RenameEvent, TabGroupsState, drag};

use opencode_kernel::{Feature, KernelBuilder, Slot};
use rune::Tx;

use crate::App;
use crate::tabs::Tab;

pub static TAB_GROUPS: Slot<TabGroupsState> = Slot::new();
/// Where each section of the grouped side tabs was laid out, by its key, in window points.
/// Only the header drag reads it, as it starts, so a layout report redraws nothing.
pub static RECTS: Slot<RefCell<HashMap<String, [f32; 4]>>> = Slot::new();

const ROW: &str = "tabs.groups";

pub struct TabGroupsFeature;

impl Feature<App> for TabGroupsFeature {
    fn id(&self) -> &'static str {
        "feature.tab-groups"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&TAB_GROUPS);
        builder.register_slot(&RECTS);
        builder.schedule(crate::tick::stage::REACT_SHELL, "tab-groups.poll", |s, _, tx| poll(s, tx));
    }
}

pub fn state(s: &App) -> &TabGroupsState {
    s.kernel.slots.get(&TAB_GROUPS)
}

fn state_mut(s: &mut App) -> &mut TabGroupsState {
    s.kernel.slots.get_mut(&TAB_GROUPS)
}

/// Grouped side tabs show: Tab groups on, side tabs in One list (`isGroupedSidebarList`).
pub fn on(s: &App) -> bool {
    s.settings.tab_groups() && s.tab_layout() == crate::layout_state::TabLayout::Vertical && s.settings.sidebar_organize.effective() == crate::settings::Organize::List
}

pub fn servers(s: &App) -> usize {
    crate::servers::registry::list(s).len()
}

/// The window's mode, a lone server's grouping reading as none.
pub fn effective_mode(s: &App) -> Mode {
    model::effective_mode(state(s).doc.mode, servers(s))
}

/// Reads the window's `tabs.groups` (live windows, once the window's rows are read).
pub fn restore(s: &mut App) {
    let ns = opencode_storage::window_namespace(&s.window.id);
    let doc = crate::storage::json(&ns, ROW).map(|v| Doc::from_json(&v)).unwrap_or_default();
    state_mut(s).doc = doc;
}

fn save(s: &App) {
    if s.persists() {
        crate::windows::store::save(&s.window, ROW, &state(s).doc.to_json());
    }
}

/// Changes the saved state and writes it back.
pub fn update(s: &mut App, f: impl FnOnce(&mut Doc)) {
    f(&mut state_mut(s).doc);
    save(s);
}

pub fn tab_key(t: &Tab) -> String {
    t.key().encode()
}

/// `resolvedTabs`: what each tab is to grouping, in the window's order.
pub fn metas(s: &App) -> Vec<Meta> {
    let servers = crate::servers::registry::list(s);
    let label = |key: &opencode_kit::servers::ServerKey| servers.iter().find(|e| e.key() == *key).map_or_else(|| crate::servers::runtime::label(key), |e| crate::servers::registry::label(&e.conn));
    s.tabs
        .iter()
        .map(|t| {
            let face = crate::view::tab_strip::face(s, t);
            let project = face.project.or_else(|| if t.is_draft() { s.project_at(&t.directory) } else { None });
            let dir = (!face.dir.is_empty()).then(|| face.dir.to_owned());
            Meta {
                key: tab_key(t),
                title: face.title,
                server: t.server.as_str().to_owned(),
                server_label: label(&t.server),
                project_id: project.map(|p| p.id.clone()),
                project_name: project.map(|p| p.name.clone()).or_else(|| dir.as_deref().map(folder_name)),
                directory: dir,
            }
        })
        .collect()
}

/// `displayName({ worktree })`: a folder's last part.
fn folder_name(dir: &str) -> String {
    dir.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(dir).to_owned()
}

/// `groups()`; `None` draws the side tabs ungrouped.
pub fn groups(s: &App, items: &[Meta]) -> Option<Vec<Group>> {
    let known: Vec<String> = crate::servers::registry::list(s).iter().map(|e| e.key().as_str().to_owned()).collect();
    let st = state(s);
    model::build(items, &st.doc, known.len(), &known, &st.filter.text, opencode_i18n::t!("sidebar.view.other"))
}

/// `tabs.reorder`: the window's tabs in the order of `keys` (Desktop's tab keys).
pub fn reorder(s: &mut App, keys: &[String]) {
    let rank: HashMap<&str, usize> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let mut tabs: Vec<Tab> = s.tabs.to_vec();
    tabs.sort_by_key(|t| rank.get(tab_key(t).as_str()).copied().unwrap_or(usize::MAX));
    if tabs != *s.tabs {
        *s.tabs = tabs;
        s.save_tabs();
    }
}

/// The tab list changed (a draft promoted, a tab closed or opened): manual membership follows
/// it and a manual group's tabs stay together; opening a tab in a collapsed group opens it.
fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    let keys: Vec<String> = s.tabs.iter().map(tab_key).collect();
    let st = s.kernel.slots.untracked(&TAB_GROUPS);
    if keys != st.seen_tabs {
        let reconciled = model::reconcile(&st.seen_tabs, &keys, &st.doc.manual_assignments);
        let freeform = effective_mode(s) == Mode::None;
        state_mut(s).seen_tabs = keys.clone();
        changed = true;
        if let Some(assignments) = reconciled {
            update(s, |d| {
                d.manual_groups = model::prune_empty(&d.manual_groups, &assignments);
                d.manual_assignments = assignments;
            });
        }
        let doc = &state(s).doc;
        if freeform && !doc.manual_groups.is_empty() {
            let canonical = model::canonical_order(&keys, &doc.manual_assignments);
            if canonical != keys {
                reorder(s, &canonical);
            }
        }
    }
    // The header's name field: armed until it has focus, ended once it loses it.
    let focused = match &s.kernel.slots.untracked(&TAB_GROUPS).rename {
        Rename::Arming { field, .. } => Some(field.focused),
        Rename::Editing { field, .. } if !field.focused => Some(false),
        Rename::Editing { .. } | Rename::Idle => None,
    };
    if let Some(focused) = focused {
        rename_step(s, RenameEvent::Focused(focused), tx);
        changed = true;
    }
    let current = s.current_tab();
    if s.kernel.slots.untracked(&TAB_GROUPS).seen_current != current {
        state_mut(s).seen_current = current.clone();
        expand_current(s, current.as_deref());
        changed = true;
    }
    changed
}

/// Navigating into a session inside a collapsed group opens that group.
fn expand_current(s: &mut App, current: Option<&str>) {
    let Some(at) = current.and_then(|uid| s.tabs.iter().position(|t| t.uid() == uid)) else { return };
    if !s.settings.tab_groups() {
        return;
    }
    let items = metas(s);
    let owner = groups(s, &items).into_iter().flatten().find(|g| g.collapsed && g.all.contains(&at)).map(|g| g.key);
    if let Some(key) = owner {
        update(s, |d| _ = d.collapsed.insert(key, false));
    }
}

pub fn toggle_collapsed(s: &mut App, key: &str) {
    update(s, |d| {
        let next = !d.collapsed.get(key).copied().unwrap_or(false);
        d.collapsed.insert(key.to_owned(), next);
    });
}

/// `renameGroup`: a manual group's own name, or an automatic group's override.
pub fn rename(s: &mut App, key: &str, name: String) {
    update(s, |d| match key.strip_prefix("manual:") {
        Some(id) => d.manual_groups.iter_mut().filter(|g| g.id == id).for_each(|g| g.name = name.clone()),
        None => d.overrides.entry(key.to_owned()).or_default().name = Some(name),
    });
}

/// `recolorGroup`.
pub fn recolor(s: &mut App, key: &str, color: Color) {
    update(s, |d| match key.strip_prefix("manual:") {
        Some(id) => d.manual_groups.iter_mut().filter(|g| g.id == id).for_each(|g| g.color = color),
        None => d.overrides.entry(key.to_owned()).or_default().color = Some(color),
    });
}

fn unnamed(n: usize) -> String {
    opencode_i18n::t!("tabs.groups.unnamed", number = n)
}

/// `createManualGroup`: a new group holding `seed` (or the current tab, or the first), named
/// the lowest free "Group N"; grouping turns to none, as the user now groups by hand.
pub fn create(s: &mut App, seed: Option<&str>) {
    let uid = seed.map(str::to_owned).or_else(|| s.current_tab()).or_else(|| s.tabs.first().map(Tab::uid));
    let Some(key) = uid.and_then(|uid| s.tab(&uid).map(tab_key)) else { return };
    let id = crate::windows::store::new_id();
    update(s, |d| {
        let color = model::next_manual_color(&d.manual_groups);
        let name = unnamed(model::next_manual_number(&d.manual_groups, &unnamed));
        d.mode = Mode::None;
        d.manual_groups.push(ManualGroup { id: id.clone(), name, color });
        d.manual_assignments.insert(key, id);
    });
}

/// `assignTabToManualGroup`: into group `id`, or out of its group with `None`.
pub fn assign(s: &mut App, uid: &str, id: Option<&str>) {
    let Some(key) = s.tab(uid).map(tab_key) else { return };
    update(s, |d| {
        d.mode = Mode::None;
        match id {
            Some(id) => _ = d.manual_assignments.insert(key, id.to_owned()),
            None => _ = d.manual_assignments.remove(&key),
        }
        d.manual_groups = model::prune_empty(&d.manual_groups, &d.manual_assignments);
    });
}

/// `ungroupManual`: its tabs stay, ungrouped.
pub fn ungroup(s: &mut App, id: &str) {
    update(s, |d| {
        d.manual_groups.retain(|g| g.id != id);
        d.manual_assignments.retain(|_, g| g != id);
        d.collapsed.remove(&format!("manual:{id}"));
    });
}

/// `closeGroup`: closes every tab in it, and a manual group goes with them.
pub fn close(s: &mut App, key: &str, tx: &mut Tx) {
    let items = metas(s);
    let Some(group) = groups(s, &items).into_iter().flatten().find(|g| g.key == key) else { return };
    let uids: Vec<String> = group.all.iter().filter_map(|&i| s.tabs.get(i).map(Tab::uid)).collect();
    for uid in uids {
        s.close_tab(&uid, tx);
    }
    if let Some(id) = group.manual_id() {
        ungroup(s, id);
    }
}

pub fn set_mode(s: &mut App, mode: Mode) {
    update(s, |d| d.mode = mode);
}

/// A header drag's step, its effects run here.
pub fn drag_step(s: &mut App, event: drag::Event, tx: &mut Tx) {
    match drag::step(&mut state_mut(s).drag, event) {
        drag::Effect::None => {}
        drag::Effect::Redraw => tx.animated(),
        drag::Effect::Click => {}
        drag::Effect::Commit { blocks, intent } => {
            tx.animated();
            let all: Vec<String> = s.tabs.iter().map(tab_key).collect();
            let c = drag::commit(&all, &state(s).doc, &blocks, &intent);
            update(s, |d| {
                d.group_order = c.group_order;
                d.manual_groups = c.manual_groups;
            });
            reorder(s, &c.tabs);
        }
    }
}

/// The header's press after any drag: toggles the group unless a drag just ended.
pub fn header_press(s: &mut App, key: &str, tx: &mut Tx) {
    tx.instant();
    if drag::step(&mut state_mut(s).drag, drag::Event::Press) == drag::Effect::Click {
        toggle_collapsed(s, key);
    }
}

/// The header's name field: starting it, a typed letter, its end.
pub fn rename_step(s: &mut App, event: RenameEvent, tx: &mut Tx) {
    match state_mut(s).rename.step(event) {
        RenameEffect::None => {}
        RenameEffect::Focus => tx.focus("tab-group-name", false),
        RenameEffect::Rename { group, name } => rename(s, &group, name),
    }
}

/// Tab `uid`'s menu rows (`TabAddToGroupMenuItems`): with no group yet New group, else Add to
/// group with New group, the groups (its own checked) and Remove from group. Only while tabs
/// group by hand.
pub fn tab_menu(s: &App, uid: &str) -> Vec<crate::session_menu::Line> {
    use crate::popup::{Item, Menu};
    use crate::session_menu::Line;
    if !on(s) || effective_mode(s) != Mode::None {
        return vec![];
    }
    let doc = &state(s).doc;
    let new = Item::new("tab-group:new", opencode_i18n::t!("tabs.groups.new"));
    if doc.manual_groups.is_empty() {
        return vec![Line::Extra(new)];
    }
    let assigned = s.tab(uid).and_then(|t| doc.manual_assignments.get(&tab_key(t)));
    let mut menu = Menu::new(opencode_i18n::t!("tabs.groups.addToGroup")).min_width(164.0).item(new.icon(&rune_theme_opencode::icons::PLUS)).separator();
    for g in &doc.manual_groups {
        menu = menu.item(Item::new(format!("tab-group:in:{}", g.id), g.name.clone()).checked(assigned == Some(&g.id)));
    }
    if assigned.is_some() {
        menu = menu.separator().item(Item::new("tab-group:out", opencode_i18n::t!("tabs.groups.removeFromGroup")));
    }
    vec![Line::Sub { id: "tab-group:add".into(), title: opencode_i18n::t!("tabs.groups.addToGroup").into(), menu }]
}

/// Runs a row [`tab_menu`] added.
pub fn run_tab_menu(s: &mut App, uid: &str, id: &str) -> bool {
    let Some(rest) = id.strip_prefix("tab-group:") else { return false };
    match rest {
        "new" => create(s, Some(uid)),
        "out" => assign(s, uid, None),
        _ => match rest.strip_prefix("in:") {
            // Picking its own checked group takes it out (`onChange(false)`).
            Some(g) => {
                let current = s.tab(uid).and_then(|t| state(s).doc.manual_assignments.get(&tab_key(t)).cloned());
                let next = (current.as_deref() != Some(g)).then_some(g);
                assign(s, uid, next);
            }
            None => return false,
        },
    }
    true
}
