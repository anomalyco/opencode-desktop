//! Sidebar state held on [`crate::ProjectsState`]:
//! - [`ProjectDrag`]: vertical drag state for reordering projects in the Organized sidebar.
//! - [`Customize`], [`Grouping`], [`Ordering`], [`Status`], [`Show`], [`Filters`]: Customize
//!   sidebar view, filters, and per-group Show more counts (`sidebar-customized.tsx`).
//! - [`Recent`]: Recent sidebar pins (`sidebar.recent`) and revealed count (`recent-sessions.tsx`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use opencode_i18n::t;
use serde_json::{Value, json};

/// A project row's drag (`DragDropProvider` with `RestrictToVerticalAxis`): it starts 4 pt from
/// the press, the project's block follows the pointer up and down, the blocks it passes step
/// aside by its height, and a drop puts it there at once (`dropAnimation: null`).
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectDrag {
    pub from: usize,
    pub to: usize,
    pub dy: f32,
    /// The listed projects' ids, in order, as the drag started.
    pub ids: Vec<String>,
}

/// Group by (`SidebarGrouping`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Grouping {
    None,
    #[default]
    Project,
    ServerProject,
    Server,
}

impl Grouping {
    pub const ALL: &'static [Grouping] = &[Grouping::None, Grouping::Project, Grouping::ServerProject, Grouping::Server];

    pub fn name(self) -> &'static str {
        match self {
            Grouping::None => "none",
            Grouping::Project => "project",
            Grouping::ServerProject => "server-project",
            Grouping::Server => "server",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Grouping::None => t!("sidebar.view.grouping.none"),
            Grouping::Project => t!("sidebar.view.grouping.project"),
            Grouping::ServerProject => t!("sidebar.view.grouping.server-project"),
            Grouping::Server => t!("sidebar.view.grouping.server"),
        }
    }

    pub fn parse(raw: Option<&str>) -> Grouping {
        match raw {
            Some("none") => Grouping::None,
            Some("project") => Grouping::Project,
            Some("server-project") => Grouping::ServerProject,
            Some("server") => Grouping::Server,
            _ => Grouping::default(),
        }
    }

    pub fn by_server(self) -> bool {
        matches!(self, Grouping::Server | Grouping::ServerProject)
    }
}

/// Every grouping the menu offers: the server ones only once a second server is listed.
pub fn groupings(multi_server: bool) -> impl Iterator<Item = Grouping> {
    Grouping::ALL.iter().copied().filter(move |g| multi_server || !g.by_server())
}

/// Sort by (`SidebarOrdering`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Ordering {
    #[default]
    Manual,
    Updated,
    Created,
    Attention,
}

impl Ordering {
    pub const ALL: &'static [Ordering] = &[Ordering::Manual, Ordering::Updated, Ordering::Created, Ordering::Attention];

    pub fn name(self) -> &'static str {
        match self {
            Ordering::Manual => "manual",
            Ordering::Updated => "updated",
            Ordering::Created => "created",
            Ordering::Attention => "attention",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Ordering::Manual => t!("sidebar.view.ordering.manual"),
            Ordering::Updated => t!("sidebar.view.ordering.updated"),
            Ordering::Created => t!("sidebar.view.ordering.created"),
            Ordering::Attention => t!("sidebar.view.ordering.attention"),
        }
    }

    pub fn parse(raw: Option<&str>) -> Ordering {
        match raw {
            Some("manual") => Ordering::Manual,
            Some("updated") => Ordering::Updated,
            Some("created") => Ordering::Created,
            Some("attention") => Ordering::Attention,
            _ => Ordering::default(),
        }
    }
}

/// A row's status, in the Status filter's order (`SIDEBAR_STATUSES`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Status {
    Running,
    Input,
    Unread,
    Failed,
    #[default]
    Idle,
}

impl Status {
    pub const ALL: &'static [Status] = &[Status::Running, Status::Input, Status::Unread, Status::Failed, Status::Idle];

    pub fn name(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Input => "input",
            Status::Unread => "unread",
            Status::Failed => "failed",
            Status::Idle => "idle",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Running => t!("sidebar.view.status.running"),
            Status::Input => t!("sidebar.view.status.input"),
            Status::Unread => t!("sidebar.view.status.unread"),
            Status::Failed => t!("sidebar.view.status.failed"),
            Status::Idle => t!("sidebar.view.status.idle"),
        }
    }

    pub fn parse(raw: Option<&str>) -> Status {
        match raw {
            Some("running") => Status::Running,
            Some("input") => Status::Input,
            Some("unread") => Status::Unread,
            Some("failed") => Status::Failed,
            Some("idle") => Status::Idle,
            _ => Status::default(),
        }
    }

    /// Attention order (`ATTENTION`): needs input, failed, unread, running, idle.
    pub fn attention(self) -> u8 {
        match self {
            Status::Input => 0,
            Status::Failed => 1,
            Status::Unread => 2,
            Status::Running => 3,
            Status::Idle => 4,
        }
    }
}

