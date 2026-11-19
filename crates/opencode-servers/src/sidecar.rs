//! The connection to the app's own local service that every window shares
//! (`desktop/src/main/service/background-service-machine.ts`, ported transition for transition).
//! Each attempt to reach it has an id, and a result of any other attempt is stale. Callers that
//! arrive while an attempt runs wait for that attempt, so windows that start or reconnect
//! together run one `ensure`.

use opencode_kit::servers::HttpBase;
use rune::machine::{Attempt, Effects};

/// `initial` may adopt a service a probe found; `reconnect` always ensures one (`ConnectMode`, `:8-9`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectMode {
    Initial,
    Reconnect,
}

/// One caller waiting on `connection` or `reconnect`, named by the caller (`Reply`, `:11-15`).
pub type ReplyId = u64;

/// `State` (`background-service-machine.ts:17-43`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// No service has been reached yet.
    Connecting { attempt: Attempt, waiters: Vec<ReplyId> },
    /// `data` is the active service; a probed one is still verified by `attempt`'s ensure.
    Ready { attempt: Attempt, data: HttpBase },
    /// Requests keep using `data` until the reconnect replaces it. `waiters` asked to reconnect
    /// and share its outcome; `joiners` opened a window meanwhile, and a failed reconnect answers
    /// them with `data`.
    Reconnecting { attempt: Attempt, data: HttpBase, waiters: Vec<ReplyId>, joiners: Vec<ReplyId> },
    /// The last attempt failed; the next caller starts a new one. `last` is the service reached
    /// before it, if any.
    Failed { attempt: Attempt, error: String, last: Option<HttpBase> },
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    ConnectionRequested(ReplyId),
    ReconnectRequested(ReplyId),
    WaiterCancelled(ReplyId),
    ProbeSucceeded { attempt: Attempt, data: HttpBase },
    ConnectSucceeded { attempt: Attempt, data: HttpBase },
    ConnectFailed { attempt: Attempt, error: String },
    VerifySucceeded { attempt: Attempt, data: HttpBase },
    VerifyFailed { attempt: Attempt, error: String },
    Disposed,
}

/// `Command` (`:56-61`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Connect { attempt: Attempt, mode: ConnectMode },
    Verify { attempt: Attempt },
    /// The active service changed: every window moves to it.
    Publish(HttpBase),
    Resolve { replies: Vec<ReplyId>, data: HttpBase },
    Reject { replies: Vec<ReplyId>, error: String },
}

/// `DISPOSED_ERROR` (`:68`).
pub const DISPOSED_ERROR: &str = "Background service is disposed";

impl State {
    /// `initialTransition` (`:70-75`): the first connect starts at once.
    pub fn new() -> (State, Effects<Effect>) {
        (State::Connecting { attempt: Attempt(1), waiters: Vec::new() }, Effects::one(Effect::Connect { attempt: Attempt(1), mode: ConnectMode::Initial }))
    }

    /// The service every window uses now, if one was reached.
    pub fn data(&self) -> Option<&HttpBase> {
        match self {
            State::Ready { data, .. } | State::Reconnecting { data, .. } => Some(data),
            _ => None,
        }
    }
}

/// `transition` (`background-service-machine.ts:77-119`).
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    if *state == State::Disposed {
        return match event {
            Event::ConnectionRequested(r) | Event::ReconnectRequested(r) => Effects::one(Effect::Reject { replies: vec![r], error: DISPOSED_ERROR.into() }),
            _ => Effects::none(),
        };
    }
    if event == Event::Disposed {
        let pending = match std::mem::replace(state, State::Disposed) {
            State::Connecting { waiters, .. } => waiters,
            State::Reconnecting { waiters, joiners, .. } => [waiters, joiners].concat(),
            _ => Vec::new(),
        };
        return reject(pending, DISPOSED_ERROR);
    }
    match state {
        State::Connecting { .. } => connecting(state, event),
        State::Reconnecting { .. } => reconnecting(state, event),
        State::Ready { .. } => ready(state, event),
        State::Failed { attempt, last, .. } => {
            let (Event::ConnectionRequested(reply) | Event::ReconnectRequested(reply)) = event else { return Effects::none() };
            let attempt = Attempt(attempt.0 + 1);
            let reconnect = matches!(event, Event::ReconnectRequested(_));
            // A service reached before is replaced the way a reconnect replaces it; otherwise this
            // is a new first connect, so a since-fixed failure isn't replayed (`:101-118`).
            *state = match last.take() {
                Some(data) => State::Reconnecting {
                    attempt,
                    data,
                    waiters: if reconnect { vec![reply] } else { Vec::new() },
                    joiners: if reconnect { Vec::new() } else { vec![reply] },
                },
                None => State::Connecting { attempt, waiters: vec![reply] },
            };
            let mode = if matches!(state, State::Reconnecting { .. }) { ConnectMode::Reconnect } else { ConnectMode::Initial };
            Effects::one(Effect::Connect { attempt, mode })
        }
        State::Disposed => Effects::none(),
    }
}

