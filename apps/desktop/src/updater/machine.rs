//! The updater's lifecycle (`gui-plugins/src/updater/machine.ts`): one machine per app process
//! that every window asks to check or install. Callers waiting on a check or an install are
//! listed by waiter id in the phase that will answer them; `issued` and each phase's `attempt`
//! fence platform results, so a stale result never moves the machine.
//!
//! Desktop's external-download mode (Beta moving to Stable) has no counterpart here: every
//! update installs on restart.

pub type Waiter = u64;
pub type Attempt = u64;

/// What windows see (`UpdaterState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// No updater runs in this build (a development build, or no release feed yet).
    Disabled,
    Idle,
    Checking,
    Downloading(String),
    UpToDate,
    Ready(String),
    Installing(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub current: String,
    /// The last attempt id issued; a result for any other attempt is stale.
    pub issued: Attempt,
}

/// A silent check, then maybe a download of a newer release, while a staged update stays
/// installable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refresh {
    Checking { attempt: Attempt, waiters: Vec<Waiter> },
    Staging { attempt: Attempt, next: String, waiters: Vec<Waiter> },
}

/// The step an installation is at: a last check for a newer release, maybe its download, then
/// the restart handoff; `Quitting` holds once the app quits into the update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Checking { attempt: Attempt, waiters: Vec<Waiter> },
    Staging { attempt: Attempt, next: String, waiters: Vec<Waiter> },
    Restarting { attempt: Attempt },
    Quitting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Disabled,
    Idle(Meta),
    Checking { meta: Meta, attempt: Attempt, waiters: Vec<Waiter> },
    Downloading { meta: Meta, attempt: Attempt, version: String, waiters: Vec<Waiter> },
    UpToDate(Meta),
    Ready { meta: Meta, version: String, refresh: Option<Refresh> },
    Installing { meta: Meta, version: String, plan: Plan, installs: Vec<Waiter> },
    Error { meta: Meta, message: String },
    Disposed(View),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    CheckRequested(Waiter),
    InstallRequested(Waiter),
    WaiterCancelled(Waiter),
    /// The feed's current release when it differs from the running one, `None` when up to date.
    CheckSucceeded(Attempt, Option<String>),
    CheckFailed(Attempt, String),
    StageSucceeded(Attempt),
    StageFailed(Attempt, String),
    RestartSucceeded(Attempt),
    RestartFailed(Attempt, String),
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Check(Attempt),
    Stage(Attempt),
    Restart(Attempt),
    Cancel(Attempt),
    PersistSet(String),
    PersistClear,
    ResolveChecks(Vec<Waiter>, View),
    ResolveInstalls(Vec<Waiter>),
    RejectInstalls(Vec<Waiter>, String),
    Log(bool, String),
}

pub type Transition = (State, Vec<Effect>);

/// The starting state: disabled without a platform; a persisted release the running build is
/// was installed, so it's forgotten.
pub fn init(current: &str, enabled: bool, persisted: Option<&str>) -> Transition {
    if !enabled {
        return (State::Disabled, Vec::new());
    }
    let installed = persisted == Some(current);
    let meta = Meta { current: current.to_owned(), issued: 0 };
    (State::Idle(meta), if installed { vec![Effect::PersistClear] } else { Vec::new() })
}

pub fn view(state: &State) -> View {
    match state {
        State::Disabled => View::Disabled,
        State::Idle(_) => View::Idle,
        State::Checking { .. } => View::Checking,
        State::Downloading { version, .. } => View::Downloading(version.clone()),
        State::UpToDate(_) => View::UpToDate,
        State::Ready { version, .. } => View::Ready(version.clone()),
        State::Installing { version, .. } => View::Installing(version.clone()),
        State::Error { message, .. } => View::Error(message.clone()),
        State::Disposed(last) => last.clone(),
    }
}

pub fn transition(state: &State, event: Event) -> Transition {
    if let State::Disposed(_) = state {
        return after_dispose(state, event);
    }
    if event == Event::Disposed {
        return dispose(state);
    }
    if *state == State::Disabled {
        return match event {
            Event::CheckRequested(w) => with_checks(state.clone(), vec![w], Vec::new()),
            Event::InstallRequested(w) => (state.clone(), vec![Effect::RejectInstalls(vec![w], NOT_READY.into())]),
            _ => (state.clone(), Vec::new()),
        };
    }
    match event {
        Event::CheckRequested(w) => check_requested(state, w),
        Event::InstallRequested(w) => install_requested(state, w),
        Event::WaiterCancelled(w) => waiter_cancelled(state, w),
        settled => {
            if attempt_of(&settled) != active_attempt(state) {
                return (state.clone(), Vec::new());
            }
            settle(state, settled)
        }
    }
}

