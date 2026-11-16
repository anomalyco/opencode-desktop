//! The updater's lifecycle (`gui-plugins/src/updater/machine.ts`): one machine per app process
//! that every window asks to check or install. Callers waiting on a check or an install are
//! listed by waiter id in the phase that will answer them; `issued` and each phase's `attempt`
//! fence platform results, so a stale result never moves the machine.
//!
//! Updates come in Desktop's two modes (`UpdateTarget`): `Restart`, which downloads and stages
//! the release then restarts into it (macOS, Windows), and `External`, a release this install
//! can't put in place by itself, whose download the user opens (`download-required`; Linux, where
//! the AppImage, `.deb` and `.rpm` update by hand).
//!
//! The machine starts in `Starting` while the worker thread finds out whether this build updates
//! at all (Desktop knows that at build time; here it reads the install and its signature, which
//! stays off the UI thread). Checks asked meanwhile run once it's up.

pub type Waiter = u64;
pub type Attempt = u64;

/// What a check found newer than the running build (`machine.ts` `UpdateTarget`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Already published at the install path; this process still runs the previous image.
    Installed(String),
    /// Download and stage it, then restart into it.
    Restart(String),
    /// Offer its download, at `url`.
    External { version: String, url: String },
}

/// What windows see (`UpdaterState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// The worker is still finding out whether this build updates.
    Starting,
    /// No updater runs in this build (a development build, a build the release's publisher
    /// didn't sign).
    Disabled,
    Idle,
    Checking,
    Downloading(String),
    UpToDate,
    Installed(String),
    Ready(String),
    /// A newer release to download by hand.
    DownloadRequired(String),
    Installing(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub current: String,
    /// The last attempt id issued; a result for any other attempt is stale.
    pub issued: Attempt,
    /// The platform can open an external download (`canExternalInstall`).
    pub external: bool,
}

/// A silent check while an external download stays offered (`ExternalRefresh`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalRefresh {
    pub attempt: Attempt,
    pub waiters: Vec<Waiter>,
}

/// A silent check, then maybe a download of a newer release, while a staged update stays
/// installable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refresh {
    Checking { attempt: Attempt, waiters: Vec<Waiter> },
    Staging { attempt: Attempt, next: String, waiters: Vec<Waiter> },
}

/// The step an installation is at (`InstallingPlan`). A restart: a last check for a newer
/// release, maybe its download, then the restart handoff; `Quitting` holds once the app quits
/// into the update. An external install: a last check that its download is still current
/// (`ExternalChecking`), then opening it (`Opening`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Checking { attempt: Attempt, waiters: Vec<Waiter> },
    Staging { attempt: Attempt, next: String, waiters: Vec<Waiter> },
    /// Everything slow or fallible before the quit (signature checks, the helper's copy), while
    /// the windows are still there to show a failure: a quit must end visibly or not start.
    Preparing { attempt: Attempt },
    /// Staged and verified, but no helper, bundle swap or restart is authorized yet.
    AwaitingSave { attempt: Attempt },
    Restarting { attempt: Attempt },
    Quitting,
    ExternalChecking { url: String, attempt: Attempt, waiters: Vec<Waiter> },
    Opening { url: String, attempt: Attempt },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Starting { waiters: Vec<Waiter> },
    Disabled,
    Idle(Meta),
    Checking { meta: Meta, attempt: Attempt, waiters: Vec<Waiter> },
    Downloading { meta: Meta, attempt: Attempt, version: String, waiters: Vec<Waiter> },
    UpToDate(Meta),
    Installed { meta: Meta, version: String },
    Ready { meta: Meta, version: String, refresh: Option<Refresh> },
    DownloadRequired { meta: Meta, version: String, url: String, refresh: Option<ExternalRefresh> },
    Installing { meta: Meta, version: String, plan: Plan, installs: Vec<Waiter> },
    Error { meta: Meta, message: String },
    Disposed(View),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    SaveConfirmed,
    QuitResumed,
    /// The worker made the platform updater: the running build's version, and the release an
    /// earlier run staged (`ready`), if any, and whether it can open an external download.
    Started { current: String, persisted: Option<String>, external: bool },
    /// This build has no updater.
    Unavailable,
    CheckRequested(Waiter),
    InstallRequested(Waiter),
    WaiterCancelled(Waiter),
    /// The feed's current release when it differs from the running one, `None` when up to date.
    CheckSucceeded(Attempt, Option<Target>),
    CheckFailed(Attempt, String),
    StageSucceeded(Attempt),
    StageFailed(Attempt, String),
    PrepareSucceeded(Attempt),
    PrepareFailed(Attempt, String),
    RestartSucceeded(Attempt),
    RestartFailed(Attempt, String),
    ExternalSucceeded(Attempt),
    ExternalFailed(Attempt, String),
    /// An out-of-process update helper failed to put the staged bundle in place.
    ApplyFailed(String),
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    RequestSave,
    Check(Attempt),
    Stage(Attempt),
    Prepare(Attempt),
    Restart(Attempt),
    /// Open the external download at the URL.
    External(Attempt, String),
    Cancel(Attempt),
    PersistSet(String),
    PersistClear,
    ResolveChecks(Vec<Waiter>, View),
    ResolveInstalls(Vec<Waiter>),
    RejectInstalls(Vec<Waiter>, String),
    Log(bool, String),
}

