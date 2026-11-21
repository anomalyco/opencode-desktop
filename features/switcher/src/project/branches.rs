//! The base-branch step of `new-session/location/dialog.tsx`. Search the project's server (not
//! only its first 50 branches). A reply for a previous query, project or opening can't install
//! its rows under a new search. Picking a branch changes the proposal on the worktrees step;
//! only picking New worktree copies it to the draft.

use opencode_kit::model::project_git;
use opencode_kit::servers::ServerKey;
use rune::Tx;
use rune::machine::{Attempt, Seq};

use super::{ProjectSwitcherHost, RowKind, StepMode, SwitcherRow, matches_query, open_worktrees};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum List {
    #[default]
    Idle,
    Loading(Attempt),
    Ready(Vec<String>),
    Unavailable,
}

enum Event {
    Search,
    Answer(Attempt, Option<Vec<String>>),
}

impl List {
    fn step(&mut self, seq: &mut Seq, event: Event) -> Option<Attempt> {
        match event {
            Event::Search => {
                let id = seq.next();
                *self = Self::Loading(id);
                Some(id)
            }
            Event::Answer(id, result) => {
                if *self == Self::Loading(id) {
                    *self = result.map_or(Self::Unavailable, Self::Ready);
                }
                None
            }
        }
    }
}

pub(super) fn rows(list: &List, query: &str) -> Vec<SwitcherRow> {
    let List::Ready(names) = list else { return Vec::new() };
    names
        .iter()
        .filter(|n| matches_query(query, &[n]))
        .map(|name| SwitcherRow {
            key: format!("branch:{name}"),
            title: name.clone(),
            subtitle: String::new(),
            section: "Branches",
            kind: RowKind::Branch,
        })
        .collect()
}

pub(super) fn checkout<'a>(host: &'a dyn ProjectSwitcherHost, server: Option<&ServerKey>, directory: &str) -> Option<&'a str> {
    if let Some(key) = server.filter(|k| **k != host.active_server()) {
        return host
            .project_switcher()
            .servers()
            .iter()
            .find(|l| &l.key == key)?
            .checkout_branch
            .as_ref()
            .filter(|(d, _)| d == directory)
            .map(|(_, b)| b.as_str());
    }
    host.checkout_branch(directory)
}

/// Filters the worktrees step by query (`new-session/location/dialog.tsx:180-202`). Base branch
/// selection lives in its own `BaseBranchPicker` popover (`features/workspace-tools/src/new_worktree`)
/// once New worktree is chosen, rather than as a row inside the worktree picker.
pub(super) fn worktree_rows(host: &dyn ProjectSwitcherHost, mut rows: Vec<SwitcherRow>, q: &str) -> Vec<SwitcherRow> {
    if !matches!(host.project_switcher().mode(), Some(StepMode::Worktrees { .. })) {
        return rows;
    }
    rows.retain(|r| match &r.kind {
        RowKind::Worktree { directory, main: true, server, .. } => matches_query(q, &[&r.title, directory, checkout(host, server.as_ref(), directory).unwrap_or("")]),
        _ => matches_query(q, &[&r.title, &r.subtitle]),
    });
    rows
}

pub fn open_git_location(host: &mut dyn ProjectSwitcherHost, base: bool, tx: &mut Tx) {
    if base && host.draft_creates() {
        host.open_branch_picker(tx);
        return;
    }
    let Some((id, directory)) = host.draft_project().filter(|p| project_git(&p.id)).map(|p| (p.id.clone(), p.directory.clone())) else {
        return;
    };
    host.close_branch_picker(tx);
    open_worktrees(host, &id, tx);
    if base {
        if let Some(open) = host.project_switcher_mut().state.open_state_mut() {
            open.mode = StepMode::Branches {
                project: id,
                server: None,
                directory,
                direct: true,
                list: List::Idle,
            };
        }
        search(host, tx);
    }
}

pub(super) fn search(host: &mut dyn ProjectSwitcherHost, tx: &mut Tx) {
    if !matches!(host.project_switcher().mode(), Some(StepMode::Branches { .. })) {
        return;
    }
    let (id, directory, server, query) = {
        let ps = host.project_switcher_mut();
        let Some(open) = ps.state.open_state_mut() else { return };
        let StepMode::Branches { directory, server, list, .. } = &mut open.mode else { return };
        let Some(id) = list.step(&mut ps.branch_requests, Event::Search) else { return };
        (id, directory.clone(), server.clone(), ps.search.text.clone())
    };
    tx.instant();
    if !host.fetch_branches(server.as_ref(), directory, query, id) {
        apply_answer(host, id, None);
    }
}

/// Installs an asynchronous branch-search answer for `id` if it matches the active attempt.
pub fn apply_answer(host: &mut dyn ProjectSwitcherHost, id: Attempt, result: Option<Vec<String>>) {
    if !matches!(host.project_switcher().mode(), Some(StepMode::Branches { .. })) {
        return;
    }
    let ps = host.project_switcher_mut();
    if let Some(open) = ps.state.open_state_mut()
        && let StepMode::Branches { list, .. } = &mut open.mode
    {
        list.step(&mut ps.branch_requests, Event::Answer(id, result));
    }
}
