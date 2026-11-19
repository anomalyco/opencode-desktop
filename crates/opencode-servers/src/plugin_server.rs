//! One contributed server's stage and sign-in (`runtime/server/plugin-machine.ts`, ported
//! transition for transition). `Authenticating` starts when the user asks to sign in and lasts
//! until the plugin reports the server ready, the plugin's `connect` gives up, or the 120 s
//! timeout runs out; meanwhile the server shows as starting.

use opencode_kit::servers::HttpBase;
use rune::machine::{Attempt, Effects};

/// How long a sign-in may run before the server shows what its plugin reports again
/// (`PLUGIN_CONNECT_TIMEOUT_MS`, `plugin-machine.ts:4-5`).
pub const PLUGIN_CONNECT_TIMEOUT_MS: u64 = 120_000;

/// A stage a contributed server reports while it isn't ready (`ServerState` without `ready`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PluginStage {
    Stopped,
    Starting,
    Auth,
    Failed,
    Incompatible,
}

impl PluginStage {
    pub fn name(self) -> &'static str {
        match self {
            PluginStage::Stopped => "stopped",
            PluginStage::Starting => "starting",
            PluginStage::Auth => "auth",
            PluginStage::Failed => "failed",
            PluginStage::Incompatible => "incompatible",
        }
    }

    pub fn parse(s: &str) -> Option<PluginStage> {
        Some(match s {
            "stopped" => PluginStage::Stopped,
            "starting" => PluginStage::Starting,
            "auth" => PluginStage::Auth,
            "failed" => PluginStage::Failed,
            "incompatible" => PluginStage::Incompatible,
            _ => return None,
        })
    }
}

/// Where a contributed server stands; only a ready one has an endpoint (`PluginServerStatus`,
/// `plugin-machine.ts:7-10`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PluginStatus {
    Stage(PluginStage),
    Ready { http: HttpBase },
}

impl PluginStatus {
    pub fn http(&self) -> Option<&HttpBase> {
        match self {
            PluginStatus::Ready { http } => Some(http),
            PluginStatus::Stage(_) => None,
        }
    }
}

/// What the app reads from a plugin's `ServerEntry` (`PluginEntrySnapshot`, `:12-13`): its state
/// (`"ready"` or a stage) and endpoint, if any.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PluginEntry {
    pub ready: bool,
    pub stage: PluginStage,
    pub http: Option<HttpBase>,
}

impl PluginEntry {
    pub fn ready(http: HttpBase) -> PluginEntry {
        PluginEntry { ready: true, stage: PluginStage::Starting, http: Some(http) }
    }

    pub fn staged(stage: PluginStage) -> PluginEntry {
        PluginEntry { ready: false, stage, http: None }
    }

    /// `target` (`:153-162`): a plugin that reports ready without an endpoint can't be reached,
    /// so it reads as failed.
    fn target(&self) -> PluginStatus {
        match (&self.http, self.ready) {
            (Some(http), true) => PluginStatus::Ready { http: http.clone() },
            (None, true) => PluginStatus::Stage(PluginStage::Failed),
            (_, false) => PluginStatus::Stage(self.stage),
        }
    }
}

/// What a sign-in in progress shows meanwhile; it returns to it when sign-in ends unready.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reported {
    Auth,
    Starting,
}

impl Reported {
    fn stage(self) -> PluginStage {
        match self {
            Reported::Auth => PluginStage::Auth,
            Reported::Starting => PluginStage::Starting,
        }
    }
}

/// A caller's continuation to run once the server is ready (`onConnected`), named by the caller.
pub type CallbackId = u64;

/// `PluginServerState` (`plugin-machine.ts:17-31`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Stage { stage: PluginStage, attempt: Attempt },
    Ready { http: HttpBase, attempt: Attempt },
    Authenticating { attempt: Attempt, reported: Reported, callbacks: Vec<CallbackId> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    EntryUpdated(PluginEntry),
    Authenticate { on_connected: Option<CallbackId> },
    ConnectSettled { attempt: Attempt, ready: bool },
    ConnectTimedOut { attempt: Attempt },
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Run the plugin's `connect`, and arm the timeout that answers `ConnectTimedOut`.
    Connect { attempt: Attempt, timeout_ms: u64 },
    TimeoutCancel,
    NotifyConnected(Vec<CallbackId>),
}

impl State {
    /// `createPluginServerState` (`:50-52`).
    pub fn new(entry: &PluginEntry) -> State {
        with_attempt(entry.target(), Attempt(0))
    }

