//! Each listed server's controller, for the registry's lifetime (`runtime/server/machine.ts`,
//! ported transition for transition): one per listed server, kept until the server is unlisted,
//! answers our credentials with 401, gets new HTTP credentials, or the app quits. The caller owns
//! the controllers themselves (a server's peer: its event stream, worker and data), keyed by
//! `(ServerKey, Attempt)`; this machine says when to create and dispose them.

use opencode_kit::servers::key::ServerKey;
use rune::machine::{Attempt, Effects};

use crate::health::ProbeResult;
use crate::registry::{Credentials, ServerConn};

/// One server's controller (`ServerControllerState`, `machine.ts:15-39`). `Starting` lasts only
/// until the caller reports the controller it created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    Absent { attempt: Attempt },
    Starting { attempt: Attempt, conn: ServerConn, credentials: Credentials },
    Live { attempt: Attempt, conn: ServerConn, credentials: Credentials },
    /// The server answered `credentials.auth` with HTTP 401 (`rejected`), so it has no controller
    /// until it signs in again.
    Unauthorized { attempt: Attempt, conn: ServerConn, credentials: Credentials, rejected: String },
}

impl Slot {
    pub fn attempt(&self) -> Attempt {
        match self {
            Slot::Absent { attempt } | Slot::Starting { attempt, .. } | Slot::Live { attempt, .. } | Slot::Unauthorized { attempt, .. } => *attempt,
        }
    }
}

/// `ServerControllersState` (`:41`): every server's slot, in the order they were first listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub slots: Vec<(ServerKey, Slot)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The listed servers, each with its credentials (`servers.synced`).
    ServersSynced(Vec<ServerConn>),
    /// The caller created the controller `Create` asked for.
    ControllerCreated { key: ServerKey, attempt: Attempt },
    /// A health probe of `auth` answered (`health.reported`).
    HealthReported { key: ServerKey, auth: String, result: ProbeResult },
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Create { key: ServerKey, attempt: Attempt, conn: ServerConn },
    /// Disposes the controller created for `(key, attempt)`.
    Dispose { key: ServerKey, attempt: Attempt },
}

impl State {
    pub fn get(&self, key: &ServerKey) -> Option<&Slot> {
        self.slots.iter().find(|(k, _)| k == key).map(|(_, s)| s)
    }

    /// `selectServerCtx` (`:130-135`): the attempt whose controller is live, if any.
    pub fn live(&self, key: &ServerKey) -> Option<Attempt> {
        match self.get(key) {
            Some(Slot::Live { attempt, .. }) => Some(*attempt),
            _ => None,
        }
    }

    /// `selectServerSignedOut` (`:137-140`).
    pub fn signed_out(&self, key: &ServerKey) -> bool {
        matches!(self.get(key), Some(Slot::Unauthorized { .. }))
    }

    fn slot_mut(&mut self, key: &ServerKey) -> &mut Slot {
        let at = match self.slots.iter().position(|(k, _)| k == key) {
            Some(at) => at,
            None => {
                self.slots.push((key.clone(), Slot::Absent { attempt: Attempt(0) }));
                self.slots.len() - 1
            }
        };
        &mut self.slots[at].1
    }
}

/// `transitionServerControllers` (`machine.ts:109-128`).
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let mut fx = Effects::none();
    match event {
        Event::ServersSynced(conns) => {
            let gone: Vec<ServerKey> = state.slots.iter().map(|(k, _)| k.clone()).filter(|k| !conns.iter().any(|c| c.key() == *k)).collect();
            for key in gone {
                unlisted(state.slot_mut(&key), &key, &mut fx);
            }
            for conn in conns {
                let key = conn.key();
                let credentials = conn.credentials();
                listed(state.slot_mut(&key), &key, conn, credentials, &mut fx);
            }
        }
        Event::Disposed => {
            for (key, slot) in &mut state.slots {
                unlisted(slot, key, &mut fx);
            }
        }
        Event::ControllerCreated { key, attempt } => {
            let slot = state.slot_mut(&key);
            match slot {
                Slot::Starting { attempt: a, conn, credentials } if *a == attempt => {
                    *slot = Slot::Live { attempt, conn: conn.clone(), credentials: credentials.clone() };
                }
                // A creation a newer listing, a rejection or an unlisting superseded never shows (`:181-182`).
                _ => fx.push(Effect::Dispose { key, attempt }),
            }
        }
        Event::HealthReported { key, auth, result } => reported(state.slot_mut(&key), &key, &auth, &result, &mut fx),
    }
    state.slots.retain(|(_, s)| !matches!(s, Slot::Absent { attempt: Attempt(0) }));
    fx
}

