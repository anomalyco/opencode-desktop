//! The focused Home server's session list phase (`home/sessions/load-machine.ts`, ported
//! transition for transition). The list itself lives in the caller's per-server cache; this owns
//! only the phase. A list already shown stays on screen while its server is away.

use rune::machine::{Attempt, Effects};

use super::key::ServerKey;

/// A cached list younger than this shows without a refetch (`HOME_SESSIONS_STALE_MS`, `:3-4`).
pub const HOME_SESSIONS_STALE_MS: u64 = 30_000;
pub const HOME_SESSIONS_FETCH_TIMEOUT_MS: u64 = 30_000;
pub const HOME_SESSIONS_CONNECT_TIMEOUT_MS: u64 = 15_000;
pub const HOME_SESSIONS_PLACEHOLDER_DELAY_MS: u64 = 250;
/// The list's reveal animation (`HOME_SESSIONS_REVEAL_MS`, `:12-13`).
pub const HOME_SESSIONS_REVEAL_MS: u64 = 200;

/// Why a server's list can't load (`HomeSessionsUnavailableReason`, `:15`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unavailable {
    Offline,
    Auth,
    Incompatible,
}

/// The focused server's connection as the list sees it (`HomeSessionsTarget.connection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Connection {
    Connected,
    Connecting,
    Unavailable(Unavailable),
}

/// The focused server (`HomeSessionsTarget`, `:17-26`). `cached`: the cache holds a list, shown
/// whatever the phase; `stale`: it is missing, invalidated or older than the stale time.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub server: ServerKey,
    pub connection: Connection,
    pub cached: bool,
    pub stale: bool,
}

/// `HomeSessionsFailure` (`:28`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Failure {
    Rejected(String),
    Timeout,
}

/// `HomeSessionsLoadState` (`:30-82`). `Connecting` and `Loading` show placeholder rows once
/// `since`'s delay passes (`skeleton`) and carry it across the connect so the rows never restart.
/// `reveal` is non-zero while the list fades in over the placeholders and names the fetch that
/// revealed it. `queued`: a refresh asked for while a fetch ran; one more runs when it settles.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Idle { issued: Attempt },
    Connecting { issued: Attempt, server: ServerKey, attempt: Attempt, since: Attempt, skeleton: bool },
    Loading { issued: Attempt, server: ServerKey, attempt: Attempt, since: Attempt, skeleton: bool, queued: bool },
    Ready { issued: Attempt, server: ServerKey, reveal: Attempt },
    Refreshing { issued: Attempt, server: ServerKey, attempt: Attempt, reveal: Attempt, queued: bool },
    Unavailable { issued: Attempt, server: ServerKey, reason: Unavailable },
    Error { issued: Attempt, server: ServerKey, failure: Failure },
    Disposed,
}

impl Default for State {
    fn default() -> State {
        State::Idle { issued: Attempt(0) }
    }
}

/// `HomeSessionsTimer` (`:84`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Timer {
    Placeholder,
    Deadline,
    Reveal,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    PlaceholderDue { since: Attempt },
    ConnectTimedOut { attempt: Attempt },
    FetchTimedOut { attempt: Attempt },
    RevealCompleted { attempt: Attempt },
    ContextChanged(Option<Target>),
    FetchSucceeded { attempt: Attempt, reduced_motion: bool },
    FetchFailed { attempt: Attempt, message: String },
    /// The cached list was invalidated, or the app came back online while it was stale.
    Refresh(ServerKey),
    Retry,
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    FetchStart { server: ServerKey, attempt: Attempt },
    FetchCancel(ServerKey),
    ProbeServers,
    /// Arms `timer`; when it fires, step `event`.
    TimerStart { timer: Timer, delay_ms: u64, event: Event },
    TimerStop(Timer),
}

/// What the list area shows (`HomeSessionsListView`, `:119-123`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ListView {
    Loading { skeleton: bool },
    Ready { revealing: bool },
    Unavailable(Unavailable),
    Error(Failure),
}

const NO_REVEAL: Attempt = Attempt(0);

impl State {
    fn issued(&self) -> Attempt {
        match self {
            State::Idle { issued }
            | State::Connecting { issued, .. }
            | State::Loading { issued, .. }
            | State::Ready { issued, .. }
            | State::Refreshing { issued, .. }
            | State::Unavailable { issued, .. }
            | State::Error { issued, .. } => *issued,
            State::Disposed => Attempt(0),
        }
    }

