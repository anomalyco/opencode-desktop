//! Group-and-sort model and menu for Consolidated navigation's List view
//! (`packages/app/src/shell/titlebar/recent-view.ts`, `recent-view-menu.tsx`, #64668).
//!
//! Persisted globally in `opencode.global.dat` under `"navigation.recent-view"` in Desktop's shape.

use std::collections::{BTreeMap, HashMap};

use opencode_i18n::t;
use opencode_kernel::Slot;
use rune::Tx;
use serde_json::{Value, json};

use crate::App;
use crate::popup::{self, Item, Menu};

pub static RECENT_VIEW: Slot<RecentViewState> = Slot::new();

const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "navigation.recent-view";
pub const NO_PROJECT: &str = "none";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupBy {
    #[default]
    None,
    Date,
    Status,
    Project,
    Server,
}

impl GroupBy {
    pub const GROUPED: [GroupBy; 4] = [GroupBy::Date, GroupBy::Status, GroupBy::Project, GroupBy::Server];

    pub fn name(self) -> &'static str {
        match self {
            GroupBy::None => "none",
            GroupBy::Date => "date",
            GroupBy::Status => "status",
            GroupBy::Project => "project",
            GroupBy::Server => "server",
        }
    }

    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("date") => GroupBy::Date,
            Some("status") => GroupBy::Status,
            Some("project") => GroupBy::Project,
            Some("server") => GroupBy::Server,
            _ => GroupBy::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GroupBy::None => t!("sidebar.recent.view.groupBy.none"),
            GroupBy::Date => t!("sidebar.recent.view.groupBy.date"),
            GroupBy::Status => t!("sidebar.recent.view.groupBy.status"),
            GroupBy::Project => t!("sidebar.recent.view.groupBy.project"),
            GroupBy::Server => t!("sidebar.recent.view.groupBy.server"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Updated,
    Created,
    Name,
}

impl SortBy {
    pub const ALL: [SortBy; 3] = [SortBy::Updated, SortBy::Created, SortBy::Name];

    pub fn name(self) -> &'static str {
        match self {
            SortBy::Updated => "updated",
            SortBy::Created => "created",
            SortBy::Name => "name",
        }
    }

    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("created") => SortBy::Created,
            Some("name") => SortBy::Name,
            _ => SortBy::Updated,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortBy::Updated => t!("sidebar.recent.view.sortBy.updated"),
            SortBy::Created => t!("sidebar.recent.view.sortBy.created"),
            SortBy::Name => t!("sidebar.recent.view.sortBy.name"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Desc,
    Asc,
}

impl Direction {
    pub const ALL: [Direction; 2] = [Direction::Desc, Direction::Asc];

