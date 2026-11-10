//! What Home's session list shows, and in what order:
//! - Sessions an automation made (Flow's `flow: …`) don't get rows of their own: each day group
//!   ends in one quiet row per automation, "Flow · 12 running · 400 done", that opens to them.
//!   Search still finds them.
//! - While the pointer is on the list, its order holds: a session that updates keeps its row and
//!   its day, and one that's new joins the end of its group. When the pointer leaves, the rows
//!   move to their new places on the layout spring.

use std::collections::{HashMap, HashSet};

use opencode_kit::model::{Project, SessionInfo};
use opencode_i18n::t;
use rune::Tx;

use crate::{App, Route};

/// Title prefixes of sessions an automation creates, with the name its fold shows.
const AUTOMATIONS: &[(&str, &str)] = &[("flow:", "Flow")];

/// An open fold shows at most as many sessions as Home lists of a person's.
const LIMIT: usize = crate::home_search::HOME_SESSION_LIMIT;

/// The automation that made a session titled `title`, by name.
pub(crate) fn automation(title: &str) -> Option<&'static str> {
    AUTOMATIONS.iter().find(|(prefix, _)| title.starts_with(prefix)).map(|(_, name)| *name)
}

pub(crate) fn is_automation(title: &str) -> bool {
    automation(title).is_some()
}

/// One automation's sessions in a day group.
pub(crate) struct Fold<'a> {
    pub name: &'static str,
    /// Newest first (held while the pointer is on the list), at most `LIMIT`.
    pub sessions: Vec<&'a SessionInfo>,
    pub running: usize,
    pub total: usize,
}

/// A day group: Today, Yesterday, then the rest.
pub(crate) struct Group<'a> {
    pub title: &'static str,
    /// 0 today, 1 yesterday, 2 before.
    pub day: usize,
    pub rows: Vec<&'a SessionInfo>,
    pub folds: Vec<Fold<'a>>,
}

impl Group<'_> {
    /// The rows in the order they show, an open fold's after the person's own.
    pub fn shown_ids(&self, open: bool) -> impl Iterator<Item = &str> {
        let folded = self.folds.iter().filter(move |_| open).flat_map(|f| f.sessions.iter());
        self.rows.iter().chain(folded).map(|s| s.id.as_str())
    }
}

/// The list's order against the pointer (Kit's design; Desktop's list reorders under it).
#[derive(Default)]
pub(crate) enum Hold {
    /// The rows sit in their own order.
    #[default]
    Free,
    /// The pointer is on the list: `at` is each row's place and day as it came on.
    Holding { at: HashMap<String, (usize, usize)> },
    /// The pointer has left: the next poll lets the order go.
    Leaving { at: HashMap<String, (usize, usize)> },
}

impl Hold {
    /// Each row's held place and day, while the order holds.
    fn at(&self) -> Option<&HashMap<String, (usize, usize)>> {
        match self {
            Hold::Free => None,
            Hold::Holding { at } | Hold::Leaving { at } => Some(at),
        }
    }
}