const NOT_READY: &str = "Update is not ready to install";
const STOPPED: &str = "Updater stopped";

fn after_dispose(state: &State, event: Event) -> Transition {
    match event {
        Event::CheckRequested(w) => with_checks(state.clone(), vec![w], Vec::new()),
        Event::InstallRequested(w) => (state.clone(), vec![Effect::RejectInstalls(vec![w], STOPPED.into())]),
        _ => (state.clone(), Vec::new()),
    }
}

// Platform work still running is cancelled and every caller hears now. The restart handoff
// is left to finish.
fn dispose(state: &State) -> Transition {
    let last = view(state);
    let mut effects = Vec::new();
    if let Some(attempt) = active_attempt(state)
        && !matches!(state, State::Installing { plan: Plan::Restarting { .. }, .. })
    {
        effects.push(Effect::Cancel(attempt));
    }
    if let State::Installing { installs, .. } = state
        && !installs.is_empty()
    {
        effects.push(Effect::RejectInstalls(installs.clone(), STOPPED.into()));
    }
    with_checks(State::Disposed(last), check_waiters(state), effects)
}

fn meta(state: &State) -> Meta {
    match state {
        State::Idle(m) | State::UpToDate(m) => m.clone(),
        State::Checking { meta, .. } | State::Downloading { meta, .. } | State::Ready { meta, .. } | State::Installing { meta, .. } | State::Error { meta, .. } => meta.clone(),
        State::Disabled | State::Disposed(_) => Meta { current: String::new(), issued: 0 },
    }
}

fn issue(state: &State) -> (Meta, Attempt) {
    let mut m = meta(state);
    m.issued += 1;
    let attempt = m.issued;
    (m, attempt)
}

fn check_requested(state: &State, w: Waiter) -> Transition {
    match state {
        State::Idle(_) | State::UpToDate(_) | State::Error { .. } => {
            let (meta, attempt) = issue(state);
            (State::Checking { meta, attempt, waiters: vec![w] }, vec![Effect::Check(attempt)])
        }
        State::Checking { meta, attempt, waiters } => (State::Checking { meta: meta.clone(), attempt: *attempt, waiters: plus(waiters, w) }, Vec::new()),
        State::Downloading { meta, attempt, version, waiters } => {
            (State::Downloading { meta: meta.clone(), attempt: *attempt, version: version.clone(), waiters: plus(waiters, w) }, Vec::new())
        }
        // A staged update stays on screen while a silent check runs; later checks join it.
        State::Ready { meta, version, refresh: Some(refresh) } => {
            let refresh = match refresh {
                Refresh::Checking { attempt, waiters } => Refresh::Checking { attempt: *attempt, waiters: plus(waiters, w) },
                Refresh::Staging { attempt, next, waiters } => Refresh::Staging { attempt: *attempt, next: next.clone(), waiters: plus(waiters, w) },
            };
            (State::Ready { meta: meta.clone(), version: version.clone(), refresh: Some(refresh) }, Vec::new())
        }
        State::Ready { version, refresh: None, .. } => {
            let (meta, attempt) = issue(state);
            (State::Ready { meta, version: version.clone(), refresh: Some(Refresh::Checking { attempt, waiters: vec![w] }) }, vec![Effect::Check(attempt)])
        }
        // An installation is underway: report it without checking again.
        _ => with_checks(state.clone(), vec![w], Vec::new()),
    }
}

// The installation claims the updater in the same event, so a check that starts meanwhile never
// runs a second platform check. A silent check or download already running becomes its own.
fn install_requested(state: &State, w: Waiter) -> Transition {
    match state {
        // The app already quits into the update and nothing settles this step again.
        State::Installing { plan: Plan::Quitting, .. } => (state.clone(), vec![Effect::ResolveInstalls(vec![w])]),
        State::Installing { meta, version, plan, installs } => (State::Installing { meta: meta.clone(), version: version.clone(), plan: plan.clone(), installs: plus(installs, w) }, Vec::new()),
        State::Ready { meta, version, refresh: Some(refresh) } => {
            let plan = match refresh.clone() {
                Refresh::Checking { attempt, waiters } => Plan::Checking { attempt, waiters },
                Refresh::Staging { attempt, next, waiters } => Plan::Staging { attempt, next, waiters },
            };
            (State::Installing { meta: meta.clone(), version: version.clone(), plan, installs: vec![w] }, Vec::new())
        }
        State::Ready { version, refresh: None, .. } => {
            let (meta, attempt) = issue(state);
            (State::Installing { meta, version: version.clone(), plan: Plan::Checking { attempt, waiters: Vec::new() }, installs: vec![w] }, vec![Effect::Check(attempt)])
        }
        _ => (state.clone(), vec![Effect::RejectInstalls(vec![w], NOT_READY.into())]),
    }
}

