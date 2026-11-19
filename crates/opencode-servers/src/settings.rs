//! Whether one server's per-server Settings pages can load, and their Retry button
//! (`settings/servers/machine.ts`, ported). Every Settings surface reads [`availability`] rather
//! than its own predicate; [`step`] fences Retry to the server the page shows.

use opencode_kit::servers::key::ServerKey;
use rune::machine::{Attempt, Effects};

use crate::health::{Health, ProbeResult};
use crate::plugin_server::{PluginStage, PluginStatus};
use crate::registry::ServerConn;

/// Longer than a health probe can take, so a Retry whose probe never answers still enables
/// again (`SERVER_RETRY_TIMEOUT_MS`, `machine.ts:5-6`).
pub const SERVER_RETRY_TIMEOUT_MS: u64 = 35_000;

/// Why a server's pages can't load (`reason`, `:17-21`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    Offline,
    Incompatible,
    Stopped,
}

/// `SettingsServerAvailability` (`:8-21`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    /// `health` is healthy, checking, or not known yet.
    Ready { connection: ServerConn, health: Option<Health> },
    Starting,
    Unauthorized { connection: Option<ServerConn> },
    Unreachable { reason: Reason, connection: Option<ServerConn> },
}

/// `ServerAvailabilityInput` (`:23-29`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Input<'a> {
    pub connection: Option<&'a ServerConn>,
    /// The plugin entry of a server not listed yet: ready, or its stage.
    pub source: Option<Option<PluginStage>>,
    pub health: Option<&'a Health>,
    /// The server rejected our credentials, so it has no controller until it signs in again.
    pub signed_out: bool,
}

/// `resolveServerAvailability` (`:54-79`).
pub fn availability(input: Input<'_>) -> Availability {
    // A listed plugin server's status already shows a sign-in as starting and a ready entry
    // without an endpoint as failed; the plugin entry speaks only for servers not listed yet.
    if let Some(ServerConn::Plugin { status: PluginStatus::Stage(stage), .. }) = input.connection {
        return staged(*stage);
    }
    let Some(conn) = input.connection else {
        if let Some(Some(stage)) = input.source {
            return staged(stage);
        }
        return Availability::Unreachable { reason: Reason::Offline, connection: None };
    };
    let connection = Some(conn.clone());
    match input.health {
        _ if input.signed_out => Availability::Unauthorized { connection },
        Some(Health::Probe(ProbeResult::Unauthorized) | Health::Auth) => Availability::Unauthorized { connection },
        Some(Health::Starting) => Availability::Starting,
        Some(Health::Incompatible) => Availability::Unreachable { reason: Reason::Incompatible, connection },
        Some(Health::Stopped) => Availability::Unreachable { reason: Reason::Stopped, connection },
        Some(Health::Probe(ProbeResult::Unreachable)) => Availability::Unreachable { reason: Reason::Offline, connection },
        health => Availability::Ready { connection: conn.clone(), health: health.cloned() },
    }
}

/// `staged` (`:160-168`).
fn staged(stage: PluginStage) -> Availability {
    match stage {
        PluginStage::Starting => Availability::Starting,
        PluginStage::Auth => Availability::Unauthorized { connection: None },
        PluginStage::Failed => Availability::Unreachable { reason: Reason::Offline, connection: None },
        PluginStage::Stopped => Availability::Unreachable { reason: Reason::Stopped, connection: None },
        PluginStage::Incompatible => Availability::Unreachable { reason: Reason::Incompatible, connection: None },
    }
}

impl Availability {
    /// `serverPagesConnection` (`:81-84`).
    pub fn pages_connection(&self) -> Option<&ServerConn> {
        match self {
            Availability::Ready { connection, .. } => Some(connection),
            _ => None,
        }
    }

    /// `canIndexServerSettings` (`:86-89`): search lists a server's settings only while its pages open.
    pub fn can_index(&self) -> bool {
        matches!(self, Availability::Ready { .. })
    }

    /// `canHoldProjectRoute` (`:91-94`): a starting server connects shortly, so a project page waits.
    pub fn can_hold_project_route(&self) -> bool {
        matches!(self, Availability::Ready { .. } | Availability::Starting)
    }