    fn server(&self) -> Option<&ServerKey> {
        match self {
            State::Connecting { server, .. } | State::Loading { server, .. } | State::Ready { server, .. } | State::Refreshing { server, .. } | State::Unavailable { server, .. } | State::Error { server, .. } => Some(server),
            State::Idle { .. } | State::Disposed => None,
        }
    }

    /// `selectHomeSessionsSearchLoading` (`:183-186`).
    pub fn search_loading(&self) -> bool {
        matches!(self, State::Connecting { .. } | State::Loading { .. })
    }

    /// `selectHomeSessionsListView` (`:188-201`): `has_sessions` is whether the cache or the
    /// server's live store holds sessions to list.
    pub fn list_view(&self, has_sessions: bool) -> ListView {
        match self {
            State::Connecting { skeleton, .. } | State::Loading { skeleton, .. } => ListView::Loading { skeleton: *skeleton },
            State::Ready { reveal, .. } | State::Refreshing { reveal, .. } => ListView::Ready { revealing: *reveal != NO_REVEAL },
            State::Error { failure, .. } => ListView::Error(failure.clone()),
            State::Unavailable { reason, .. } if !has_sessions => ListView::Unavailable(*reason),
            _ => ListView::Ready { revealing: false },
        }
    }
}

/// `transitionHomeSessionsLoad` (`load-machine.ts:135-181`).
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    if *state == State::Disposed {
        return Effects::none();
    }
    let (fx, next) = match event {
        Event::Disposed => (stop(state), Some(State::Disposed)),
        Event::ContextChanged(target) => return retarget(state, target),
        Event::PlaceholderDue { since: s } => match state {
            State::Connecting { skeleton, since, .. } | State::Loading { skeleton, since, .. } if !*skeleton && *since == s => {
                *skeleton = true;
                return Effects::none();
            }
            _ => return Effects::none(),
        },
        Event::ConnectTimedOut { attempt: a } => match &*state {
            State::Connecting { issued, server, attempt, .. } if *attempt == a => (stop(state), Some(State::Unavailable { issued: *issued, server: server.clone(), reason: Unavailable::Offline })),
            _ => return Effects::none(),
        },
        Event::FetchSucceeded { attempt, reduced_motion } => return succeeded(state, attempt, reduced_motion),
        Event::FetchFailed { attempt, message } => return failed(state, attempt, Some(message)),
        Event::FetchTimedOut { attempt } => return failed(state, attempt, None),
        Event::RevealCompleted { attempt: a } => match state {
            State::Ready { reveal, .. } | State::Refreshing { reveal, .. } if *reveal != NO_REVEAL && *reveal == a => {
                *reveal = NO_REVEAL;
                return Effects::none();
            }
            _ => return Effects::none(),
        },
        Event::Refresh(server) => return refresh(state, server),
        Event::Retry => {
            let next = match &*state {
                State::Error { issued, server, .. } => load(*issued, server.clone(), Effects::none()),
                State::Unavailable { issued, server, reason: Unavailable::Offline } => connect(*issued, server.clone(), Effects::one(Effect::ProbeServers)),
                _ => return Effects::none(),
            };
            return apply(state, next);
        }
    };
    if let Some(next) = next {
        *state = next;
    }
    fx
}

fn apply(state: &mut State, (next, fx): (State, Effects<Effect>)) -> Effects<Effect> {
    *state = next;
    fx
}

