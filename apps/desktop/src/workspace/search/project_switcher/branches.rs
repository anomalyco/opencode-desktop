//! The base-branch step of `new-session/location/dialog.tsx`. Search the project's server (not
//! only its first 50 branches). A reply for a previous query, project or opening can't install
//! its rows under a new search. Picking a branch changes the proposal on the worktrees step;
//! only picking New worktree copies it to the draft.

use opencode_kit::servers::ServerKey;
use rune::Tx;
use rune::machine::{Attempt, Seq};
use super::{App, StepMode, RowKind, SwitcherRow, location, matches_query};
use opencode_i18n::t;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum List {
    #[default]
    Idle,
    Loading(Attempt),
    Ready(Vec<String>),
    Unavailable,
}

enum Event { Search, Answer(Attempt, Option<Vec<String>>) }

impl List {
    fn step(&mut self, seq: &mut Seq, event: Event) -> Option<Attempt> {
        match event {
            Event::Search => {
                let id = seq.next();
                *self = Self::Loading(id);
                Some(id)
            }
            Event::Answer(id, result) => {
                if *self == Self::Loading(id) { *self = result.map_or(Self::Unavailable, Self::Ready); }
                None
            }
        }
    }
}

pub(super) fn rows(list: &List, query: &str) -> Vec<SwitcherRow> {
    let List::Ready(names) = list else { return Vec::new() };
    names.iter().filter(|n| matches_query(query, &[n])).map(|name| SwitcherRow {
        key: format!("branch:{name}"), title: name.clone(), subtitle: String::new(), section: "Branches", kind: RowKind::Branch,
    }).collect()
}

impl App {
    pub(super) fn location_checkout(&self, server: Option<&ServerKey>, directory: &str) -> Option<&str> {
        if let Some(key) = server.filter(|k| *k != crate::servers::runtime::active(self)) {
            return self.project_switcher.servers.iter().find(|l| &l.key == key)?.checkout_branch.as_ref().filter(|(d, _)| d == directory).map(|(_, b)| b.as_str());
        }
        self.sidebar_inbox.branches.get(directory)
    }

    pub(super) fn location_worktree_rows(&self, mut rows: Vec<SwitcherRow>, q: &str) -> Vec<SwitcherRow> {
        let StepMode::Worktrees { base, .. } = &self.project_switcher.mode else { return rows };
        if let Some(SwitcherRow { kind: RowKind::Worktree { project_id, directory, server, main: true }, .. }) = rows.first()
            && opencode_kit::model::project_git(project_id)
        {
            let branch = base.as_deref().or_else(|| self.location_checkout(server.as_ref(), directory)).unwrap_or_else(|| t!("session.location.noBranch"));
            let entry = SwitcherRow { key: "base".into(), title: t!("session.new.workspace.fromBranch", branch = branch), subtitle: t!("command.session.branch.choose").into(), section: "Worktrees", kind: RowKind::Base { directory: directory.clone() } };
            rows.insert(2, entry);
        }
        rows.retain(|r| match &r.kind {
            RowKind::Worktree { directory, main: true, server, .. } => matches_query(q, &[&r.title, directory, self.location_checkout(server.as_ref(), directory).unwrap_or("")]),
            RowKind::Base { directory } => matches_query(q, &[&r.title, directory]),
            _ => matches_query(q, &[&r.title, &r.subtitle]),
        });
        rows
    }

    pub(super) fn location_base(&mut self, directory: String, tx: &mut Tx) {
        self.location_event(location::Event::Base { directory }, tx);
        self.location_branch_search(tx);
    }

    pub(crate) fn open_git_location(&mut self, base: bool, tx: &mut Tx) {
        if base && self.draft_creates() {
            return self.open_branch_picker(tx);
        }
        let Some(p) = self.draft_project().filter(|p| opencode_kit::model::project_git(&p.id)) else { return };
        let (id, directory) = (p.id.clone(), p.directory.clone());
        self.close_branch_picker(tx);
        self.open_project_worktrees(&id, tx);
        if base {
            self.project_switcher.mode = StepMode::Branches { project: id, server: None, directory, direct: true, list: List::Idle };
            self.location_branch_search(tx);
        }
    }

    pub(super) fn location_branch_search(&mut self, tx: &mut Tx) {
        let ps = &mut *self.project_switcher;
        let StepMode::Branches { directory, server, list, .. } = &mut ps.mode else { return };
        let Some(id) = list.step(&mut ps.branch_requests, Event::Search) else { return };
        let (directory, server, query) = (directory.clone(), server.clone(), ps.search.text.clone());
        let key = server.unwrap_or_else(|| crate::servers::runtime::active(self).clone());
        tx.instant();
        let issued = crate::servers::runtime::with_server(self, &key, |s| {
            if !s.live() { return false; }
            s.job(move |srv| {
                let result = srv.branches(&directory, &query).ok();
                Box::new(move |s, _| s.location_branch_answer(id, result))
            });
            true
        });
        if issued != Some(true) { self.location_branch_answer(id, None); }
    }

    fn location_branch_answer(&mut self, id: Attempt, result: Option<Vec<String>>) {
        let ps = &mut *self.project_switcher;
        if let StepMode::Branches { list, .. } = &mut ps.mode {
            list.step(&mut ps.branch_requests, Event::Answer(id, result));
        }
    }
}
