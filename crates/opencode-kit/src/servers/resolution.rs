//! One routed session's resolution against its server (`session/resolution-machine.ts`, ported
//! transition for transition), and what the route shows given the server's health
//! ([`route_view`]): a cached session stays on screen while its server is away; an uncached one
//! on an unreachable server explains itself after 30 s instead of waiting blank.
//!
//! `attempt` fences a metadata sync and the timeout to the target they started for: it advances
//! when the target changes, a pending session starts or settles, a reconnect retries an
//! unresolved target, or the route unmounts. A timeout keeps the attempt, as the request it gave
//! up on may still answer.

use rune::machine::{Attempt, Effects};

use super::health::{Health, ProbeResult};
use super::key::ServerKey;

/// `SESSION_RESOLUTION_TIMEOUT_MS` (`resolution-machine.ts:5-6`).
pub const SESSION_RESOLUTION_TIMEOUT_MS: u64 = 30_000;

/// A routed session: its id on the server whose store resolves it (`SessionTarget`, `:8-12`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub server: ServerKey,
    pub session_id: String,
}

/// A metadata sync's failure, classified by the caller (`isSessionNotFoundError`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SyncError {
    NotFound,
    Other(String),
}

/// `SessionResolutionState` (`:14-35`). `S` is the session's metadata, `P` the tab promotion
/// that still owns a new session's creation (`Preparing`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State<S, P> {
    Idle { attempt: Attempt },
    Preparing { target: Target, pending: P, attempt: Attempt },
    Resolving { target: Target, attempt: Attempt },
    Unreachable { target: Target, attempt: Attempt },
    Ready { target: Target, session: S, attempt: Attempt },
    NotFound { target: Target, attempt: Attempt },
    Error { target: Target, error: String, attempt: Attempt },
}

/// What a newly routed or settled target finds: the cached session, and whether the server is
/// connected and its data needs a refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found<S> {
    pub cached: Option<S>,
    pub connected: bool,
    pub refresh: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event<S, P> {
    TargetChanged { target: Option<Target>, pending: Option<P>, found: Found<S> },
    PendingChanged { target: Target, pending: Option<P>, found: Found<S> },
    CacheChanged { target: Target, session: Option<S> },
    Connected { target: Target, refresh: bool },
    SyncSettled { target: Target, attempt: Attempt, session: Option<S> },
    SyncFailed { target: Target, attempt: Attempt, error: SyncError },
    ResolveTimedOut { attempt: Attempt },
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    SyncTranscript(Target),
    SyncMetadata { target: Target, attempt: Attempt },
    TimerStart { attempt: Attempt, delay_ms: u64 },
    TimerClear,
}

/// What the routed session shows (`SessionRouteView`, `:82-91`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View<S, P> {
    Idle,
    Incompatible { session_id: String },
    Preparing { session_id: String, pending: P },
    SignedOut { session_id: String },
    Unreachable { session_id: String },
    Resolving { session_id: String },
    Ready { session_id: String, session: S },
    NotFound { session_id: String },
    Error { session_id: String, error: String },
}

impl<S, P> State<S, P> {
    pub fn new() -> State<S, P> {
        State::Idle { attempt: Attempt(0) }
    }

    pub fn attempt(&self) -> Attempt {
        match self {
            State::Idle { attempt }
            | State::Preparing { attempt, .. }
            | State::Resolving { attempt, .. }
            | State::Unreachable { attempt, .. }
            | State::Ready { attempt, .. }
            | State::NotFound { attempt, .. }
            | State::Error { attempt, .. } => *attempt,
        }
    }

    pub fn target(&self) -> Option<&Target> {
        match self {
            State::Idle { .. } => None,
            State::Preparing { target, .. }
            | State::Resolving { target, .. }
            | State::Unreachable { target, .. }
            | State::Ready { target, .. }
            | State::NotFound { target, .. }
            | State::Error { target, .. } => Some(target),
        }
    }
}

impl<S, P> Default for State<S, P> {
    fn default() -> Self {
        State::new()
    }
}