/// `retarget` (`:203-244`).
fn retarget(state: &mut State, target: Option<Target>) -> Effects<Effect> {
    let Some(target) = target else {
        if matches!(state, State::Idle { .. }) {
            return Effects::none();
        }
        let fx = stop(state);
        *state = State::Idle { issued: state.issued() };
        return fx;
    };
    if state.server() != Some(&target.server) {
        return enter(state, &target);
    }
    match (target.connection, &*state) {
        (Connection::Connected, State::Connecting { issued, server, since, skeleton, .. }) => {
            let next = fetch(*issued, server.clone(), *since, *skeleton, Effects::none());
            apply(state, next)
        }
        // The cache lost the list it showed (another connection object for the server): reload.
        (Connection::Connected, State::Ready { .. }) if !target.cached => enter(state, &target),
        (Connection::Connected, State::Ready { issued, server, reveal }) if target.stale => {
            let next = refetch(*issued, server.clone(), *reveal, Effects::none());
            apply(state, next)
        }
        (Connection::Connected, State::Unavailable { .. }) => enter(state, &target),
        // A fetch in flight keeps running; a failed one waits for the user's retry.
        (Connection::Connected, _) => Effects::none(),
        (Connection::Connecting, State::Unavailable { .. }) => enter(state, &target),
        (Connection::Connecting, _) => Effects::none(),
        // A list already shown stays shown while the server is away.
        (Connection::Unavailable(_), State::Ready { .. }) => Effects::none(),
        (Connection::Unavailable(_), State::Refreshing { issued, server, reveal, .. }) => {
            *state = State::Ready { issued: *issued, server: server.clone(), reveal: *reveal };
            Effects::one(Effect::TimerStop(Timer::Deadline))
        }
        (Connection::Unavailable(reason), State::Unavailable { reason: current, .. }) if *current == reason => Effects::none(),
        (Connection::Unavailable(_), _) => enter(state, &target),
    }
}

/// A new target starts over: a list cached for it shows at once, and only a connected server is
/// asked for one (`enter`, `:246-263`).
fn enter(state: &mut State, target: &Target) -> Effects<Effect> {
    let fx = stop(state);
    let issued = state.issued();
    let server = target.server.clone();
    let next = match target.connection {
        _ if target.cached && (target.connection != Connection::Connected || !target.stale) => (State::Ready { issued, server, reveal: NO_REVEAL }, fx),
        _ if target.cached => refetch(issued, server, NO_REVEAL, fx),
        Connection::Connected => load(issued, server, fx),
        Connection::Connecting => connect(issued, server, fx),
        Connection::Unavailable(reason) => (State::Unavailable { issued, server, reason }, fx),
    };
    apply(state, next)
}

/// `refresh` (`:265-277`).
fn refresh(state: &mut State, server: ServerKey) -> Effects<Effect> {
    if state.server() != Some(&server) {
        return Effects::none();
    }
    match state {
        State::Ready { issued, server, reveal } => {
            let next = refetch(*issued, server.clone(), *reveal, Effects::none());
            apply(state, next)
        }
        State::Loading { queued, .. } | State::Refreshing { queued, .. } => {
            *queued = true;
            Effects::none()
        }
        State::Error { issued, server, .. } => {
            let next = load(*issued, server.clone(), Effects::none());
            apply(state, next)
        }
        _ => Effects::none(),
    }
}

/// `succeeded` (`:279-307`).
fn succeeded(state: &mut State, attempt: Attempt, reduced_motion: bool) -> Effects<Effect> {
    match state {
        State::Loading { issued, server, attempt: a, skeleton, queued, .. } if *a == attempt => {
            let reveal = if *skeleton && !reduced_motion { attempt } else { NO_REVEAL };
            let mut fx = placeholder_stop(*skeleton);
            if reveal != NO_REVEAL {
                fx.push(start_timer(Timer::Reveal, HOME_SESSIONS_REVEAL_MS, Event::RevealCompleted { attempt: reveal }));
            }
            if *queued {
                let next = refetch(*issued, server.clone(), reveal, fx);
                return apply(state, next);
            }
            fx.push(Effect::TimerStop(Timer::Deadline));
            *state = State::Ready { issued: *issued, server: server.clone(), reveal };
            fx
        }
        State::Refreshing { issued, server, attempt: a, reveal, queued } if *a == attempt => {
            if *queued {
                let next = refetch(*issued, server.clone(), *reveal, Effects::none());
                return apply(state, next);
            }
            *state = State::Ready { issued: *issued, server: server.clone(), reveal: *reveal };
            Effects::one(Effect::TimerStop(Timer::Deadline))
        }
        _ => Effects::none(),
    }
}