pub type Transition = (State, Vec<Effect>);

/// The starting state, until the worker says whether this build updates.
pub fn init() -> State {
    State::Starting { waiters: Vec::new() }
}

pub fn view(state: &State) -> View {
    match state {
        State::Starting { .. } => View::Starting,
        State::Disabled => View::Disabled,
        State::Idle(_) => View::Idle,
        State::Checking { .. } => View::Checking,
        State::Downloading { version, .. } => View::Downloading(version.clone()),
        State::UpToDate(_) => View::UpToDate,
        State::Installed { version, .. } => View::Installed(version.clone()),
        State::Ready { version, .. } => View::Ready(version.clone()),
        State::DownloadRequired { version, .. } => View::DownloadRequired(version.clone()),
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
    if let Event::ApplyFailed(message) = event {
        return match state {
            State::Idle(meta) => (State::Error { meta: meta.clone(), message }, Vec::new()),
            _ => (state.clone(), Vec::new()),
        };
    }
    if let State::Starting { waiters } = state {
        return starting(waiters, event);
    }
    if *state == State::Disabled {
        return match event {
            Event::CheckRequested(w) => with_checks(state.clone(), vec![w], Vec::new()),
            Event::InstallRequested(w) => (state.clone(), vec![Effect::RejectInstalls(vec![w], NOT_READY.into())]),
            _ => (state.clone(), Vec::new()),
        };
    }
    match event {
        Event::SaveConfirmed => match state {
            State::Installing { meta, version, plan: Plan::AwaitingSave { attempt }, installs } => (State::Installing { meta: meta.clone(), version: version.clone(), plan: Plan::Restarting { attempt: *attempt }, installs: installs.clone() }, vec![Effect::Restart(*attempt)]),
            _ => (state.clone(), Vec::new()),
        },
        Event::QuitResumed => match state {
            State::Installing { meta, version, plan: Plan::AwaitingSave { .. }, installs } => (State::Ready { meta: meta.clone(), version: version.clone(), refresh: None }, vec![Effect::RejectInstalls(installs.clone(), "Restart cancelled; the update remains staged".into())]),
            _ => (state.clone(), Vec::new()),
        },
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
const EXTERNAL_UNAVAILABLE: &str = "External installer is unavailable";

// A persisted release the running build is was installed, so it's forgotten. Checks asked while
// starting run now.
fn starting(waiters: &[Waiter], event: Event) -> Transition {
    match event {
        Event::Started { current, persisted, external } => {
            let mut effects = if persisted.as_deref() == Some(current.as_str()) { vec![Effect::PersistClear] } else { Vec::new() };
            let idle = State::Idle(Meta { current, issued: 0, external });
            if waiters.is_empty() {
                return (idle, effects);
            }
            let (meta, attempt) = issue(&idle);
            effects.push(Effect::Check(attempt));
            (State::Checking { meta, attempt, waiters: waiters.to_vec() }, effects)
        }
        Event::Unavailable => with_checks(State::Disabled, waiters.to_vec(), Vec::new()),
        Event::CheckRequested(w) => (State::Starting { waiters: plus(waiters, w) }, Vec::new()),
        Event::InstallRequested(w) => (State::Starting { waiters: waiters.to_vec() }, vec![Effect::RejectInstalls(vec![w], NOT_READY.into())]),
        Event::WaiterCancelled(w) => (State::Starting { waiters: waiters.iter().copied().filter(|x| *x != w).collect() }, Vec::new()),
        _ => (State::Starting { waiters: waiters.to_vec() }, Vec::new()),
    }
}

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
        State::Installed { meta, .. } | State::Checking { meta, .. }
        | State::Downloading { meta, .. }
        | State::Ready { meta, .. }
        | State::DownloadRequired { meta, .. }
        | State::Installing { meta, .. }
        | State::Error { meta, .. } => meta.clone(),
        State::Starting { .. } | State::Disabled | State::Disposed(_) => Meta { current: String::new(), issued: 0, external: false },
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
        State::Idle(_) | State::UpToDate(_) | State::Installed { .. } | State::Error { .. } => {
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
        // An offered download stays on screen too.
        State::DownloadRequired { meta, version, url, refresh: Some(r) } => {
            let refresh = ExternalRefresh { attempt: r.attempt, waiters: plus(&r.waiters, w) };
            (State::DownloadRequired { meta: meta.clone(), version: version.clone(), url: url.clone(), refresh: Some(refresh) }, Vec::new())
        }
        State::DownloadRequired { version, url, refresh: None, .. } => {
            let (meta, attempt) = issue(state);
            let refresh = ExternalRefresh { attempt, waiters: vec![w] };
            (State::DownloadRequired { meta, version: version.clone(), url: url.clone(), refresh: Some(refresh) }, vec![Effect::Check(attempt)])
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
        State::DownloadRequired { meta, .. } if !meta.external => (state.clone(), vec![Effect::RejectInstalls(vec![w], EXTERNAL_UNAVAILABLE.into())]),
        // A silent check already running becomes the install's last check.
        State::DownloadRequired { meta, version, url, refresh: Some(r) } => {
            let plan = Plan::ExternalChecking { url: url.clone(), attempt: r.attempt, waiters: r.waiters.clone() };
            (State::Installing { meta: meta.clone(), version: version.clone(), plan, installs: vec![w] }, Vec::new())
        }
        State::DownloadRequired { version, url, refresh: None, .. } => {
            let (meta, attempt) = issue(state);
            let plan = Plan::ExternalChecking { url: url.clone(), attempt, waiters: Vec::new() };
            (State::Installing { meta, version: version.clone(), plan, installs: vec![w] }, vec![Effect::Check(attempt)])
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
        State::DownloadRequired { refresh: Some(r), .. } => r.waiters.retain(|x| *x != w),
        State::Installing { plan, installs, .. } => {
            installs.retain(|x| *x != w);
            if let Plan::Checking { waiters, .. } | Plan::Staging { waiters, .. } | Plan::ExternalChecking { waiters, .. } = plan {
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
        State::DownloadRequired { meta, version, url, refresh: Some(r) } => match event {
            Event::CheckSucceeded(_, target) => discovered(state, target, r.waiters.clone()),
            Event::CheckFailed(_, message) => {
                let offered = State::DownloadRequired { meta: meta.clone(), version: version.clone(), url: url.clone(), refresh: None };
                with_checks(offered, r.waiters.clone(), vec![external_refresh_failed(&message)])
            }
            _ => (state.clone(), Vec::new()),
        },
        State::Installing { meta, version, plan: plan @ (Plan::ExternalChecking { .. } | Plan::Opening { .. }), installs } => external_settled(meta, version, plan, installs, state, event),
        State::Installing { meta, version, plan, installs } => restart_settled(meta, version, plan, installs, state, event),
        _ => (state.clone(), Vec::new()),
    }
}

fn refresh_settled(meta: &Meta, version: &str, refresh: &Refresh, state: &State, event: Event) -> Transition {
    let ready = State::Ready { meta: meta.clone(), version: version.to_owned(), refresh: None };
    match (refresh, event) {
        (Refresh::Checking { waiters, .. }, Event::CheckSucceeded(_, target)) => {
            if keeps_staged(meta, version, target.as_ref()) {
                return with_checks(ready, waiters.clone(), Vec::new());
            }
            let Some(Target::Restart(next)) = target else { return discovered(state, target, waiters.clone()) };
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
            if keeps_staged(meta, version, target.as_ref()) {
                return restart(state, version, waiters.clone(), Vec::new());
            }
            // A check that finds an external download instead cancels the restart, so the app
            // never quits into a superseded staged update.
            let next = match target {
                Some(Target::Restart(next)) => next,
                external => {
                    let (next, mut effects) = discovered(state, external, waiters.clone());
                    effects.push(Effect::ResolveInstalls(installs.to_vec()));
                    return (next, effects);
                }
            };
            let (meta, attempt) = issue(state);
            let plan = Plan::Staging { attempt, next, waiters: waiters.clone() };
            (State::Installing { meta, version: version.to_owned(), plan, installs: installs.to_vec() }, vec![Effect::Stage(attempt)])
        }
        // The staged release still installs when the last-minute check or download fails.
        (Plan::Checking { waiters, .. }, Event::CheckFailed(_, message)) | (Plan::Staging { waiters, .. }, Event::StageFailed(_, message)) => {
            restart(state, version, waiters.clone(), vec![staged_refresh_failed(version, &message)])
        }
        (Plan::Staging { next, waiters, .. }, Event::StageSucceeded(_)) => restart(state, next, waiters.clone(), vec![Effect::PersistSet(next.clone())]),
        (Plan::Preparing { .. }, Event::PrepareSucceeded(_)) => {
            let (meta, attempt) = issue(state);
            (State::Installing { meta, version: version.to_owned(), plan: Plan::AwaitingSave { attempt }, installs: installs.to_vec() }, vec![Effect::RequestSave])
        }
        // Nothing has quit: the app keeps running and says why, so the user can retry.
        (Plan::Preparing { .. }, Event::PrepareFailed(_, message)) => {
            (State::Ready { meta: meta.clone(), version: version.to_owned(), refresh: None }, vec![Effect::RejectInstalls(installs.to_vec(), message)])
        }
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

// The external installer opens the current download only. Once it opened, or failed to, the
// download stays offered: the app keeps running, so the user can open it again.
fn external_settled(meta: &Meta, version: &str, plan: &Plan, installs: &[Waiter], state: &State, event: Event) -> Transition {
    let offered = |version: &str, url: &str| State::DownloadRequired { meta: meta.clone(), version: version.to_owned(), url: url.to_owned(), refresh: None };
    match (plan, event) {
        (Plan::ExternalChecking { waiters, .. }, Event::CheckSucceeded(_, Some(Target::External { version, url }))) => open(state, &version, &url, waiters.clone(), Vec::new()),
        (Plan::ExternalChecking { waiters, .. }, Event::CheckSucceeded(_, target)) => {
            let (next, mut effects) = discovered(state, target, waiters.clone());
            effects.push(Effect::RejectInstalls(installs.to_vec(), EXTERNAL_UNAVAILABLE.into()));
            (next, effects)
        }
        (Plan::ExternalChecking { url, waiters, .. }, Event::CheckFailed(_, message)) => open(state, version, url, waiters.clone(), vec![external_refresh_failed(&message)]),
        (Plan::Opening { url, .. }, Event::ExternalSucceeded(_)) => (offered(version, url), vec![Effect::ResolveInstalls(installs.to_vec())]),
        (Plan::Opening { url, .. }, Event::ExternalFailed(_, message)) => (offered(version, url), vec![Effect::RejectInstalls(installs.to_vec(), message)]),
        _ => (state.clone(), Vec::new()),
    }
}

// What a fresh check found when no staged update is kept.
fn discovered(state: &State, target: Option<Target>, waiters: Vec<Waiter>) -> Transition {
    let m = meta(state);
    match target {
        Some(Target::Installed(version)) => with_checks(State::Installed { meta: m, version }, waiters, vec![Effect::PersistClear]),
        Some(Target::Restart(version)) if version != m.current => {
            let (meta, attempt) = issue(state);
            (State::Downloading { meta, attempt, version, waiters }, vec![Effect::Stage(attempt)])
        }
        Some(Target::External { version, url }) => with_checks(State::DownloadRequired { meta: m, version, url, refresh: None }, waiters, vec![Effect::PersistClear]),
        _ => with_checks(State::UpToDate(m), waiters, vec![Effect::PersistClear]),
    }
}

fn open(state: &State, version: &str, url: &str, waiters: Vec<Waiter>, mut effects: Vec<Effect>) -> Transition {
    let (meta, attempt) = issue(state);
    let installs = match state {
        State::Installing { installs, .. } => installs.clone(),
        _ => Vec::new(),
    };
    effects.push(Effect::External(attempt, url.to_owned()));
    with_checks(State::Installing { meta, version: version.to_owned(), plan: Plan::Opening { url: url.to_owned(), attempt }, installs }, waiters, effects)
}

fn restart(state: &State, version: &str, waiters: Vec<Waiter>, mut effects: Vec<Effect>) -> Transition {
    let (meta, attempt) = issue(state);
    let installs = match state {
        State::Installing { installs, .. } => installs.clone(),
        _ => Vec::new(),
    };
    effects.push(Effect::Prepare(attempt));
    with_checks(State::Installing { meta, version: version.to_owned(), plan: Plan::Preparing { attempt }, installs }, waiters, effects)
}

fn keeps_staged(meta: &Meta, staged: &str, target: Option<&Target>) -> bool {
    match target {
        None => true,
        Some(Target::Restart(t)) => t == staged || *t == meta.current,
        Some(Target::External { .. } | Target::Installed(_)) => false,
    }
}

fn attempt_of(event: &Event) -> Option<Attempt> {
    match event {
        Event::CheckSucceeded(a, _)
        | Event::CheckFailed(a, _)
        | Event::StageSucceeded(a)
        | Event::StageFailed(a, _)
        | Event::PrepareSucceeded(a)
        | Event::PrepareFailed(a, _)
        | Event::RestartSucceeded(a)
        | Event::RestartFailed(a, _)
        | Event::ExternalSucceeded(a)
        | Event::ExternalFailed(a, _) => Some(*a),
        _ => None,
    }
}

fn active_attempt(state: &State) -> Option<Attempt> {
    match state {
        State::Checking { attempt, .. } | State::Downloading { attempt, .. } => Some(*attempt),
        State::Ready { refresh: Some(Refresh::Checking { attempt, .. } | Refresh::Staging { attempt, .. }), .. } => Some(*attempt),
        State::DownloadRequired { refresh: Some(r), .. } => Some(r.attempt),
        State::Installing {
            plan: Plan::Checking { attempt, .. } | Plan::Staging { attempt, .. } | Plan::Preparing { attempt } | Plan::Restarting { attempt } | Plan::ExternalChecking { attempt, .. } | Plan::Opening { attempt, .. },
            ..
        } => Some(*attempt),
        _ => None,
    }
}

fn check_waiters(state: &State) -> Vec<Waiter> {
    match state {
        State::Starting { waiters } | State::Checking { waiters, .. } | State::Downloading { waiters, .. } => waiters.clone(),
        State::Ready { refresh: Some(Refresh::Checking { waiters, .. } | Refresh::Staging { waiters, .. }), .. } => waiters.clone(),
        State::DownloadRequired { refresh: Some(r), .. } => r.waiters.clone(),
        State::Installing { plan: Plan::Checking { waiters, .. } | Plan::Staging { waiters, .. } | Plan::ExternalChecking { waiters, .. }, .. } => waiters.clone(),
        _ => Vec::new(),
    }
}

fn staged_refresh_failed(staged: &str, message: &str) -> Effect {
    Effect::Log(true, format!("updater refresh failed, keeping staged update {staged}: {message}"))
}

fn external_refresh_failed(message: &str) -> Effect {
    Effect::Log(true, format!("external updater refresh failed, keeping download available: {message}"))
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