// A caller that stops waiting leaves the shared work running for everyone else.
fn waiter_cancelled(state: &State, w: Waiter) -> Transition {
    let mut next = state.clone();
    match &mut next {
        State::Checking { waiters, .. } | State::Downloading { waiters, .. } => waiters.retain(|x| *x != w),
        State::Ready { refresh: Some(Refresh::Checking { waiters, .. } | Refresh::Staging { waiters, .. }), .. } => waiters.retain(|x| *x != w),
        State::Installing { plan, installs, .. } => {
            installs.retain(|x| *x != w);
            if let Plan::Checking { waiters, .. } | Plan::Staging { waiters, .. } = plan {
                waiters.retain(|x| *x != w);
            }
        }
        _ => {}
    }
    (next, Vec::new())
}

fn settle(state: &State, event: Event) -> Transition {
    match state {
        State::Checking { meta, waiters, .. } => match event {
            Event::CheckSucceeded(_, target) => discovered(state, target, waiters.clone()),
            Event::CheckFailed(_, message) => with_checks(State::Error { meta: meta.clone(), message }, waiters.clone(), Vec::new()),
            _ => (state.clone(), Vec::new()),
        },
        State::Downloading { meta, version, waiters, .. } => match event {
            Event::StageSucceeded(_) => with_checks(State::Ready { meta: meta.clone(), version: version.clone(), refresh: None }, waiters.clone(), vec![Effect::PersistSet(version.clone())]),
            Event::StageFailed(_, message) => with_checks(State::Error { meta: meta.clone(), message }, waiters.clone(), Vec::new()),
            _ => (state.clone(), Vec::new()),
        },
        State::Ready { meta, version, refresh: Some(refresh) } => refresh_settled(meta, version, refresh, state, event),
        State::Installing { meta, version, plan, installs } => restart_settled(meta, version, plan, installs, state, event),
        _ => (state.clone(), Vec::new()),
    }
}

fn refresh_settled(meta: &Meta, version: &str, refresh: &Refresh, state: &State, event: Event) -> Transition {
    let ready = State::Ready { meta: meta.clone(), version: version.to_owned(), refresh: None };
    match (refresh, event) {
        (Refresh::Checking { waiters, .. }, Event::CheckSucceeded(_, target)) => {
            if keeps_staged(meta, version, target.as_deref()) {
                return with_checks(ready, waiters.clone(), Vec::new());
            }
            let next = target.unwrap_or_default();
            let (meta, attempt) = issue(state);
            (State::Ready { meta, version: version.to_owned(), refresh: Some(Refresh::Staging { attempt, next, waiters: waiters.clone() }) }, vec![Effect::Stage(attempt)])
        }
        (Refresh::Staging { next, waiters, .. }, Event::StageSucceeded(_)) => {
            with_checks(State::Ready { meta: meta.clone(), version: next.clone(), refresh: None }, waiters.clone(), vec![Effect::PersistSet(next.clone())])
        }
        (Refresh::Checking { waiters, .. } | Refresh::Staging { waiters, .. }, Event::CheckFailed(_, message) | Event::StageFailed(_, message)) => {
            with_checks(ready, waiters.clone(), vec![staged_refresh_failed(version, &message)])
        }
        _ => (state.clone(), Vec::new()),
    }
}

