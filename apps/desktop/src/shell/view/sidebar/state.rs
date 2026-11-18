//! What the Customize and Recent sidebars keep: Customize's view (`sidebar.view`, shared by every
//! window, `sidebar-customized.tsx:40-49`) with this window's filters and Show more counts, which
//! reset on reload; Recent's pins (`sidebar.recent`, `recent-sessions.tsx:61-64`) and how many
//! more rows Show more revealed.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::grouping::{Filters, Grouping, Ordering, Show};
use crate::settings::Choice;

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
        let flag = |k: &str, d: bool| v["show"][k].as_bool().unwrap_or(d);
        self.grouping = Grouping::parse(v["grouping"].as_str());
        self.ordering = Ordering::parse(v["ordering"].as_str());
        self.show = Show { recent: flag("recent", d.recent), subagents: flag("subagents", d.subagents), server_labels: flag("serverLabels", d.server_labels) };
        self.collapsed = v["collapsed"].as_object().into_iter().flatten().filter_map(|(k, on)| Some((k.clone(), on.as_bool()?))).collect();
    }

    /// The persisted half, for the restore stamp.
    pub fn stamp(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        (self.grouping, self.ordering, self.show, &self.collapsed).hash(h);
    }

    pub fn collapsed(&self, key: &str) -> bool {
        self.collapsed.get(key).copied().unwrap_or(false)
    }
}

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
        let mut seen = std::collections::HashSet::new();
        self.pins = v["pins"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|k| split(k).is_some_and(|(_, id)| !id.is_empty()) && seen.insert(k.to_owned())).map(str::to_owned).collect();
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
        for pin in self.pins.iter_mut() {
            if visible.contains(pin) {
                *pin = it.next().unwrap_or_default();
            }
        }
    }
}

const SEP: char = '\u{0}';

/// `recentSessionKey`.
pub fn key(server: &str, id: &str) -> String {
    format!("{server}{SEP}{id}")
}

/// `splitRecentSessionKey`.
pub fn split(key: &str) -> Option<(&str, &str)> {
    key.split_once(SEP)
}