/// `transitionSessionResolution` (`resolution-machine.ts:103-149`).
pub fn step<S: Clone + PartialEq, P: Clone + PartialEq>(state: &mut State<S, P>, event: Event<S, P>) -> Effects<Effect> {
    match event {
        Event::Disposed => released(state),
        Event::TargetChanged { target: None, .. } => released(state),
        Event::TargetChanged { target: Some(target), pending, found } => {
            if state.target() == Some(&target) {
                return Effects::none();
            }
            let attempt = Attempt(state.attempt().0 + 1);
            match pending {
                Some(pending) => changed(state, State::Preparing { target, pending, attempt }, Effects::none()),
                None => opened(state, target, attempt, found),
            }
        }
        // Same attempt: the metadata request in flight may answer late, and still decides (`:111-116`).
        Event::ResolveTimedOut { attempt } => match state {
            State::Resolving { target, attempt: a } if *a == attempt => {
                let target = target.clone();
                changed(state, State::Unreachable { target, attempt }, Effects::none())
            }
            _ => Effects::none(),
        },
        Event::PendingChanged { target, pending, found } => {
            if state.target() != Some(&target) {
                return Effects::none();
            }
            pending_changed(state, target, pending, found)
        }
        Event::CacheChanged { target, .. } | Event::Connected { target, .. } | Event::SyncSettled { target, .. } | Event::SyncFailed { target, .. }
            if state.target() != Some(&target) || matches!(state, State::Preparing { .. }) =>
        {
            // The tab promotion owns a preparing session: neither its early cache entry nor a
            // reconnect starts loading before the promotion settles (`:122-124`).
            Effects::none()
        }
        Event::CacheChanged { session, .. } => cache_changed(state, session),
        Event::Connected { refresh, .. } => reconnected(state, refresh),
        Event::SyncSettled { attempt, session, .. } => {
            let Some(target) = answerable(state, attempt) else { return Effects::none() };
            match session {
                None => changed(state, State::NotFound { target, attempt }, Effects::none()),
                Some(session) => changed(state, State::Ready { target, session, attempt }, Effects::none()),
            }
        }
        Event::SyncFailed { attempt, error, .. } => {
            let Some(target) = answerable(state, attempt) else { return Effects::none() };
            match error {
                SyncError::NotFound => changed(state, State::NotFound { target, attempt }, Effects::none()),
                SyncError::Other(error) => changed(state, State::Error { target, error, attempt }, Effects::none()),
            }
        }
    }
}

/// Only the sync started for the current attempt answers a target still resolving, or that timed
/// out waiting for it: once the cache holds the session it is authoritative (`:130-134`).
fn answerable<S, P>(state: &State<S, P>, attempt: Attempt) -> Option<Target> {
    match state {
        State::Resolving { target, attempt: a } | State::Unreachable { target, attempt: a } if *a == attempt => Some(target.clone()),
        _ => None,
    }
}

/// `sessionRouteView` (`:151-179`).
pub fn route_view<S: Clone, P: Clone>(state: &State<S, P>, health: Option<&Health>) -> View<S, P> {
    let Some(target) = state.target() else { return View::Idle };
    let session_id = target.session_id.clone();
    // A server the app can't talk to wins over everything, a session still being created included.
    if health == Some(&Health::Incompatible) {
        return View::Incompatible { session_id };
    }
    match state {
        State::Preparing { pending, .. } => return View::Preparing { session_id, pending: pending.clone() },
        State::Ready { session, .. } => return View::Ready { session_id, session: session.clone() },
        State::NotFound { .. } => return View::NotFound { session_id },
        State::Error { error, .. } => return View::Error { session_id, error: error.clone() },
        _ => {}
    }
    // Nothing is cached, and a server waiting for sign-in can't resolve it until the user signs in.
    if matches!(health, Some(Health::Auth | Health::Probe(ProbeResult::Unauthorized))) {
        return View::SignedOut { session_id };
    }
    // Nothing is cached, and an unreachable, stopped or silent server never resolves it.
    if matches!(state, State::Unreachable { .. }) || matches!(health, Some(Health::Probe(ProbeResult::Unreachable) | Health::Stopped)) {
        return View::Unreachable { session_id };
    }
    View::Resolving { session_id }
}

/// `viewedSessionID` (`:194-197`): only a shown session is the one the user looks at.
pub fn viewed_session<S, P>(view: &View<S, P>) -> Option<&str> {
    match view {
        View::Ready { session_id, .. } => Some(session_id),
        _ => None,
    }
}

