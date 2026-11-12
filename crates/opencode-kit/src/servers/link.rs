//! One server's link as every surface tells it: connecting, online, its stream reopening, offline
//! (and why), a captive portal or proxy answering in its place, signed out, or a version this app
//! can't use. The app keeps one per server and feeds it what its connection hears (the event
//! stream opening and dropping, with the typed error it dropped with; health probes; the launch's
//! first reads; the OS saying the network changed or the machine woke); the header's status and
//! the reconnect wake come from it, so they can't disagree.
//!
//! Desktop has the stream's `connecting | connected | reconnecting` and a health check apart
//! (`client/src/solid/connection.ts`, `runtime/server/health-machine.ts`); this is Rune's.

use rune::machine::Effects;

use crate::wire_error::{WireError, WireKind};

/// A drop shorter than this says nothing (Kit's grace before the header says Reconnecting).
pub const QUIET: f64 = 2.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Link {
    /// Nothing heard yet, or trying again after the network changed.
    #[default]
    Connecting,
    /// The stream is up (`server.connected` heard).
    Online,
    /// The stream dropped at `since` and is reopening; `shown` once it has lasted [`QUIET`].
    Reopening { since: f64, shown: bool },
    /// Nothing answers: `why` in the client's words (no connection, no answer in time).
    Offline { since: f64, why: Offline },
    /// Something else answers in its place: a captive portal's sign-in page, a proxy.
    Intercepted { since: f64 },
    /// It refused this app's credentials (401, 403).
    SignedOut,
    /// It runs a version this app can't use.
    Incompatible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offline {
    /// Nothing accepts a connection at its address.
    Refused,
    /// It accepted, but didn't answer in time.
    Silent,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The event stream opened (or reopened).
    StreamUp,
    /// The event stream dropped at `now`, with what it dropped with when the client knows.
    StreamDown { now: f64, error: Option<WireError> },
    /// The launch's first reads found nothing answering.
    Unreachable { now: f64 },
    /// A health probe: it answers (and its version is usable or not), or refused our credentials.
    Healthy { compatible: bool },
    Unauthorized,
    /// The OS says the network changed or the machine woke: try again now.
    NetworkChanged,
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// What the header says changed.
    Repaint,
    /// Look again at this time.
    WakeAt(f64),
    /// Reopen the stream and probe now, without waiting out a backoff.
    Reconnect,
}

impl Link {
    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        use Effect::*;
        let before = self.clone();
        let effects = match (&*self, event) {
            (_, Event::StreamUp) | (Link::Connecting | Link::Offline { .. } | Link::Intercepted { .. }, Event::Healthy { compatible: true }) => {
                *self = Link::Online;
                Effects::none()
            }
            (_, Event::Healthy { compatible: false }) => {
                *self = Link::Incompatible;
                Effects::none()
            }
            (_, Event::Unauthorized) => {
                *self = Link::SignedOut;
                Effects::none()
            }
            (_, Event::StreamDown { now, error }) => match error.as_ref().map(|e| &e.kind) {
                Some(WireKind::Redirected { .. } | WireKind::NotOpenCode { .. }) => {
                    *self = Link::Intercepted { since: now };
                    Effects::none()
                }
                Some(WireKind::Status { status: 401 | 403, .. }) => {
                    *self = Link::SignedOut;
                    Effects::none()
                }
                Some(WireKind::Unreachable(_)) if !matches!(self, Link::Online) => {
                    *self = Link::Offline { since: now, why: Offline::Refused };
                    Effects::none()
                }
                // A drop from online, or one the client can't name: it's reopening, quietly at first.
                _ if matches!(self, Link::Online | Link::Connecting) => {
                    *self = Link::Reopening { since: now, shown: false };
                    Effects::one(WakeAt(now + QUIET))
                }
                Some(WireKind::TimedOut) => {
                    *self = Link::Offline { since: now, why: Offline::Silent };
                    Effects::none()
                }
                _ => Effects::none(),
            },
            (_, Event::Unreachable { now }) => {
                *self = Link::Offline { since: now, why: Offline::Refused };
                Effects::none()
            }
            (Link::Offline { .. } | Link::Intercepted { .. } | Link::Reopening { .. } | Link::Connecting, Event::NetworkChanged) => {
                *self = Link::Connecting;
                Effects::one(Reconnect)
            }
            (Link::Reopening { since, shown: false }, Event::Tick { now }) => {
                let since = *since;
                if now + 1e-3 < since + QUIET {
                    Effects::one(WakeAt(since + QUIET))
                } else {
                    *self = Link::Reopening { since, shown: true };
                    Effects::none()
                }
            }
            _ => Effects::none(),
        };
        if *self != before {
            let mut e = effects;
            e.extend(std::iter::once(Repaint));
            return e;
        }
        effects
    }

    /// Whether the header says anything: everything but online, connecting and a short drop.
    pub fn shown(&self) -> bool {
        !matches!(self, Link::Online | Link::Connecting | Link::Reopening { shown: false, .. })
    }
}