    /// The health the switcher's dot shows (`selectSettingsServerHealth`, `:96-105`).
    pub fn health(&self) -> Option<Health> {
        match self {
            Availability::Ready { health, .. } => health.clone(),
            Availability::Starting => Some(Health::Starting),
            Availability::Unauthorized { .. } => Some(Health::UNAUTHORIZED),
            Availability::Unreachable { reason: Reason::Offline, .. } => Some(Health::UNREACHABLE),
            Availability::Unreachable { reason: Reason::Incompatible, .. } => Some(Health::Incompatible),
            Availability::Unreachable { reason: Reason::Stopped, .. } => Some(Health::Stopped),
        }
    }

    /// `isSettingsServerUnreachable` (`:107-109`).
    pub fn unreachable(&self) -> bool {
        matches!(self, Availability::Unreachable { .. } | Availability::Unauthorized { .. })
    }

    /// An HTTP server rejecting its saved credentials, which signs in again through Edit server
    /// (`unauthorizedHttpServer`, `:111-116`).
    pub fn unauthorized_http(&self) -> Option<&ServerConn> {
        match self {
            Availability::Unauthorized { connection: Some(c @ ServerConn::Http { .. }) } => Some(c),
            _ => None,
        }
    }
}

/// The Retry button of an unavailable server's page (`ServerRetryState`, `:31-34`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Retry {
    Idle { server: ServerKey, attempt: Attempt },
    Retrying { server: ServerKey, attempt: Attempt },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RetryEvent {
    ServerSynced(ServerKey),
    Clicked,
    Settled { attempt: Attempt },
    TimedOut { attempt: Attempt },
    Disposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RetryEffect {
    /// Run the health `retry` and arm the timeout that answers `TimedOut`.
    Run { attempt: Attempt, timeout_ms: u64 },
    TimeoutCancel,
}

impl Retry {
    /// The server whose page the retry belongs to.
    pub fn server(&self) -> &ServerKey {
        match self {
            Retry::Idle { server, .. } | Retry::Retrying { server, .. } => server,
        }
    }

    /// `createServerRetryState` (`:118-120`).
    pub fn new(server: ServerKey) -> Retry {
        Retry::Idle { server, attempt: Attempt(0) }
    }

    /// `isServerRetrying` (`:152-154`).
    pub fn retrying(&self) -> bool {
        matches!(self, Retry::Retrying { .. })
    }

    fn parts(&self) -> (&ServerKey, Attempt) {
        match self {
            Retry::Idle { server, attempt } | Retry::Retrying { server, attempt } => (server, *attempt),
        }
    }
}

/// `transitionServerRetry` (`machine.ts:122-150`).
pub fn step(state: &mut Retry, event: RetryEvent) -> Effects<RetryEffect> {
    let (server, attempt) = (state.parts().0.clone(), state.parts().1);
    let retrying = state.retrying();
    match event {
        RetryEvent::ServerSynced(next) if next == server => Effects::none(),
        // Another server's page: its Retry starts enabled, and the previous retry can't settle it.
        RetryEvent::ServerSynced(next) => {
            *state = Retry::Idle { server: next, attempt };
            if retrying { Effects::one(RetryEffect::TimeoutCancel) } else { Effects::none() }
        }
        RetryEvent::Clicked if retrying => Effects::none(),
        RetryEvent::Clicked => {
            let attempt = Attempt(attempt.0 + 1);
            *state = Retry::Retrying { server, attempt };
            Effects::one(RetryEffect::Run { attempt, timeout_ms: SERVER_RETRY_TIMEOUT_MS })
        }
        _ if !retrying => Effects::none(),
        RetryEvent::Disposed => {
            *state = Retry::Idle { server, attempt };
            Effects::one(RetryEffect::TimeoutCancel)
        }
        RetryEvent::Settled { attempt: a } | RetryEvent::TimedOut { attempt: a } if a != attempt => Effects::none(),
        RetryEvent::Settled { .. } => {
            *state = Retry::Idle { server, attempt };
            Effects::one(RetryEffect::TimeoutCancel)
        }
        RetryEvent::TimedOut { .. } => {
            *state = Retry::Idle { server, attempt };
            Effects::none()
        }
    }
}