/// No service was reached yet, so every caller shares the attempt's outcome (`:121-158`).
fn connecting(state: &mut State, event: Event) -> Effects<Effect> {
    let State::Connecting { attempt, waiters } = state else { return Effects::none() };
    let attempt = *attempt;
    match event {
        Event::ConnectionRequested(r) | Event::ReconnectRequested(r) => {
            waiters.push(r);
            Effects::none()
        }
        Event::WaiterCancelled(r) => {
            waiters.retain(|w| *w != r);
            Effects::none()
        }
        Event::ProbeSucceeded { attempt: a, data } if a == attempt => {
            let waiters = std::mem::take(waiters);
            *state = State::Ready { attempt, data: data.clone() };
            let mut fx = Effects::one(Effect::Publish(data.clone()));
            fx.extend(resolve(waiters, &data));
            fx.push(Effect::Verify { attempt });
            fx
        }
        Event::ConnectSucceeded { attempt: a, data } if a == attempt => {
            let waiters = std::mem::take(waiters);
            *state = State::Ready { attempt, data: data.clone() };
            let mut fx = Effects::one(Effect::Publish(data.clone()));
            fx.extend(resolve(waiters, &data));
            fx
        }
        Event::ConnectFailed { attempt: a, error } if a == attempt => {
            let waiters = std::mem::take(waiters);
            *state = State::Failed { attempt, error: error.clone(), last: None };
            reject(waiters, &error)
        }
        _ => Effects::none(),
    }
}

/// `reconnecting` (`:160-196`).
fn reconnecting(state: &mut State, event: Event) -> Effects<Effect> {
    let State::Reconnecting { attempt, data, waiters, joiners } = state else { return Effects::none() };
    let attempt = *attempt;
    match event {
        Event::ReconnectRequested(r) => {
            waiters.push(r);
            Effects::none()
        }
        Event::ConnectionRequested(r) => {
            joiners.push(r);
            Effects::none()
        }
        Event::WaiterCancelled(r) => {
            waiters.retain(|w| *w != r);
            joiners.retain(|w| *w != r);
            Effects::none()
        }
        Event::ConnectSucceeded { attempt: a, data: next } if a == attempt => {
            let mut fx = Effects::none();
            if *data != next {
                fx.push(Effect::Publish(next.clone()));
            }
            let replies = [std::mem::take(waiters), std::mem::take(joiners)].concat();
            fx.extend(resolve(replies, &next));
            *state = State::Ready { attempt, data: next };
            fx
        }
        // A window that only asked for the connection still gets the last service; only the
        // windows that asked to reconnect see the failure (`:190-195`).
        Event::ConnectFailed { attempt: a, error } if a == attempt => {
            let (data, waiters, joiners) = (data.clone(), std::mem::take(waiters), std::mem::take(joiners));
            let mut fx = resolve(joiners, &data);
            fx.extend(reject(waiters, &error));
            *state = State::Failed { attempt, error, last: Some(data) };
            fx
        }
        _ => Effects::none(),
    }
}

/// `ready` (`:198-221`).
fn ready(state: &mut State, event: Event) -> Effects<Effect> {
    let State::Ready { attempt, data } = state else { return Effects::none() };
    match event {
        Event::ConnectionRequested(r) => Effects::one(Effect::Resolve { replies: vec![r], data: data.clone() }),
        // The new attempt also supersedes a verification still running for the current one.
        Event::ReconnectRequested(r) => {
            let attempt = Attempt(attempt.0 + 1);
            *state = State::Reconnecting { attempt, data: data.clone(), waiters: vec![r], joiners: Vec::new() };
            Effects::one(Effect::Connect { attempt, mode: ConnectMode::Reconnect })
        }
        // `ensure` replaced the probed service (it had exited or was incompatible): move every
        // window to it. A failed verification keeps the probed one, which answered the probe.
        Event::VerifySucceeded { attempt: a, data: next } if a == *attempt && next != *data => {
            *data = next.clone();
            Effects::one(Effect::Publish(next))
        }
        _ => Effects::none(),
    }
}

fn resolve(replies: Vec<ReplyId>, data: &HttpBase) -> Effects<Effect> {
    if replies.is_empty() { Effects::none() } else { Effects::one(Effect::Resolve { replies, data: data.clone() }) }
}

fn reject(replies: Vec<ReplyId>, error: &str) -> Effects<Effect> {
    if replies.is_empty() { Effects::none() } else { Effects::one(Effect::Reject { replies, error: error.to_owned() }) }
}
