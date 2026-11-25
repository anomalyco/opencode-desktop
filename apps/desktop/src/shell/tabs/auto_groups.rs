//! Consolidated's open tabs grouped by their project or their server, as Settings › Group tabs
//! picks (official `packages/app/src/shell/titlebar/auto-tab-groups.ts` and
//! `auto-tab-sections.tsx`, anomalyco/opencode#66350). Not the experimental tab groups of
//! `groups.rs`, which a person makes and names.
//!
//! A group stands where its first tab does; the tabs with no project come last, under no header.
//! Pinned tabs are in no group. Which groups are collapsed is the window's (`tabs.autoGroups`),
//! and only side tabs collapse.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use opencode_kernel::Slot;
use opencode_kit::model::Project;
use opencode_kit::servers::ServerKey;
use rune::Tracked;
use serde_json::json;

use crate::layout_state::TabLayout;
use crate::settings::TabGrouping;
use crate::tabs::{Tab, pins};
use crate::{App, TabStatus};

pub static AUTO_GROUPS: Slot<AutoGroups> = Slot::new();

/// The window's row (`Persist.window("tabs.autoGroups")`, `auto-tab-sections.tsx:34`).
const ROW: &str = "tabs.autoGroups";
const OTHER: &str = "other";

#[derive(Default)]
pub struct AutoGroups {
    /// The collapsed groups, by key (`CollapsedState`, `auto-tab-sections.tsx:17`).
    pub collapsed: BTreeMap<String, bool>,
    /// What the saved tab order was last checked against (`inputs`), so a frame where none of it
    /// changed groups nothing.
    checked: Cell<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Project,
    Server,
    /// The tabs with no project, after every project, without a header.
    Other,
}

/// One group (`AutoTabGroup`, `auto-tab-groups.ts:13-26`), its tabs as places in `App::tabs`.
pub struct Group {
    pub key: String,
    pub kind: Kind,
    pub server: Option<ServerKey>,
    /// Grouped by project, the same project also has a group on another server.
    pub spans_servers: bool,
    pub collapsed: bool,
    /// Every tab in the group, in order.
    pub all: Vec<usize>,
    /// The tabs shown: all of them, or only the current one while the group is collapsed.
    pub tabs: Vec<usize>,
}

/// What a collapsed group's header shows of its tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Busy,
    Attention,
    Unread,
}

/// A tab with what it groups by (`AutoTabGroupItem`).
struct Item<'a> {
    index: usize,
    server: &'a ServerKey,
    project: Option<&'a str>,
}

/// `autoTabGroups` (`auto-tab-groups.ts:34-76`): the groups in the order each one's first tab
/// appears, or `None` when nothing groups the tabs: grouping is off, or it is by server and
/// there is only one.
fn build(grouping: TabGrouping, servers: usize, items: &[Item], collapsed: &BTreeMap<String, bool>, current: Option<usize>) -> Option<Vec<Group>> {
    if grouping == TabGrouping::None || (grouping == TabGrouping::Server && servers < 2) {
        return None;
    }
    let mut buckets: Vec<(String, Vec<&Item>)> = Vec::new();
    for item in items {
        let key = match (grouping, item.project) {
            (TabGrouping::Server, _) => format!("server:{}", item.server.as_str()),
            (_, Some(project)) => format!("project:{}\0{project}", item.server.as_str()),
            (_, None) => OTHER.to_owned(),
        };
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, rows)) => rows.push(item),
            None => buckets.push((key, vec![item])),
        }
    }
    // A stable sort: the projects or servers keep their order, and Other goes after them.
    buckets.sort_by_key(|(key, _)| key == OTHER);
    let groups = buckets
        .iter()
        .map(|(key, rows)| {
            let kind = match (key == OTHER, grouping) {
                (true, _) => Kind::Other,
                (false, TabGrouping::Server) => Kind::Server,
                (false, _) => Kind::Project,
            };
            let folded = kind != Kind::Other && collapsed.get(key).copied().unwrap_or(false);
            let all: Vec<usize> = rows.iter().map(|row| row.index).collect();
            Group {
                key: key.clone(),
                kind,
                server: (kind != Kind::Other).then(|| rows[0].server.clone()),
                spans_servers: kind == Kind::Project && buckets.iter().filter(|(k, others)| k != OTHER && others[0].project == rows[0].project).count() > 1,
                collapsed: folded,
                tabs: if folded { all.iter().copied().filter(|&i| Some(i) == current).collect() } else { all.clone() },
                all,
            }
        })
        .collect();
    Some(groups)
}