    pub fn name(self) -> &'static str {
        match self {
            Direction::Desc => "desc",
            Direction::Asc => "asc",
        }
    }

    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("asc") => Direction::Asc,
            _ => Direction::Desc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Direction::Desc => t!("sidebar.recent.view.direction.desc"),
            Direction::Asc => t!("sidebar.recent.view.direction.asc"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecentStatus {
    Attention,
    Working,
    Unread,
    Read,
}

impl RecentStatus {
    pub const ALL: [RecentStatus; 4] = [
        RecentStatus::Attention,
        RecentStatus::Working,
        RecentStatus::Unread,
        RecentStatus::Read,
    ];

    pub fn id(self) -> &'static str {
        match self {
            RecentStatus::Attention => "attention",
            RecentStatus::Working => "working",
            RecentStatus::Unread => "unread",
            RecentStatus::Read => "read",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RecentStatus::Attention => t!("sidebar.recent.group.status.attention"),
            RecentStatus::Working => t!("sidebar.recent.group.status.working"),
            RecentStatus::Unread => t!("sidebar.recent.group.status.unread"),
            RecentStatus::Read => t!("sidebar.recent.group.status.read"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecentDate {
    Today,
    Yesterday,
    Week,
    Older,
}

impl RecentDate {
    pub const ALL: [RecentDate; 4] = [
        RecentDate::Today,
        RecentDate::Yesterday,
        RecentDate::Week,
        RecentDate::Older,
    ];

    pub fn id(self) -> &'static str {
        match self {
            RecentDate::Today => "today",
            RecentDate::Yesterday => "yesterday",
            RecentDate::Week => "week",
            RecentDate::Older => "older",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RecentDate::Today => t!("sidebar.recent.group.date.today"),
            RecentDate::Yesterday => t!("sidebar.recent.group.date.yesterday"),
            RecentDate::Week => t!("sidebar.recent.group.date.week"),
            RecentDate::Older => t!("sidebar.recent.group.date.older"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentView {
    pub group_by: GroupBy,
    pub sort_by: SortBy,
    pub direction: Direction,
    pub metadata: bool,
    pub empty_groups: bool,
    pub collapsed: Vec<String>,
}

impl RecentView {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn is_collapsed(&self, key: &str) -> bool {
        self.group_by != GroupBy::None && self.collapsed.iter().any(|k| k == key)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "groupBy": self.group_by.name(),
            "sortBy": self.sort_by.name(),
            "direction": self.direction.name(),
            "metadata": self.metadata,
            "emptyGroups": self.empty_groups,
            "collapsed": self.collapsed,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        if !v.is_object() {
            return Self::default();
        }
        Self {
            group_by: GroupBy::parse(v["groupBy"].as_str()),
            sort_by: SortBy::parse(v["sortBy"].as_str()),
            direction: Direction::parse(v["direction"].as_str()),
            metadata: v["metadata"].as_bool().unwrap_or(false),
            empty_groups: v["emptyGroups"].as_bool().unwrap_or(false),
            collapsed: v["collapsed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
        }
    }
}

#[derive(Default)]
pub struct RecentViewState {
    pub view: RecentView,
}

pub fn current(s: &App) -> &RecentView {
    &s.kernel.slots.get(&RECENT_VIEW).view
}

pub fn restore(s: &mut App) {
    let view = crate::storage::json(GLOBAL, KEY).map(|v| RecentView::from_json(&v)).unwrap_or_default();
    s.kernel.slots.get_mut(&RECENT_VIEW).view = view;
}

pub fn update(s: &mut App, f: impl FnOnce(&mut RecentView)) {
    let persists = s.persists();
    let st = s.kernel.slots.get_mut(&RECENT_VIEW);
    let before = st.view.clone();
    f(&mut st.view);
    if st.view != before && persists {
        let json = st.view.to_json();
        crate::storage::set_json(GLOBAL, KEY, &json);
    }
}

pub fn toggle_collapsed(s: &mut App, key: &str) {
    let key = key.to_owned();
    update(s, |v| {
        if let Some(pos) = v.collapsed.iter().position(|k| k == &key) {
            v.collapsed.remove(pos);
        } else {
            v.collapsed.push(key);
        }
    });
}

pub struct ViewRow<T> {
    pub key: String,
    pub title: String,
    pub updated: i64,
    pub created: i64,
    pub status: RecentStatus,
    pub project: Option<(String, String)>,
    pub server: String,
    pub item: T,
}

pub struct ViewGroup<T> {
    pub key: String,
    pub label: String,
    pub rows: Vec<ViewRow<T>>,
}

/// Groups and sorts Recent rows (`recentGroups`, `recent-view.ts`).
pub fn build_groups<T>(
    mut rows: Vec<ViewRow<T>>,
    view: &RecentView,
    now_ms: i64,
    servers: &[(String, String)],
) -> Vec<ViewGroup<T>> {
    rows.sort_by(|a, b| compare_rows(view, a, b));
    match view.group_by {
        GroupBy::None => vec![ViewGroup {
            key: "none:all".into(),
            label: t!("sidebar.recent.section.recent").to_owned(),
            rows,
        }],
        GroupBy::Date => {
            let mut order = RecentDate::ALL.to_vec();
            if view.direction == Direction::Asc {
                order.reverse();
            }
            let specs: Vec<(String, String)> = order.into_iter().map(|d| (d.id().to_owned(), d.label().to_owned())).collect();
            let by = |r: &ViewRow<T>| {
                let ts = if view.sort_by == SortBy::Created { r.created } else { r.updated };
                date_bucket(now_ms, ts).id().to_owned()
            };
            fixed_groups(view.group_by, view.empty_groups, &specs, rows, by)
        }
        GroupBy::Status => {
            let specs: Vec<(String, String)> = RecentStatus::ALL.into_iter().map(|s| (s.id().to_owned(), s.label().to_owned())).collect();
            fixed_groups(view.group_by, view.empty_groups, &specs, rows, |r| r.status.id().to_owned())
        }
        GroupBy::Server => fixed_groups(view.group_by, view.empty_groups, servers, rows, |r| r.server.clone()),
        GroupBy::Project => {
            let mut projects: BTreeMap<String, String> = BTreeMap::new();
            for row in &rows {
                let (id, label) = match &row.project {
                    Some((k, l)) => (k.clone(), l.clone()),
                    None => (NO_PROJECT.to_owned(), t!("sidebar.recent.group.project.none").to_owned()),
                };
                projects.entry(id).or_insert(label);
            }
            let mut specs: Vec<(String, String)> = projects.into_iter().collect();
            specs.sort_by(|a, b| match (a.0 == NO_PROJECT, b.0 == NO_PROJECT) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => rune::text::compare_names(&a.1, &b.1),
            });
            fixed_groups(view.group_by, false, &specs, rows, |r| {
                r.project.as_ref().map_or_else(|| NO_PROJECT.to_owned(), |(k, _)| k.clone())
            })
        }
    }
}

fn fixed_groups<T>(
    group_by: GroupBy,
    empty_groups: bool,
    specs: &[(String, String)],
    rows: Vec<ViewRow<T>>,
    group_of: impl Fn(&ViewRow<T>) -> String,
) -> Vec<ViewGroup<T>> {
    let mut map: HashMap<String, Vec<ViewRow<T>>> = HashMap::new();
    for row in rows {
        map.entry(group_of(&row)).or_default().push(row);
    }
    specs
        .iter()
        .filter_map(|(id, label)| {
            let members = map.remove(id).unwrap_or_default();
            if members.is_empty() && !empty_groups {
                return None;
            }
            Some(ViewGroup {
                key: format!("{}:{id}", group_by.name()),
                label: label.clone(),
                rows: members,
            })
        })
        .collect()
}

fn compare_rows<T>(view: &RecentView, a: &ViewRow<T>, b: &ViewRow<T>) -> std::cmp::Ordering {
    let primary = match view.sort_by {
        SortBy::Name => rune::text::compare_names(&b.title, &a.title),
        SortBy::Created => b.created.cmp(&a.created),
        SortBy::Updated => b.updated.cmp(&a.updated),
    };
    let primary = if view.direction == Direction::Asc { primary.reverse() } else { primary };
    primary.then_with(|| b.updated.cmp(&a.updated)).then_with(|| a.key.cmp(&b.key))
}

fn date_bucket(now_ms: i64, ts_ms: i64) -> RecentDate {
    let day = |ms: i64| {
        let t = rune::LocalTime::at(ms);
        (t.year, t.month, t.day)
    };
    let d = day(ts_ms);
    if d == day(now_ms) {
        return RecentDate::Today;
    }
    if d == day(now_ms - 86_400_000) {
        return RecentDate::Yesterday;
    }
    if ts_ms >= now_ms - 7 * 86_400_000 {
        return RecentDate::Week;
    }
    RecentDate::Older
}

/// Relative time or status metadata shown when `RecentView::metadata` is enabled.
pub fn meta_label(view: &RecentView, status: RecentStatus, created: i64, updated: i64, now_ms: i64) -> Option<String> {
    if !view.metadata {
        return None;
    }
    match status {
        RecentStatus::Attention => return Some(t!("sidebar.recent.meta.attention").to_owned()),
        RecentStatus::Working => return Some(t!("sidebar.recent.meta.working").to_owned()),
        RecentStatus::Unread | RecentStatus::Read => {}
    }
    let ts = if view.sort_by == SortBy::Created { created } else if updated > 0 { updated } else { created };
    let secs = ((now_ms - ts).max(0)) / 1000;
    if secs < 60 {
        return Some(t!("sidebar.inbox.time.now").to_owned());
    }
    if secs < 3600 {
        return Some(format!("{}m", secs / 60));
    }
    if secs < 86_400 {
        return Some(format!("{}h", secs / 3600));
    }
    Some(format!("{}d", secs / 86_400))
}

/// Opens `RecentViewMenu` anchored to `trigger_key`.
pub fn open_menu(s: &mut App, trigger_key: String, tx: &mut Tx) {
    let view = current(s).clone();
    let mut group_menu = Menu::new(t!("sidebar.recent.view.groupBy")).min_width(160.0);
    for g in GroupBy::GROUPED {
        group_menu = group_menu.item(Item::new(format!("group:{}", g.name()), g.label()).checked(view.group_by == g));
    }
    group_menu = group_menu
        .separator()
        .item(Item::new("group:none", GroupBy::None.label()).checked(view.group_by == GroupBy::None));

    let mut sort_menu = Menu::new(t!("sidebar.recent.view.sortBy")).min_width(160.0);
    for by in SortBy::ALL {
        sort_menu = sort_menu.item(Item::new(format!("sort:{}", by.name()), by.label()).checked(view.sort_by == by));
    }
    sort_menu = sort_menu.separator();
    for dir in Direction::ALL {
        sort_menu = sort_menu.item(Item::new(format!("dir:{}", dir.name()), dir.label()).checked(view.direction == dir));
    }

    let show_menu = Menu::new(t!("sidebar.recent.view.show"))
        .min_width(160.0)
        .item(Item::new("show:metadata", t!("sidebar.recent.view.show.metadata")).checked(view.metadata))
        .item(Item::new("show:empty", t!("sidebar.recent.view.show.emptyGroups")).checked(view.empty_groups));

    let menu = Menu::new(t!("sidebar.recent.view.options"))
        .min_width(200.0)
        .gap(4.0)
        .submenu("group", format!("{}  ·  {}", t!("sidebar.recent.view.groupBy"), view.group_by.label()), group_menu)
        .submenu("sort", format!("{}  ·  {}", t!("sidebar.recent.view.sortBy"), view.sort_by.label()), sort_menu)
        .submenu("show", t!("sidebar.recent.view.show"), show_menu)
        .separator()
        .item(Item::new("reset", t!("sidebar.recent.view.reset")).enabled(!view.is_default()))
        .item(Item::new("view-all", t!("sidebar.recent.view.viewAll")));

    tx.instant();
    popup::open_drawn(s, menu, trigger_key, tx, |s: &mut App, picked, tx| {
        tx.instant();
        let Some(id) = picked else { return };
        match id {
            "reset" => update(s, |v| *v = RecentView::default()),
            "view-all" => {
                if s.route != crate::Route::Home || s.settings_open() {
                    s.go_home(tx);
                }
            }
            "show:metadata" => update(s, |v| v.metadata = !v.metadata),
            "show:empty" => update(s, |v| v.empty_groups = !v.empty_groups),
            _ => {
                if let Some(g) = id.strip_prefix("group:") {
                    let next = GroupBy::parse(Some(g));
                    update(s, |v| v.group_by = next);
                } else if let Some(by) = id.strip_prefix("sort:") {
                    let next = SortBy::parse(Some(by));
                    update(s, |v| v.sort_by = next);
                } else if let Some(d) = id.strip_prefix("dir:") {
                    let next = Direction::parse(Some(d));
                    update(s, |v| v.direction = next);
                }
            }
        }
    });
}
