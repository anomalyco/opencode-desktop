//! Desktop host for `opencode-feature-tab-groups` (`packages/app/src/shell/titlebar/tab-groups-ui.tsx`,
//! #64407): window-scoped `tabs.groups` persistence, 4-intent vertical drag execution (with 220 ms
//! dwell combine and horizontal tear-out handoff), immediate canonical ordering, and inline rename.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;

pub use opencode_feature_tab_groups::model::{self, Color, Doc, Group, Kind, ManualGroup, Meta, Mode};
pub use opencode_feature_tab_groups::{Rename, RenameEffect, RenameEvent, TabGroupsState, drag};

use opencode_kernel::{Feature, KernelBuilder, Slot};
use rune::Tx;
use rune_kit::shell::{TabTearMove, TabTearOut};

use crate::App;
use crate::tabs::Tab;

pub static TAB_GROUPS: Slot<TabGroupsState> = Slot::new();

/// Transient layout geometry and active horizontal tear-out state, kept outside tracked view state
/// so layout callbacks never dirty or trigger view rebuilds (`ours.md` §3.2).
#[derive(Default)]
pub struct GroupLayout {
    pub rects: RefCell<HashMap<String, [f32; 4]>>,
    pub tearing: Cell<bool>,
}

pub static LAYOUT: Slot<GroupLayout> = Slot::new();

const ROW: &str = "tabs.groups";
/// Horizontal tear-out threshold out of a vertical strip (`tab_strip.rs:105`: 48 pt).
const TEAR_DX: f32 = 48.0;

pub struct TabGroupsFeature;

impl Feature<App> for TabGroupsFeature {
    fn id(&self) -> &'static str {
        "feature.tab-groups"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&TAB_GROUPS);
        builder.register_slot(&LAYOUT);
        builder.schedule(crate::tick::stage::REACT_SHELL, "tab-groups.poll", |s, now, tx| poll(s, now, tx));
    }
}

pub fn state(s: &App) -> &TabGroupsState {
    s.kernel.slots.get(&TAB_GROUPS)
}

fn state_mut(s: &mut App) -> &mut TabGroupsState {
    s.kernel.slots.get_mut(&TAB_GROUPS)
}

pub fn torn_out(s: &App) -> bool {
    s.kernel.slots.untracked(&LAYOUT).tearing.get()
}

/// Whether grouped side tabs are active (`isGroupedSidebarList`, `titlebar.tsx:638`).
pub fn on(s: &App) -> bool {
    s.settings.tab_groups()
        && s.tab_layout() == crate::layout_state::TabLayout::Vertical
        && s.settings.sidebar_organize.effective() == crate::settings::Organize::List
}

pub fn servers(s: &App) -> usize {
    crate::servers::registry::list(s).len()
}

pub fn effective_mode(s: &App) -> Mode {
    model::effective_mode(state(s).doc.mode, servers(s))
}

/// Reads the window's `tabs.groups` row (from boot packet during launch).
pub fn restore(s: &mut App) {
    let ns = opencode_storage::window_namespace(&s.window.id);
    let doc = crate::storage::json(&ns, ROW).map(|v| Doc::from_json(&v)).unwrap_or_default();
    let keys: Vec<String> = s.tabs.iter().map(tab_key).collect();
    let cur = s.current_tab();
    let st = state_mut(s);
    st.doc = doc;
    st.seen_tabs = keys;
    st.seen_current = cur;
}

fn save(s: &App) {
    if s.persists() {
        crate::windows::store::save(&s.window, ROW, &state(s).doc.to_json());
    }
}

pub fn update(s: &mut App, f: impl FnOnce(&mut Doc)) {
    f(&mut state_mut(s).doc);
    save(s);
}

pub fn tab_key(t: &Tab) -> String {
    t.key().encode()
}