/// Show (`SidebarShow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Show {
    pub recent: bool,
    pub subagents: bool,
    pub server_labels: bool,
}

impl Default for Show {
    fn default() -> Self {
        Show { recent: true, subagents: false, server_labels: true }
    }
}

/// Filters (`SidebarFilters`): a project of `"none"` matches rows without one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Filters {
    pub status: Vec<Status>,
    pub server: Vec<String>,
    pub project: Vec<String>,
}

impl Filters {
    pub fn count(&self) -> usize {
        self.status.len() + self.server.len() + self.project.len()
    }
}

/// Customize sidebar view (`sidebar.view`, shared by every window) with this window's filters and
/// Show more counts, which reset on reload.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Customize {
    pub grouping: Grouping,
    pub ordering: Ordering,
    pub show: Show,
    /// Collapsed groups by key.
    pub collapsed: BTreeMap<String, bool>,
    /// This window's filters (`local.filters`).
    pub filters: Filters,
    /// Rows each group's footer shows, past its first step (`local.limits`).
    pub limits: HashMap<String, usize>,
}

impl Customize {
    pub fn to_json(&self) -> Value {
        json!({
            "grouping": self.grouping.name(),
            "ordering": self.ordering.name(),
            "show": { "recent": self.show.recent, "subagents": self.show.subagents, "serverLabels": self.show.server_labels },
            "collapsed": self.collapsed,
        })
    }

    pub fn restore(&mut self, v: &Value) {
        let d = Show::default();
        let flag = |k: &str, default: bool| v["show"][k].as_bool().unwrap_or(default);
        self.grouping = Grouping::parse(v["grouping"].as_str());
        self.ordering = Ordering::parse(v["ordering"].as_str());
        self.show = Show {
            recent: flag("recent", d.recent),
            subagents: flag("subagents", d.subagents),
            server_labels: flag("serverLabels", d.server_labels),
        };
        self.collapsed = v["collapsed"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, on)| Some((k.clone(), on.as_bool()?)))
            .collect();
    }

    /// The persisted half, for the restore stamp.
    pub fn stamp(&self, h: &mut impl Hasher) {
        (self.grouping, self.ordering, self.show, &self.collapsed).hash(h);
    }

    pub fn collapsed(&self, key: &str) -> bool {
        self.collapsed.get(key).copied().unwrap_or(false)
    }
}

/// Recent sidebar pins (`sidebar.recent`) and how many extra rows Show more revealed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recent {
    /// Ordered `${server}\u{0}${sessionID}` keys (`recentSessionKey`).
    pub pins: Vec<String>,
    /// Rows Show more added past the 7-day window (`view.revealed`).
    pub revealed: usize,
}

impl Recent {
    pub fn to_json(&self) -> Value {
        json!({ "pins": self.pins })
    }

    /// Drops duplicate and malformed keys, keeping each one's first place (`dedupePins`).
    pub fn restore(&mut self, v: &Value) {
        let mut seen = HashSet::new();
        self.pins = v["pins"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|k| split_recent_key(k).is_some_and(|(_, id)| !id.is_empty()) && seen.insert((*k).to_owned()))
            .map(str::to_owned)
            .collect();
    }

    /// Pinning appends; an existing pin keeps its place (`addPin`).
    pub fn pin(&mut self, key: String) {
        if !self.pins.contains(&key) {
            self.pins.push(key);
        }
    }

    pub fn unpin(&mut self, key: &str) {
        self.pins.retain(|k| k != key);
    }

    /// Moves `key` one place among the `visible` pins (`movePin`).
    pub fn shift(&mut self, visible: &[String], key: &str, up: bool) {
        let Some(at) = visible.iter().position(|k| k == key) else { return };
        let Some(to) = (if up { at.checked_sub(1) } else { Some(at + 1).filter(|&t| t < visible.len()) }) else { return };
        self.reorder(visible, at, to);
    }

    /// Moves the visible pin at `from` to `to`, keeping hidden pins in their slots (`arrayMove`,
    /// `reorderPins`).
    pub fn reorder(&mut self, visible: &[String], from: usize, to: usize) {
        if from >= visible.len() || to >= visible.len() || from == to {
            return;
        }
        let mut next = visible.to_vec();
        let moved = next.remove(from);
        next.insert(to, moved);
        let mut it = next.into_iter();
        for pin in &mut self.pins {
            if visible.contains(pin) {
                *pin = it.next().unwrap_or_default();
            }
        }
    }
}

const SEP: char = '\u{0}';

/// `recentSessionKey`.
pub fn recent_key(server: &str, id: &str) -> String {
    format!("{server}{SEP}{id}")
}

/// `splitRecentSessionKey`.
pub fn split_recent_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(SEP)
}