// The installation hands off only for a release it keeps or has just staged.
fn restart_settled(meta: &Meta, version: &str, plan: &Plan, installs: &[Waiter], state: &State, event: Event) -> Transition {
    match (plan, event) {
        (Plan::Checking { waiters, .. }, Event::CheckSucceeded(_, target)) => {
            if keeps_staged(meta, version, target.as_deref()) {
                return restart(state, version, waiters.clone(), Vec::new());
            }
            let next = target.unwrap_or_else(|| version.to_owned());
            let (meta, attempt) = issue(state);
            let plan = Plan::Staging { attempt, next, waiters: waiters.clone() };
            (State::Installing { meta, version: version.to_owned(), plan, installs: installs.to_vec() }, vec![Effect::Stage(attempt)])
        }
        // The staged release still installs when the last-minute check or download fails.
        (Plan::Checking { waiters, .. }, Event::CheckFailed(_, message)) | (Plan::Staging { waiters, .. }, Event::StageFailed(_, message)) => {
            restart(state, version, waiters.clone(), vec![staged_refresh_failed(version, &message)])
        }
        (Plan::Staging { next, waiters, .. }, Event::StageSucceeded(_)) => restart(state, next, waiters.clone(), vec![Effect::PersistSet(next.clone())]),
        (Plan::Restarting { .. }, Event::RestartSucceeded(_)) => {
            (State::Installing { meta: meta.clone(), version: version.to_owned(), plan: Plan::Quitting, installs: Vec::new() }, vec![Effect::ResolveInstalls(installs.to_vec())])
        }
        // The app keeps running, so the user can retry.
        (Plan::Restarting { .. }, Event::RestartFailed(_, message)) => {
            (State::Ready { meta: meta.clone(), version: version.to_owned(), refresh: None }, vec![Effect::RejectInstalls(installs.to_vec(), message)])
        }
        _ => (state.clone(), Vec::new()),
    }
}

// What a fresh check found when no staged update is kept.
fn discovered(state: &State, target: Option<String>, waiters: Vec<Waiter>) -> Transition {
    let m = meta(state);
    match target {
        Some(version) if version != m.current => {
            let (meta, attempt) = issue(state);
            (State::Downloading { meta, attempt, version, waiters }, vec![Effect::Stage(attempt)])
        }
        _ => with_checks(State::UpToDate(m), waiters, vec![Effect::PersistClear]),
    }
}

fn restart(state: &State, version: &str, waiters: Vec<Waiter>, mut effects: Vec<Effect>) -> Transition {
    let (meta, attempt) = issue(state);
    let installs = match state {
        State::Installing { installs, .. } => installs.clone(),
        _ => Vec::new(),
    };
    effects.push(Effect::Restart(attempt));
    with_checks(State::Installing { meta, version: version.to_owned(), plan: Plan::Restarting { attempt }, installs }, waiters, effects)
}

fn keeps_staged(meta: &Meta, staged: &str, target: Option<&str>) -> bool {
    target.is_none_or(|t| t == staged || t == meta.current)
}

fn attempt_of(event: &Event) -> Option<Attempt> {
    match event {
        Event::CheckSucceeded(a, _) | Event::CheckFailed(a, _) | Event::StageSucceeded(a) | Event::StageFailed(a, _) | Event::RestartSucceeded(a) | Event::RestartFailed(a, _) => Some(*a),
        _ => None,
    }
}

fn active_attempt(state: &State) -> Option<Attempt> {
    match state {
        State::Checking { attempt, .. } | State::Downloading { attempt, .. } => Some(*attempt),
        State::Ready { refresh: Some(Refresh::Checking { attempt, .. } | Refresh::Staging { attempt, .. }), .. } => Some(*attempt),
        State::Installing { plan: Plan::Checking { attempt, .. } | Plan::Staging { attempt, .. } | Plan::Restarting { attempt }, .. } => Some(*attempt),
        _ => None,
    }
}

fn check_waiters(state: &State) -> Vec<Waiter> {
    match state {
        State::Checking { waiters, .. } | State::Downloading { waiters, .. } => waiters.clone(),
        State::Ready { refresh: Some(Refresh::Checking { waiters, .. } | Refresh::Staging { waiters, .. }), .. } => waiters.clone(),
        State::Installing { plan: Plan::Checking { waiters, .. } | Plan::Staging { waiters, .. }, .. } => waiters.clone(),
        _ => Vec::new(),
    }
}

fn staged_refresh_failed(staged: &str, message: &str) -> Effect {
    Effect::Log(true, format!("updater refresh failed, keeping staged update {staged}: {message}"))
}

fn plus(waiters: &[Waiter], w: Waiter) -> Vec<Waiter> {
    let mut out = waiters.to_vec();
    out.push(w);
    out
}

// Answers `waiters` with what the next state shows, after the transition's other effects.
fn with_checks(state: State, waiters: Vec<Waiter>, mut effects: Vec<Effect>) -> Transition {
    if !waiters.is_empty() {
        effects.push(Effect::ResolveChecks(waiters, view(&state)));
    }
    (state, effects)
}