/// `server.unlisted` (`:185-189`).
fn unlisted(slot: &mut Slot, key: &ServerKey, fx: &mut Effects<Effect>) {
    if matches!(slot, Slot::Absent { .. }) {
        return;
    }
    release(slot, key, fx);
    *slot = Slot::Absent { attempt: slot.attempt() };
}

/// `listed` (`:196-213`).
fn listed(slot: &mut Slot, key: &ServerKey, conn: ServerConn, credentials: Credentials, fx: &mut Effects<Effect>) {
    match slot {
        Slot::Absent { .. } => start(slot, key, conn, credentials, fx),
        Slot::Unauthorized { rejected, .. } => {
            // The rejected credentials would fail again; a plugin server that isn't ready has none.
            match &credentials.auth {
                Some(auth) if auth != rejected => start(slot, key, conn, credentials, fx),
                _ => update(slot, conn, credentials),
            }
        }
        Slot::Starting { credentials: c, .. } | Slot::Live { credentials: c, .. } => {
            if c.controller != credentials.controller {
                release(slot, key, fx);
                start(slot, key, conn, credentials, fx);
                return;
            }
            update(slot, conn, credentials);
        }
    }
}

/// `reported` (`:215-242`).
fn reported(slot: &mut Slot, key: &ServerKey, auth: &str, result: &ProbeResult, fx: &mut Effects<Effect>) {
    match slot {
        Slot::Absent { .. } => {}
        // A server that accepts the rejected credentials again gets a controller (`:221-227`).
        Slot::Unauthorized { conn, credentials, .. } => {
            if matches!(result, ProbeResult::Healthy { .. }) && credentials.auth.as_deref() == Some(auth) {
                let (conn, credentials) = (conn.clone(), credentials.clone());
                start(slot, key, conn, credentials, fx);
            }
        }
        // A probe of credentials since replaced says nothing about the current ones (`:229-230`).
        Slot::Starting { conn, credentials, attempt } | Slot::Live { conn, credentials, attempt } => {
            if *result != ProbeResult::Unauthorized || credentials.auth.as_deref() != Some(auth) {
                return;
            }
            let next = Slot::Unauthorized { attempt: *attempt, conn: conn.clone(), credentials: credentials.clone(), rejected: auth.to_owned() };
            release(slot, key, fx);
            *slot = next;
        }
    }
}

/// `start` (`:244-256`).
fn start(slot: &mut Slot, key: &ServerKey, conn: ServerConn, credentials: Credentials, fx: &mut Effects<Effect>) {
    let attempt = Attempt(slot.attempt().0 + 1);
    fx.push(Effect::Create { key: key.clone(), attempt, conn: conn.clone() });
    *slot = Slot::Starting { attempt, conn, credentials };
}

/// `update` (`:258-271`): a new connection or credentials that keep the controller.
fn update(slot: &mut Slot, conn: ServerConn, credentials: Credentials) {
    match slot {
        Slot::Starting { conn: c, credentials: k, .. } | Slot::Live { conn: c, credentials: k, .. } | Slot::Unauthorized { conn: c, credentials: k, .. } => {
            *c = conn;
            *k = credentials;
        }
        Slot::Absent { .. } => {}
    }
}

/// `release` (`:273-275`): only a live controller has something to dispose.
fn release(slot: &Slot, key: &ServerKey, fx: &mut Effects<Effect>) {
    if let Slot::Live { attempt, .. } = slot {
        fx.push(Effect::Dispose { key: key.clone(), attempt: *attempt });
    }
}