/// `failed` (`:309-336`): `message` is None for a timeout. A failed refresh keeps its list.
fn failed(state: &mut State, attempt: Attempt, message: Option<String>) -> Effects<Effect> {
    let (State::Loading { issued, server, attempt: a, .. } | State::Refreshing { issued, server, attempt: a, .. }) = &*state else { return Effects::none() };
    if *a != attempt {
        return Effects::none();
    }
    let (issued, server) = (*issued, server.clone());
    // A request that timed out may still hold the shared query; cancelling it lets the next start fresh.
    let mut settle = Effects::one(Effect::TimerStop(Timer::Deadline));
    if message.is_none() {
        settle.push(Effect::FetchCancel(server.clone()));
    }
    if let State::Refreshing { reveal, .. } = state {
        *state = State::Ready { issued, server, reveal: *reveal };
        return settle;
    }
    let State::Loading { skeleton, .. } = state else { return Effects::none() };
    let mut fx = placeholder_stop(*skeleton);
    fx.extend(settle);
    *state = State::Error { issued, server, failure: message.map_or(Failure::Timeout, Failure::Rejected) };
    fx
}

/// `load` (`:338-346`).
fn load(issued: Attempt, server: ServerKey, mut fx: Effects<Effect>) -> (State, Effects<Effect>) {
    let attempt = Attempt(issued.0 + 1);
    fx.push(placeholder(attempt));
    fetch(issued, server, attempt, false, fx)
}

/// Keeps `since` and `skeleton`, so rows shown while connecting stay through the first fetch
/// (`fetch`, `:348-361`).
fn fetch(issued: Attempt, server: ServerKey, since: Attempt, skeleton: bool, mut fx: Effects<Effect>) -> (State, Effects<Effect>) {
    let attempt = Attempt(issued.0 + 1);
    fx.extend(request(&server, attempt));
    (State::Loading { issued: attempt, server, attempt, since, skeleton, queued: false }, fx)
}

/// `refetch` (`:363-374`).
fn refetch(issued: Attempt, server: ServerKey, reveal: Attempt, mut fx: Effects<Effect>) -> (State, Effects<Effect>) {
    let attempt = Attempt(issued.0 + 1);
    fx.extend(request(&server, attempt));
    (State::Refreshing { issued: attempt, server, attempt, reveal, queued: false }, fx)
}

/// `connect` (`:376-391`).
fn connect(issued: Attempt, server: ServerKey, mut fx: Effects<Effect>) -> (State, Effects<Effect>) {
    let attempt = Attempt(issued.0 + 1);
    fx.push(placeholder(attempt));
    fx.push(start_timer(Timer::Deadline, HOME_SESSIONS_CONNECT_TIMEOUT_MS, Event::ConnectTimedOut { attempt }));
    (State::Connecting { issued: attempt, server, attempt, since: attempt, skeleton: false }, fx)
}

fn request(server: &ServerKey, attempt: Attempt) -> Effects<Effect> {
    Effects::two(Effect::FetchStart { server: server.clone(), attempt }, start_timer(Timer::Deadline, HOME_SESSIONS_FETCH_TIMEOUT_MS, Event::FetchTimedOut { attempt }))
}

fn placeholder(since: Attempt) -> Effect {
    start_timer(Timer::Placeholder, HOME_SESSIONS_PLACEHOLDER_DELAY_MS, Event::PlaceholderDue { since })
}

/// A superseded fetch keeps running and fills the cache; only its result is ignored (`stop`, `:404-414`).
fn stop(state: &State) -> Effects<Effect> {
    match state {
        State::Connecting { skeleton, .. } | State::Loading { skeleton, .. } => {
            let mut fx = placeholder_stop(*skeleton);
            fx.push(Effect::TimerStop(Timer::Deadline));
            fx
        }
        State::Refreshing { reveal, .. } => {
            let mut fx = Effects::one(Effect::TimerStop(Timer::Deadline));
            if *reveal != NO_REVEAL {
                fx.push(Effect::TimerStop(Timer::Reveal));
            }
            fx
        }
        State::Ready { reveal, .. } if *reveal != NO_REVEAL => Effects::one(Effect::TimerStop(Timer::Reveal)),
        _ => Effects::none(),
    }
}

fn placeholder_stop(skeleton: bool) -> Effects<Effect> {
    if skeleton { Effects::none() } else { Effects::one(Effect::TimerStop(Timer::Placeholder)) }
}

fn start_timer(timer: Timer, delay_ms: u64, event: Event) -> Effect {
    Effect::TimerStart { timer, delay_ms, event }
}
