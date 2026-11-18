//! What a session cost, as Desktop's data store sums it (`session.cost` in
//! `client/src/solid/data.ts`): a subagent's session its own server total, a root its own plus
//! every session of its family the app holds. Opening a session lists all its direct subagents
//! (`session.sync(id, { children: true })`), and `session.usage.updated` keeps each total current
//! while a step or a subagent runs (`events.rs`).

use std::collections::HashSet;

use opencode_kit::client::Request;
use opencode_kit::model::SessionInfo;

use crate::{App, Source};

impl App {
    /// Session `id`'s cost in USD, its family's when it's a root. The family is every session
    /// whose parents the app holds up to `id` (`resolveRoot`); read once per frame for every
    /// session, so it walks the list a level at a time instead of each session up its parents.
    pub fn session_cost(&self, id: &str) -> f64 {
        if self.sessions.len() >= 512 { return self.catalog.lookup(&self.sessions, id).map_or(0.0, |(_, cost)| cost); }
        let Some(info) = self.session(id) else { return 0.0 };
        if info.parent.is_some() {
            return info.cost;
        }
        let mut total = info.cost;
        let mut level = vec![id];
        // A parent cycle can't reach a root, which has no parent; the bound is for depth alone.
        for _ in 0..32 {
            let wide: HashSet<&str> = if level.len() > 8 { level.iter().copied().collect() } else { HashSet::new() };
            let member = |p: &str| if wide.is_empty() { level.contains(&p) } else { wide.contains(p) };
            let next: Vec<&SessionInfo> = self.sessions.iter().filter(|s| s.parent.as_deref().is_some_and(member)).collect();
            if next.is_empty() {
                break;
            }
            total += next.iter().map(|s| s.cost).sum::<f64>();
            level = next.iter().map(|s| s.id.as_str()).collect();
        }
        total
    }

    /// Lists every direct subagent session of `id`, so its family's cost counts them.
    pub(crate) fn list_children(&self, id: &str) {
        if let Source::Live { requests: Some(r), .. } = &*self.source {
            let _ = r.send(Request::Children(id.to_owned()));
        }
    }

    /// The answer to [`App::list_children`]: each subagent session joins the list, or updates
    /// the one it holds.
    pub(crate) fn children_listed(&mut self, sessions: Vec<SessionInfo>) {
        for mut info in sessions {
            match self.sessions.iter_mut().find(|x| x.id == info.id) {
                Some(old) => {
                    info.viewed = info.viewed.max(old.viewed);
                    *old = info;
                }
                None => self.sessions.push(info),
            }
        }
    }

    /// `session.usage.updated`: a session's server total moved.
    pub(crate) fn usage_updated(&mut self, sid: &str, cost: Option<f64>) -> bool {
        let Some(cost) = cost else { return false };
        self.sessions.iter_mut().find(|s| s.id == sid).is_some_and(|s| std::mem::replace(&mut s.cost, cost) != cost)
    }
}
