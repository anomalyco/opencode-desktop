//! Deleting worktrees from Settings › Worktrees, as Desktop's `settings/workspaces/delete-machine.ts`:
//! one confirmation or one batch at a time. A single worktree's confirmation first checks its
//! git status and sessions and deletes with force once that passes; Delete all and Delete
//! without sessions check each worktree as they reach it and skip one that is active, linked to
//! sessions or dirty, with a toast. A deleted row collapses over 150 ms, then stays hidden
//! (`removed`) until the inventory drops it. Attempts come from `issued`: a confirmation, each
//! check and each removal take one, and a result for an attempt no longer held is stale.
//! `step` returns the effects for the caller to run.

use std::collections::BTreeSet;

/// How long a deleted row takes to collapse (`WORKSPACE_COLLAPSE_MS`).
pub const COLLAPSE_MS: i64 = 150;
/// How long one worktree's check and removal may run (`WORKSPACE_DELETE_TIMEOUT_MS`).
pub const DELETE_TIMEOUT_MS: i64 = 60_000;
/// How long the single-worktree confirmation checks before it offers Retry (`WORKSPACE_INSPECT_TIMEOUT_MS`).
pub const INSPECT_TIMEOUT_MS: i64 = 15_000;

/// A worktree to delete (`WorkspaceTarget`).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub key: String,
    pub directory: String,
    pub project: String,
}

/// What the confirmation asked for (`WorkspaceDeleteScope`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    One,
    All,
    Unused,
}

/// A worktree's check (`WorkspaceDeleteInspection`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inspection {
    pub active: bool,
    pub linked: bool,
    pub dirty: bool,
}

/// The single-worktree confirmation's check (`WorkspaceConfirmPreflight`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preflight {
    Checking { attempt: u64 },
    Ready(Inspection),
    Failed,
}

/// Why a worktree was skipped (`WorkspaceSkipReason`).
#[derive(Clone, Debug, PartialEq)]
pub enum Skip {
    Blocked(Inspection),
    Failed(Option<String>),
    TimedOut,
}

/// One removal under way, and the worktrees after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub attempt: u64,
    force: bool,
    active: Option<String>,
    pub current: Target,
    queue: Vec<Target>,
    /// The accepted single check, which the closing dialog keeps showing.
    confirmed: Option<(u64, Inspection)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    /// Delete all or Delete without sessions waits on its confirmation.
    ConfirmingMany { attempt: u64, scope: Scope, active: Option<String>, targets: Vec<Target> },
    /// One worktree's confirmation, checking it.
    ConfirmingOne { attempt: u64, active: Option<String>, target: Target, preflight: Preflight },
    Deleting(Run),
    Collapsing(Run),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub phase: Phase,
    issued: u64,
    /// Rows that finished collapsing, hidden until the inventory drops them.
    pub removed: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    ConfirmOpened { scope: Scope, active: Option<String>, targets: Vec<Target> },
    InspectSucceeded { attempt: u64, inspection: Inspection },
    InspectFailed { attempt: u64 },
    InspectTimedOut { attempt: u64 },
    InspectRetried { attempt: u64 },
    ConfirmCancelled { attempt: u64 },
    ConfirmAccepted { attempt: u64 },
    DeleteSucceeded { attempt: u64, target: Target },
    DeleteSkipped { attempt: u64, reason: Skip },
    DeleteTimedOut { attempt: u64 },
    CollapseCompleted { attempt: u64 },
    InventorySynced { keys: Vec<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Starts the check's timeout and reads git status and sessions.
    InspectRun { attempt: u64, directory: String, active: Option<String> },
    InspectStop { attempt: u64 },
    /// Starts the attempt's timeout, then checks (unless forced) and removes the worktree.
    DeleteRun { attempt: u64, target: Target, force: bool, active: Option<String> },
    DeleteTimerStop { attempt: u64 },
    CollapseStart { attempt: u64 },
    /// Refreshes the inventories after a worktree went.
    Finalize { target: Target },
    ToastSkipped { reason: Skip },
}