    /// `selectPluginStatus` (`:100-107`): a sign-in in progress shows as starting.
    pub fn status(&self) -> PluginStatus {
        match self {
            State::Ready { http, .. } => PluginStatus::Ready { http: http.clone() },
            State::Authenticating { .. } => PluginStatus::Stage(PluginStage::Starting),
            State::Stage { stage, .. } => PluginStatus::Stage(*stage),
        }
    }

    /// `canAuthenticatePluginServer` (`:109-112`).
    pub fn can_authenticate(&self) -> bool {
        matches!(self, State::Stage { stage: PluginStage::Auth, .. } | State::Authenticating { .. })
    }

    pub fn attempt(&self) -> Attempt {
        match self {
            State::Stage { attempt, .. } | State::Ready { attempt, .. } | State::Authenticating { attempt, .. } => *attempt,
        }
    }
}

/// `transitionPluginServer` (`plugin-machine.ts:54-98`).
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    match event {
        Event::EntryUpdated(entry) => updated(state, &entry),
        Event::Authenticate { on_connected } => {
            if let State::Stage { stage: PluginStage::Auth, attempt } = state {
                let attempt = Attempt(attempt.0 + 1);
                *state = State::Authenticating { attempt, reported: Reported::Auth, callbacks: on_connected.into_iter().collect() };
                return Effects::one(Effect::Connect { attempt, timeout_ms: PLUGIN_CONNECT_TIMEOUT_MS });
            }
            // A second click joins the sign-in in progress instead of starting another (`:72-74`).
            if let (State::Authenticating { callbacks, .. }, Some(cb)) = (&mut *state, on_connected) {
                callbacks.push(cb);
            }
            Effects::none()
        }
        // An unlisted server drops its sign-in's callbacks; a late answer finds nothing (`:79-87`).
        Event::Disposed => {
            if matches!(state, State::Stage { stage: PluginStage::Stopped, .. }) {
                return Effects::none();
            }
            let cancel = matches!(state, State::Authenticating { .. });
            *state = State::Stage { stage: PluginStage::Stopped, attempt: state.attempt() };
            if cancel { Effects::one(Effect::TimeoutCancel) } else { Effects::none() }
        }
        Event::ConnectTimedOut { attempt: a } | Event::ConnectSettled { attempt: a, .. } => {
            let State::Authenticating { attempt, reported, .. } = state else { return Effects::none() };
            if *attempt != a {
                return Effects::none();
            }
            let back = State::Stage { stage: reported.stage(), attempt: *attempt };
            match event {
                Event::ConnectTimedOut { .. } => {
                    *state = back;
                    Effects::none()
                }
                // The plugin reports ready on its own channel; until it does, the timeout bounds the wait.
                Event::ConnectSettled { ready: true, .. } => Effects::none(),
                _ => {
                    *state = back;
                    Effects::one(Effect::TimeoutCancel)
                }
            }
        }
    }
}

/// `updated` (`:122-151`).
fn updated(state: &mut State, entry: &PluginEntry) -> Effects<Effect> {
    let next = entry.target();
    if let State::Authenticating { attempt, reported, callbacks } = state {
        let attempt = *attempt;
        return match next {
            PluginStatus::Ready { http } => {
                let callbacks = std::mem::take(callbacks);
                *state = State::Ready { http, attempt };
                let mut fx = Effects::one(Effect::TimeoutCancel);
                if !callbacks.is_empty() {
                    fx.push(Effect::NotifyConnected(callbacks));
                }
                fx
            }
            // A sign-in can report a prompt or progress before it settles (`:135-139`).
            PluginStatus::Stage(PluginStage::Auth) => {
                *reported = Reported::Auth;
                Effects::none()
            }
            PluginStatus::Stage(PluginStage::Starting) => {
                *reported = Reported::Starting;
                Effects::none()
            }
            PluginStatus::Stage(stage) => {
                *state = State::Stage { stage, attempt };
                Effects::one(Effect::TimeoutCancel)
            }
        };
    }
    let attempt = state.attempt();
    let same = match (&*state, &next) {
        (State::Stage { stage, .. }, PluginStatus::Stage(n)) => stage == n,
        (State::Ready { http, .. }, PluginStatus::Ready { http: n }) => http == n,
        _ => false,
    };
    if !same {
        *state = with_attempt(next, attempt);
    }
    Effects::none()
}

fn with_attempt(status: PluginStatus, attempt: Attempt) -> State {
    match status {
        PluginStatus::Ready { http } => State::Ready { http, attempt },
        PluginStatus::Stage(stage) => State::Stage { stage, attempt },
    }
}