impl App {
    /// Home's day groups for `selected` project (`groupSessions`).
    pub(crate) fn home_groups(&self, selected: Option<&Project>) -> Vec<Group<'_>> {
        let day = |ms: i64| {
            let t = rune::LocalTime::at(ms);
            (t.year, t.month, t.day)
        };
        let (today, yesterday) = (day(self.now_ms), day(self.now_ms - 86_400_000));
        let hold = self.home_search.hold.at();
        let bucket = |ses: &SessionInfo| {
            if let Some(&(_, b)) = hold.and_then(|h| h.get(&ses.id)) {
                return b;
            }
            match day(if ses.updated > 0 { ses.updated } else { ses.created }) {
                d if d == today => 0,
                d if d == yesterday => 1,
                _ => 2,
            }
        };
        let busy = self.busy_roots();
        let mut rows: [Vec<&SessionInfo>; 3] = Default::default();
        let mut folds: [Vec<Fold<'_>>; 3] = Default::default();
        let mut people = 0;
        for ses in self.home_roots(selected) {
            let b = bucket(ses);
            match automation(&ses.title) {
                None if people < LIMIT => {
                    people += 1;
                    rows[b].push(ses);
                }
                None => {}
                Some(name) => {
                    let fold = match folds[b].iter().position(|f| f.name == name) {
                        Some(i) => &mut folds[b][i],
                        None => {
                            folds[b].push(Fold { name, sessions: vec![], running: 0, total: 0 });
                            folds[b].last_mut().unwrap()
                        }
                    };
                    fold.total += 1;
                    fold.running += busy.contains(ses.id.as_str()) as usize;
                    fold.sessions.push(ses);
                }
            }
        }
        if let Some(h) = hold {
            let place = |ses: &&SessionInfo| h.get(&ses.id).map_or(usize::MAX, |&(at, _)| at);
            for list in rows.iter_mut().chain(folds.iter_mut().flat_map(|f| f.iter_mut().map(|f| &mut f.sessions))) {
                list.sort_by_key(place);
            }
        }
        for fold in folds.iter_mut().flatten() {
            fold.sessions.truncate(LIMIT);
        }
        let rest = if rows[0].is_empty() && folds[0].is_empty() && rows[1].is_empty() && folds[1].is_empty() {
            t!("sidebar.project.recentSessions")
        } else {
            t!("home.sessions.group.older")
        };
        let titles = [t!("home.sessions.group.today"), t!("home.sessions.group.yesterday"), rest];
        rows.into_iter()
            .zip(folds)
            .zip(titles)
            .enumerate()
            .filter(|(_, ((rows, folds), _))| !rows.is_empty() || !folds.is_empty())
            .map(|(day, ((rows, folds), title))| Group { title, day, rows, folds })
            .collect()
    }

    /// Roots with something working in their family.
    fn busy_roots(&self) -> HashSet<&str> {
        let inboxes = self.inboxes.iter().filter(|(_, i)| !i.items.is_empty() || i.compaction.is_some()).map(|(id, _)| id);
        self.running.iter().chain(inboxes).map(|id| self.root_of(id)).collect()
    }

    /// The pointer enters or leaves Home's list: entering holds its order, leaving lets it go.
    /// The rows glide to their places in the next frame's own commit, as the row the pointer left
    /// commits its hover at once in this one.
    pub(crate) fn hold_home_order(&mut self, inside: bool, tx: &mut Tx) {
        self.home_search.hold = match std::mem::take(&mut self.home_search.hold) {
            Hold::Holding { at } | Hold::Leaving { at } if inside => Hold::Holding { at },
            Hold::Holding { at } | Hold::Leaving { at } => Hold::Leaving { at },
            Hold::Free if inside => Hold::Holding { at: self.home_order() },
            Hold::Free => Hold::Free,
        };
        if !inside {
            tx.wake_after(std::time::Duration::ZERO);
        }
        tx.live_only();
    }

    fn home_order(&self) -> HashMap<String, (usize, usize)> {
        let selected = self.home_selected();
        let mut at = HashMap::new();
        for group in self.home_groups(selected) {
            for ses in group.rows.iter().chain(group.folds.iter().flat_map(|f| &f.sessions)) {
                let n = at.len();
                at.insert(ses.id.clone(), (n, group.day));
            }
        }
        at
    }

    /// Home's folds open or shut, all together.
    pub(crate) fn toggle_home_automations(&mut self) {
        self.restore.inbox.home_automations = !self.restore.inbox.home_automations;
    }

    /// Off Home nothing holds the list, and once the pointer has left it the rows glide to their
    /// places on the layout spring. True when they move.
    pub(crate) fn release_home_hold(&mut self, tx: &mut Tx) -> bool {
        let off = self.route != Route::Home || self.settings.open;
        let leaving = matches!(self.home_search.hold, Hold::Leaving { .. });
        if matches!(self.home_search.hold, Hold::Free) || !(off || leaving) {
            return false;
        }
        self.home_search.hold = Hold::Free;
        tx.animated();
        !off
    }
}