pub fn metas(s: &App) -> Vec<Meta> {
    let servers = crate::servers::registry::list(s);
    let label = |key: &opencode_kit::servers::ServerKey| {
        servers
            .iter()
            .find(|e| e.key() == *key)
            .map_or_else(|| crate::servers::runtime::label(key), |e| crate::servers::registry::label(&e.conn))
    };
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

fn folder_name(dir: &str) -> String {
    dir.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(dir).to_owned()
}

pub fn groups(s: &App, items: &[Meta]) -> Option<Vec<Group>> {
    let known: Vec<String> = crate::servers::registry::list(s).iter().map(|e| e.key().as_str().to_owned()).collect();
    let st = state(s);
    model::build(items, &st.doc, known.len(), &known, &st.filter.text, opencode_i18n::t!("sidebar.view.other"))
}

/// Visible tab UIDs in exact visual grouped order (for `nav_order()`, ⌘1–9, and `#63089` hints).
pub fn visible_uids(s: &App) -> Option<Vec<String>> {
    if !on(s) {
        return None;
    }
    let items = metas(s);
    let list = groups(s, &items)?;
    let mut out = Vec::new();
    for g in list {
        for &idx in &g.visible {
            if let Some(t) = s.tabs.get(idx) {
                out.push(t.uid());
            }
        }
    }
    Some(out)
}

pub fn reorder(s: &mut App, keys: &[String]) {
    let rank: HashMap<&str, usize> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let mut tabs: Vec<Tab> = s.tabs.to_vec();
    tabs.sort_by_key(|t| rank.get(tab_key(t).as_str()).copied().unwrap_or(usize::MAX));
    if tabs != *s.tabs {
        *s.tabs = tabs;
        s.save_tabs();
    }
}

/// Immediately keeps manual group tabs contiguous in `s.tabs` so visual order and `Mod+1..9` /
/// `Close tabs below` never diverge after `assign`, `create`, or switching to `Mode::None`.
fn canonicalize_now(s: &mut App) {
    if effective_mode(s) != Mode::None {
        return;
    }
    let keys: Vec<String> = s.tabs.iter().map(tab_key).collect();
    let (has_groups, canonical) = {
        let doc = &state(s).doc;
        (!doc.manual_groups.is_empty(), model::canonical_order(&keys, &doc.manual_assignments))
    };
    if has_groups && canonical != keys {
        reorder(s, &canonical);
        state_mut(s).seen_tabs = s.tabs.iter().map(tab_key).collect();
    }
}

fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    // When experimental tab groups is disabled, never reorder tabs, prune saved state, or dirty frames.
    if !s.settings.tab_groups() {
        return false;
    }
    let mut changed = false;

    // 220 ms combine-tabs dwell timer.
    if let Some(due) = s.kernel.slots.untracked(&TAB_GROUPS).drag.dwell_due() {
        if now >= due {
            drag_step(s, drag::Event::Tick { now }, tx);
            changed = true;
        } else {
            tx.wake_after(Duration::from_secs_f64((due - now).max(0.005)));
        }
    }

    let keys: Vec<String> = s.tabs.iter().map(tab_key).collect();
    let st = s.kernel.slots.untracked(&TAB_GROUPS);
    if keys != st.seen_tabs {
        let reconciled = model::reconcile(&st.seen_tabs, &keys, &st.doc.manual_assignments);
        state_mut(s).seen_tabs = keys.clone();
        changed = true;
        if let Some(assignments) = reconciled {
            update(s, |d| {
                d.manual_groups = model::prune_empty(&d.manual_groups, &assignments);
                d.manual_assignments = assignments;
            });
        }
        if on(s) {
            canonicalize_now(s);
        }
    }

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
        if on(s) {
            expand_current(s, current.as_deref());
        }
        changed = true;
    }
    changed
}

fn expand_current(s: &mut App, current: Option<&str>) {
    let Some(at) = current.and_then(|uid| s.tabs.iter().position(|t| t.uid() == uid)) else { return };
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

pub fn rename(s: &mut App, key: &str, name: String) {
    update(s, |d| match key.strip_prefix("manual:") {
        Some(id) => d.manual_groups.iter_mut().filter(|g| g.id == id).for_each(|g| g.name = name.clone()),
        None => d.overrides.entry(key.to_owned()).or_default().name = Some(name),
    });
}

pub fn recolor(s: &mut App, key: &str, color: Color) {
    update(s, |d| match key.strip_prefix("manual:") {
        Some(id) => d.manual_groups.iter_mut().filter(|g| g.id == id).for_each(|g| g.color = color),
        None => d.overrides.entry(key.to_owned()).or_default().color = Some(color),
    });
}

fn unnamed(n: usize) -> String {
    opencode_i18n::t!("tabs.groups.unnamed", number = n)
}

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
    canonicalize_now(s);
}

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
    canonicalize_now(s);
}

pub fn ungroup(s: &mut App, id: &str) {
    update(s, |d| {
        d.manual_groups.retain(|g| g.id != id);
        d.manual_assignments.retain(|_, g| g != id);
        d.collapsed.remove(&format!("manual:{id}"));
    });
}

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
    if mode == Mode::None {
        canonicalize_now(s);
    }
}