/// `groupedTabOrder` (`auto-tab-groups.ts:82-88`): for each place in the tab order, the tab that
/// goes there so that each group's tabs stand together, the groups in the order they first
/// appear. Tabs outside every group (pinned, or with no project yet) keep their places, so a
/// project that loads late joins its group where it was.
fn grouped_order(count: usize, groups: &[Group]) -> Vec<usize> {
    let grouped: Vec<usize> = groups.iter().filter(|g| g.kind != Kind::Other).flat_map(|g| g.all.iter().copied()).collect();
    let mut member = vec![false; count];
    for &i in &grouped {
        member[i] = true;
    }
    let mut next = grouped.into_iter();
    (0..count).map(|i| if member[i] { next.next().unwrap_or(i) } else { i }).collect()
}

/// The project a tab groups under (`projectOf`, `auto-tab-sections.tsx:39-49`): a draft's by its
/// folder, a session's own, or its saved folder's while its server has not listed it. A tab of a
/// server the window is not working against has what that server last showed of it.
pub fn project_of<'a>(s: &'a App, t: &'a Tab) -> Option<&'a Project> {
    if t.server != *crate::servers::runtime::active(s) {
        return if t.is_draft() { None } else { s.checkout.project(&t.server, &t.id) };
    }
    if t.is_draft() {
        return s.project_at(&t.directory);
    }
    match s.session(&t.id) {
        Some(info) => s.project_for_session(info),
        None if t.directory.is_empty() => None,
        None => s.project_at(&t.directory),
    }
}

fn grouped(s: &App, collapse: bool) -> Option<Vec<Group>> {
    let grouping = s.settings.tab_grouping();
    if grouping == TabGrouping::None {
        return None;
    }
    // Counted only where it decides something: listing the servers is not free.
    let servers = if grouping == TabGrouping::Server { crate::servers::registry::list(s).len() } else { 0 };
    let items: Vec<Item> = s
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| !pins::holds(s, t))
        .map(|(index, t)| Item { index, server: &t.server, project: project_of(s, t).map(|p| p.id.as_str()) })
        .collect();
    if !collapse {
        return build(grouping, servers, &items, &BTreeMap::new(), None);
    }
    // The tab the strip marks as the open page's (`tab_strip::strip`).
    let current = if s.settings_open() { None } else { s.current_tab().and_then(|uid| s.tab_at(&uid)) };
    build(grouping, servers, &items, &s.kernel.slots.get(&AUTO_GROUPS).collapsed, current)
}

/// The groups as this window's layout shows them, or `None` while nothing groups the tabs. Side
/// tabs collapse; top tabs never do (`topGroups`, `auto-tab-sections.tsx:66-68`). A narrow
/// window shows no groups: its drawer lists every tab in the saved order
/// (`titlebar.tsx:840-855`, a strip given no sections).
pub fn groups(s: &App) -> Option<Vec<Group>> {
    if s.narrow() {
        return None;
    }
    grouped(s, s.tab_layout() != TabLayout::Horizontal)
}