impl Machine {
    /// A confirmation, check or removal is under way (`isWorkspaceDeleteBusy`).
    pub fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Whether row `key` shows Deleting, and whether it is collapsing (`workspaceRowView`).
    pub fn row(&self, key: &str) -> (bool, bool) {
        match &self.phase {
            Phase::Deleting(run) if run.current.key == key => (true, false),
            Phase::Collapsing(run) if run.current.key == key => (true, true),
            _ if self.removed.contains(key) => (true, true),
            _ => (false, false),
        }
    }

    /// What the single-worktree dialog shows (`workspaceConfirmView`): checking, failed, can
    /// delete, and the check's result.
    pub fn confirm_view(&self) -> (bool, bool, bool, Option<Inspection>) {
        match &self.phase {
            Phase::ConfirmingOne { preflight, .. } => match *preflight {
                Preflight::Checking { .. } => (true, false, false, None),
                Preflight::Failed => (false, true, false, None),
                Preflight::Ready(i) => (false, false, true, Some(i)),
            },
            Phase::Deleting(run) | Phase::Collapsing(run) => (false, false, false, run.confirmed.map(|c| c.1)),
            _ => (false, false, false, None),
        }
    }
}

/// `transitionWorkspaceDelete`.
pub fn step(state: Machine, event: Event) -> (Machine, Vec<Effect>) {
    if let Event::InventorySynced { keys } = &event {
        let mut state = state;
        state.removed.retain(|k| keys.contains(k));
        return (state, Vec::new());
    }
    // A removal that timed out and succeeded later still removed the worktree.
    if let Event::DeleteSucceeded { attempt, target } = &event {
        match &state.phase {
            Phase::Deleting(run) if run.attempt == *attempt => {}
            Phase::Collapsing(run) if run.attempt == *attempt => return (state, Vec::new()),
            _ => return (state, vec![Effect::Finalize { target: target.clone() }]),
        }
    }
    let Machine { phase, issued, removed } = state;
    match phase {
        Phase::Idle => match event {
            Event::ConfirmOpened { scope, active, targets } if !targets.is_empty() => {
                let attempt = issued + 1;
                if scope != Scope::One {
                    return (Machine { phase: Phase::ConfirmingMany { attempt, scope, active, targets }, issued: attempt, removed }, Vec::new());
                }
                let check = attempt + 1;
                let target = targets[0].clone();
                let effect = Effect::InspectRun { attempt: check, directory: target.directory.clone(), active: active.clone() };
                (Machine { phase: Phase::ConfirmingOne { attempt, active, target, preflight: Preflight::Checking { attempt: check } }, issued: check, removed }, vec![effect])
            }
            _ => (Machine { phase: Phase::Idle, issued, removed }, Vec::new()),
        },
        Phase::ConfirmingMany { attempt, scope, active, targets } => match event {
            Event::ConfirmCancelled { attempt: a } if a == attempt => (Machine { phase: Phase::Idle, issued, removed }, Vec::new()),
            Event::ConfirmAccepted { attempt: a } if a == attempt => {
                let mut queue = targets.into_iter();
                match queue.next() {
                    Some(current) => start(issued, removed, false, active, current, queue.collect(), None),
                    None => (Machine { phase: Phase::Idle, issued, removed }, Vec::new()),
                }
            }
            _ => (Machine { phase: Phase::ConfirmingMany { attempt, scope, active, targets }, issued, removed }, Vec::new()),
        },
        Phase::ConfirmingOne { attempt, active, target, preflight } => confirming_one(issued, removed, attempt, active, target, preflight, event),
        Phase::Deleting(run) => match event {
            Event::DeleteSucceeded { attempt, .. } if attempt == run.attempt => {
                let a = run.attempt;
                (Machine { phase: Phase::Collapsing(run), issued, removed }, vec![Effect::DeleteTimerStop { attempt: a }, Effect::CollapseStart { attempt: a }])
            }
            Event::DeleteSkipped { attempt, reason } if attempt == run.attempt => skip(issued, removed, run, reason),
            Event::DeleteTimedOut { attempt } if attempt == run.attempt => skip(issued, removed, run, Skip::TimedOut),
            _ => (Machine { phase: Phase::Deleting(run), issued, removed }, Vec::new()),
        },
        Phase::Collapsing(run) => match event {
            Event::CollapseCompleted { attempt } if attempt == run.attempt => {
                let mut removed = removed;
                removed.insert(run.current.key.clone());
                let target = run.current.clone();
                let (next, mut effects) = advance(issued, removed, run);
                effects.insert(0, Effect::Finalize { target });
                (next, effects)
            }
            _ => (Machine { phase: Phase::Collapsing(run), issued, removed }, Vec::new()),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn confirming_one(issued: u64, removed: BTreeSet<String>, attempt: u64, active: Option<String>, target: Target, preflight: Preflight, event: Event) -> (Machine, Vec<Effect>) {
    let keep = |issued, removed, preflight| (Machine { phase: Phase::ConfirmingOne { attempt, active: active.clone(), target: target.clone(), preflight }, issued, removed }, Vec::new());
    match (preflight, event) {
        (Preflight::Checking { attempt: check }, Event::InspectSucceeded { attempt: a, inspection }) if a == check => {
            let (m, _) = keep(issued, removed, Preflight::Ready(inspection));
            (m, vec![Effect::InspectStop { attempt: check }])
        }
        (Preflight::Checking { attempt: check }, Event::InspectFailed { attempt: a } | Event::InspectTimedOut { attempt: a }) if a == check => {
            let (m, _) = keep(issued, removed, Preflight::Failed);
            (m, vec![Effect::InspectStop { attempt: check }])
        }
        (preflight, Event::ConfirmCancelled { attempt: a }) if a == attempt => {
            let stop = match preflight {
                Preflight::Checking { attempt } => vec![Effect::InspectStop { attempt }],
                _ => Vec::new(),
            };
            (Machine { phase: Phase::Idle, issued, removed }, stop)
        }
        (Preflight::Failed, Event::InspectRetried { attempt: a }) if a == attempt => {
            let check = issued + 1;
            let effect = Effect::InspectRun { attempt: check, directory: target.directory.clone(), active: active.clone() };
            let (m, _) = keep(check, removed, Preflight::Checking { attempt: check });
            (m, vec![effect])
        }
        // The dialog disables Delete until the check passes; the machine holds the same rule.
        (Preflight::Ready(inspection), Event::ConfirmAccepted { attempt: a }) if a == attempt => {
            start(issued, removed, true, active.clone(), target.clone(), Vec::new(), Some((attempt, inspection)))
        }
        (preflight, _) => keep(issued, removed, preflight),
    }
}

fn start(issued: u64, removed: BTreeSet<String>, force: bool, active: Option<String>, current: Target, queue: Vec<Target>, confirmed: Option<(u64, Inspection)>) -> (Machine, Vec<Effect>) {
    let attempt = issued + 1;
    let effect = Effect::DeleteRun { attempt, target: current.clone(), force, active: active.clone() };
    (Machine { phase: Phase::Deleting(Run { attempt, force, active, current, queue, confirmed }), issued: attempt, removed }, vec![effect])
}

fn advance(issued: u64, removed: BTreeSet<String>, run: Run) -> (Machine, Vec<Effect>) {
    let mut queue = run.queue.into_iter();
    match queue.next() {
        Some(current) => start(issued, removed, run.force, run.active, current, queue.collect(), None),
        None => (Machine { phase: Phase::Idle, issued, removed }, Vec::new()),
    }
}

fn skip(issued: u64, removed: BTreeSet<String>, run: Run, reason: Skip) -> (Machine, Vec<Effect>) {
    let attempt = run.attempt;
    let (next, effects) = advance(issued, removed, run);
    let mut all = vec![Effect::DeleteTimerStop { attempt }, Effect::ToastSkipped { reason }];
    all.extend(effects);
    (next, all)
}