pub fn drag_step(s: &mut App, event: drag::Event, tx: &mut Tx) {
    match drag::step(&mut state_mut(s).drag, event) {
        drag::Effect::None => {}
        drag::Effect::Redraw { wake_after } => {
            tx.animated();
            if let Some(d) = wake_after {
                tx.wake_after(d);
            }
        }
        drag::Effect::Click => {}
        drag::Effect::Commit { blocks, intent } => {
            tx.animated();
            let all: Vec<String> = s.tabs.iter().map(tab_key).collect();
            let c = drag::commit(&all, &state(s).doc, &blocks, &intent, &crate::windows::store::new_id, &unnamed);
            update(s, |d| {
                d.mode = c.mode;
                d.group_order = c.group_order;
                d.collapsed = c.collapsed;
                d.manual_groups = c.manual_groups;
                d.manual_assignments = c.manual_assignments;
            });
            reorder(s, &c.tabs);
            state_mut(s).seen_tabs = s.tabs.iter().map(tab_key).collect();
        }
    }
}

/// Handles horizontal tear-out during a grouped tab drag so tearing a tab out of a grouped
/// sidebar works identically to an ungrouped vertical strip (`TEAR_DX = 48 pt`).
pub fn handle_tab_tear(s: &mut App, uid: &str, tab_index: usize, d: &rune::Drag, tx: &mut Tx) -> bool {
    if !s.can_tear() {
        return false;
    }
    let was_tearing = torn_out(s);
    let now_tearing = d.dx.abs() >= TEAR_DX;
    match d.phase {
        rune::Phase::Move if now_tearing || was_tearing => {
            if now_tearing && !was_tearing {
                state_mut(s).drag = drag::State::Idle;
                s.kernel.slots.untracked(&LAYOUT).tearing.set(true);
            }
            if !now_tearing && was_tearing {
                s.kernel.slots.untracked(&LAYOUT).tearing.set(false);
            }
            s.tear_move(
                TabTearMove {
                    id: uid.to_owned(),
                    index: tab_index,
                    torn: now_tearing,
                    grab: [16.0, 14.0],
                    window_point: d.window_point(),
                    screen_point: d.screen_point(),
                },
                tx,
            );
            true
        }
        rune::Phase::Up if was_tearing => {
            s.kernel.slots.untracked(&LAYOUT).tearing.set(false);
            if d.cancelled() {
                s.tear_back();
            } else {
                s.tear_release(
                    TabTearOut {
                        id: uid.to_owned(),
                        index: tab_index,
                        offset: [d.dx, d.dy],
                        window_point: Some(d.window_point()),
                        screen_point: d.screen_point(),
                    },
                    tx,
                );
            }
            true
        }
        _ => false,
    }
}

pub fn header_press(s: &mut App, key: &str, tx: &mut Tx) {
    tx.instant();
    if drag::step(&mut state_mut(s).drag, drag::Event::Press) == drag::Effect::Click {
        toggle_collapsed(s, key);
    }
}

pub fn tab_press(s: &mut App, uid: &str, tx: &mut Tx) {
    if drag::step(&mut state_mut(s).drag, drag::Event::Press) == drag::Effect::Click {
        s.select_tab(uid, tx);
    }
}

pub fn rename_step(s: &mut App, event: RenameEvent<'_>, tx: &mut Tx) {
    match state_mut(s).rename.step(event) {
        RenameEffect::None => {}
        RenameEffect::Focus => tx.focus("tab-group-name", false),
        RenameEffect::Rename { group, name } => rename(s, &group, name),
    }
}

pub fn tab_menu(s: &App, uid: &str) -> Vec<crate::session_menu::Line> {
    use crate::popup::{Item, Menu};
    use crate::session_menu::Line;
    if !on(s) || effective_mode(s) != Mode::None {
        return Vec::new();
    }
    let doc = &state(s).doc;
    let new = Item::new("tab-group:new", opencode_i18n::t!("tabs.groups.new"));
    if doc.manual_groups.is_empty() {
        return vec![Line::Extra(new)];
    }
    let assigned = s.tab(uid).and_then(|t| doc.manual_assignments.get(&tab_key(t)));
    let mut menu = Menu::new(opencode_i18n::t!("tabs.groups.addToGroup"))
        .min_width(164.0)
        .item(new.icon(&rune_theme_opencode::icons::PLUS))
        .separator();
    for g in &doc.manual_groups {
        menu = menu.item(Item::new(format!("tab-group:in:{}", g.id), g.name.clone()).checked(assigned == Some(&g.id)));
    }
    if assigned.is_some() {
        menu = menu.separator().item(Item::new("tab-group:out", opencode_i18n::t!("tabs.groups.removeFromGroup")));
    }
    vec![Line::Sub {
        id: "tab-group:add".into(),
        title: opencode_i18n::t!("tabs.groups.addToGroup").into(),
        menu,
    }]
}

pub fn run_tab_menu(s: &mut App, uid: &str, id: &str) -> bool {
    let Some(rest) = id.strip_prefix("tab-group:") else { return false };
    match rest {
        "new" => create(s, Some(uid)),
        "out" => assign(s, uid, None),
        _ => match rest.strip_prefix("in:") {
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
