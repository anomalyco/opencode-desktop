//! A server's event stream as its data sync sees it, and the endpoint its transport uses
//! (`runtime/server/server-sync/machine.ts` and `endpoint-machine.ts`, ported transition for
//! transition). Each live server runs one of each.

use rune::machine::{Attempt, Effects};

use super::registry::HttpBase;

/// The event stream's status as the client reports it (`ClientConnectionStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamStatus {
    Connecting,
    Connected,
    Disconnected,
}

/// `ConnectionSyncState` (`server-sync/machine.ts:3-13`). A stream is live once the client
/// reports it connected and its `server.connected` handshake has arrived, in either order.
/// `stale`: an earlier stream already reported connected, so the bootstrap query may hold a fetch
/// it started, and going live restarts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Connection {
    #[default]
    Idle,
    Disconnected { stale: bool, handshake: bool },
    AwaitingHandshake { stale: bool },
    Live,
    Disposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectionEvent {
    StatusChanged(StreamStatus),
    HandshakeReceived,
    Disposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectionEffect {
    /// Drop the server's cached queries.
    Invalidate,
    /// Fetch the bootstrap data; `restart` cancels one a stale stream started.
    Sync { restart: bool },
}

/// `transitionConnectionSync` (`server-sync/machine.ts:31-70`).
pub fn step_connection(state: &mut Connection, event: ConnectionEvent) -> Effects<ConnectionEffect> {
    use Connection as S;
    if *state == S::Disposed {
        return Effects::none();
    }
    match event {
        ConnectionEvent::Disposed => {
            *state = S::Disposed;
            Effects::none()
        }
        ConnectionEvent::HandshakeReceived => match *state {
            S::Idle => {
                *state = S::Disconnected { stale: false, handshake: true };
                Effects::none()
            }
            S::AwaitingHandshake { stale } => {
                *state = S::Live;
                Effects::one(ConnectionEffect::Sync { restart: stale })
            }
            S::Disconnected { stale, handshake: false } => {
                *state = S::Disconnected { stale, handshake: true };
                Effects::none()
            }
            _ => Effects::none(),
        },
        ConnectionEvent::StatusChanged(StreamStatus::Connected) => match *state {
            S::Idle => {
                *state = S::AwaitingHandshake { stale: false };
                Effects::none()
            }
            S::Disconnected { stale, handshake: true } => {
                *state = S::Live;
                Effects::one(ConnectionEffect::Sync { restart: stale })
            }
            S::Disconnected { stale, handshake: false } => {
                *state = S::AwaitingHandshake { stale };
                Effects::none()
            }
            _ => Effects::none(),
        },
        ConnectionEvent::StatusChanged(_) => {
            if matches!(state, S::Disconnected { .. }) {
                return Effects::none();
            }
            *state = S::Disconnected { stale: *state != S::Idle, handshake: false };
            Effects::one(ConnectionEffect::Invalidate)
        }
    }
}

/// `EndpointSyncState` (`endpoint-machine.ts:3-10`): the endpoint a server's transport uses. The
/// stream's reconnect resolves one and a plugin can report another at any time; a resolution the
/// plugin overtook while it ran never replaces the newer endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Idle { http: HttpBase, attempt: Attempt },
    Resolving { http: HttpBase, attempt: Attempt },
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EndpointEvent {
    Reported(HttpBase),
    ResolveStarted,
    ResolveSucceeded { attempt: Attempt, http: HttpBase },
    ResolveFailed { attempt: Attempt },
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EndpointEffect {
    TransportUpdate(HttpBase),
    StreamResync,
}

impl Endpoint {
    /// `createEndpointSyncState` (`endpoint-machine.ts:28-30`).
    pub fn new(http: HttpBase) -> Endpoint {
        Endpoint::Idle { http, attempt: Attempt(0) }
    }

    /// The resolution to stamp a `resolve` with, while one runs.
    pub fn resolving(&self) -> Option<Attempt> {
        match self {
            Endpoint::Resolving { attempt, .. } => Some(*attempt),
            _ => None,
        }
    }
}

/// `transitionEndpointSync` (`endpoint-machine.ts:32-58`).
pub fn step_endpoint(state: &mut Endpoint, event: EndpointEvent) -> Effects<EndpointEffect> {
    let (http, attempt) = match &*state {
        Endpoint::Disposed => return Effects::none(),
        Endpoint::Idle { http, attempt } | Endpoint::Resolving { http, attempt } => (http.clone(), *attempt),
    };
    match event {
        EndpointEvent::Disposed => {
            *state = Endpoint::Disposed;
            Effects::none()
        }
        EndpointEvent::ResolveStarted => {
            *state = Endpoint::Resolving { http, attempt: Attempt(attempt.0 + 1) };
            Effects::none()
        }
        // A resolution still running is now stale: its result arrives in `Idle` and is ignored.
        EndpointEvent::Reported(next) => {
            if next == http {
                return Effects::none();
            }
            *state = Endpoint::Idle { http: next.clone(), attempt };
            Effects::two(EndpointEffect::TransportUpdate(next), EndpointEffect::StreamResync)
        }
        EndpointEvent::ResolveSucceeded { attempt: a, .. } | EndpointEvent::ResolveFailed { attempt: a } if state.resolving() != Some(a) => Effects::none(),
        EndpointEvent::ResolveSucceeded { http: next, .. } if next != http => {
            *state = Endpoint::Idle { http: next.clone(), attempt };
            Effects::one(EndpointEffect::TransportUpdate(next))
        }
        EndpointEvent::ResolveSucceeded { .. } | EndpointEvent::ResolveFailed { .. } => {
            *state = Endpoint::Idle { http, attempt };
            Effects::none()
        }
    }
}