/// `pendingChanged` (`:221-240`).
fn pending_changed<S: Clone + PartialEq, P: Clone + PartialEq>(state: &mut State<S, P>, target: Target, pending: Option<P>, found: Found<S>) -> Effects<Effect> {
    let attempt = Attempt(state.attempt().0 + 1);
    match pending {
        Some(pending) => {
            if matches!(state, State::Preparing { pending: p, .. } if *p == pending) {
                return Effects::none();
            }
            changed(state, State::Preparing { target, pending, attempt }, Effects::none())
        }
        // The promotion settled: the created session is usually cached, so it shows at once.
        None if matches!(state, State::Preparing { .. }) => opened(state, target, attempt, found),
        None => Effects::none(),
    }
}

/// `opened` (`:242-255`): independent reads start before the view, including when metadata is
/// cached but the transcript never loaded.
fn opened<S: Clone + PartialEq, P: Clone + PartialEq>(state: &mut State<S, P>, target: Target, attempt: Attempt, found: Found<S>) -> Effects<Effect> {
    let fx = if found.connected { syncs(&target, attempt, found.cached.is_none() || found.refresh) } else { Effects::none() };
    match found.cached {
        None => changed(state, State::Resolving { target, attempt }, fx),
        Some(session) => changed(state, State::Ready { target, session, attempt }, fx),
    }
}

/// `cacheChanged` (`:257-271`): losing a resolved session from the cache means it was deleted.
fn cache_changed<S: Clone + PartialEq, P: Clone + PartialEq>(state: &mut State<S, P>, session: Option<S>) -> Effects<Effect> {
    let (Some(target), attempt) = (state.target().cloned(), state.attempt()) else { return Effects::none() };
    match session {
        None if matches!(state, State::Ready { .. }) => changed(state, State::NotFound { target, attempt }, Effects::none()),
        None => Effects::none(),
        Some(s) if matches!(state, State::Ready { session, .. } if *session == s) => Effects::none(),
        Some(session) => changed(state, State::Ready { target, session, attempt }, Effects::none()),
    }
}

/// `reconnected` (`:273-284`): a resolved session keeps its content while it refreshes; any other
/// target retries from scratch.
fn reconnected<S: Clone + PartialEq, P: Clone + PartialEq>(state: &mut State<S, P>, refresh: bool) -> Effects<Effect> {
    let Some(target) = state.target().cloned() else { return Effects::none() };
    if matches!(state, State::Ready { .. }) {
        return syncs(&target, state.attempt(), refresh);
    }
    let attempt = Attempt(state.attempt().0 + 1);
    let fx = syncs(&target, attempt, true);
    changed(state, State::Resolving { target, attempt }, fx)
}

/// `released` (`:286-290`).
fn released<S, P>(state: &mut State<S, P>) -> Effects<Effect> {
    if matches!(state, State::Idle { .. }) {
        return Effects::none();
    }
    let attempt = Attempt(state.attempt().0 + 1);
    changed(state, State::Idle { attempt }, Effects::none())
}

/// The timeline owns transcript errors, so only metadata reports back (`syncs`, `:292-304`).
fn syncs(target: &Target, attempt: Attempt, metadata: bool) -> Effects<Effect> {
    if !metadata {
        return Effects::one(Effect::SyncTranscript(target.clone()));
    }
    Effects::two(Effect::SyncTranscript(target.clone()), Effect::SyncMetadata { target: target.clone(), attempt })
}

/// Owns the timeout: every entry into `Resolving` starts it for the new attempt, and every exit
/// clears it (`changed`, `:310-326`).
fn changed<S, P>(state: &mut State<S, P>, next: State<S, P>, mut fx: Effects<Effect>) -> Effects<Effect> {
    let was_resolving = matches!(state, State::Resolving { .. });
    if let State::Resolving { attempt, .. } = &next {
        fx.push(Effect::TimerStart { attempt: *attempt, delay_ms: SESSION_RESOLUTION_TIMEOUT_MS });
    } else if was_resolving {
        fx.push(Effect::TimerClear);
    }
    *state = next;
    fx
}