/// Whether the tabs at places `a` and `b` stand in one group, so a drag may move one to the
/// other's place: a drop in another group puts the tab back (`tab-strip.tsx:1205-1206`).
pub fn same_group(s: &App, a: usize, b: usize) -> bool {
    grouped(s, false).is_none_or(|groups| groups.iter().any(|g| g.all.contains(&a) && g.all.contains(&b)))
}

/// What a collapsed group's header shows of its tabs (`status`, `auto-tab-sections.tsx:121-129`):
/// one working, else one waiting on a person, else one unread. A permission or a question that
/// waits in the open tab counts too, as official reads every tab's own state there.
pub fn mark(s: &App, g: &Group) -> Option<Mark> {
    if !g.collapsed {
        return None;
    }
    let here = crate::servers::runtime::active(s);
    let marks: Vec<Mark> = g
        .all
        .iter()
        .filter_map(|&i| s.tabs.get(i))
        .filter(|t| !t.is_draft())
        .filter_map(|t| {
            if t.server == *here && s.marks(&t.id).attention {
                return Some(Mark::Attention);
            }
            Some(match s.tab_status_of(t)? {
                TabStatus::Busy => Mark::Busy,
                TabStatus::Attention => Mark::Attention,
                TabStatus::Unread | TabStatus::Failed => Mark::Unread,
            })
        })
        .collect();
    [Mark::Busy, Mark::Attention, Mark::Unread].into_iter().find(|m| marks.contains(m))
}

/// Reads the window's `tabs.autoGroups` row.
pub fn restore(s: &mut App) {
    let ns = opencode_storage::window_namespace(&s.window.id);
    let collapsed: BTreeMap<String, bool> = crate::storage::json(&ns, ROW)
        .and_then(|v| v["collapsed"].as_object().map(|o| o.iter().filter_map(|(k, b)| Some((k.clone(), b.as_bool()?))).collect()))
        .unwrap_or_default();
    s.kernel.slots.get_mut(&AUTO_GROUPS).collapsed = collapsed;
}

/// A press on a group's header: it collapses, or opens again.
pub fn toggle(s: &mut App, key: &str) {
    let collapsed = &mut s.kernel.slots.get_mut(&AUTO_GROUPS).collapsed;
    let next = !collapsed.get(key).copied().unwrap_or(false);
    collapsed.insert(key.to_owned(), next);
    if s.persists() {
        crate::windows::store::save(&s.window, ROW, &json!({ "collapsed": s.kernel.slots.get(&AUTO_GROUPS).collapsed }));
    }
}

/// Everything the grouped order is read from, by version.
fn inputs(s: &App) -> u64 {
    let mut h = DefaultHasher::new();
    (
        Tracked::version(&s.tabs),
        Tracked::version(&s.sessions),
        Tracked::version(&s.projects),
        Tracked::version(&s.settings),
        Tracked::version(&s.checkout),
        s.kernel.slots.version(&pins::PINS),
        crate::servers::runtime::version(),
    )
        .hash(&mut h);
    crate::servers::runtime::active(s).hash(&mut h);
    h.finish()
}

/// Per-frame poll (`stage::REACT_SHELL`): keeps each group's tabs together in the saved tab
/// order, so opening, closing and dragging tabs never reorders groups
/// (`auto-tab-sections.tsx:70-79`). Nothing to do while tabs are not grouped, or while nothing
/// the order is read from changed.
pub fn poll(s: &mut App) -> bool {
    if s.settings.tab_grouping() == TabGrouping::None {
        return false;
    }
    let seen = inputs(s);
    if s.kernel.slots.untracked(&AUTO_GROUPS).checked.replace(seen) == seen {
        return false;
    }
    let Some(groups) = grouped(s, false) else { return false };
    let order = grouped_order(s.tabs.len(), &groups);
    if order.iter().enumerate().all(|(at, &from)| at == from) {
        return false;
    }
    let tabs: Vec<Tab> = order.into_iter().map(|i| s.tabs[i].clone()).collect();
    *s.tabs = tabs;
    s.save_tabs();
    true
}
