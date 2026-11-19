//! Whether an open session tab can show: its session is known, still resolving, gone from its
//! server, or its server isn't answering. One state per (server, session), as OpenCode Desktop's
//! `packages/app/src/session/resolution-machine.ts` (`resolving | unreachable | ready |
//! not-found`, 30 s `SESSION_RESOLUTION_TIMEOUT_MS`), combined with the server's health into what
//! the tab shows (`sessionRouteView`): a session it holds shows even while its server is away; one
//! it doesn't shows "Can't reach this server" (`session/server-unavailable-panel.tsx`), and one its
//! server deleted "This session cannot be found" (`session/route-error.tsx`). Beyond Desktop, a
//! session whose messages are on screen is never replaced by a panel: offline, signed out or on
//! an unsupported server it stays readable, and the header says which ([`status`]).

use std::collections::HashMap;

use rune::machine::Effects;
use rune::{El, Tx};

use crate::palette::Pal;
use crate::reconnect::ServerId;
use crate::view::server_unavailable::{self, Cover};
use crate::{App, Route};

/// How long a session nothing knows yet may wait for its server before the tab says it can't
/// reach it (`SESSION_RESOLUTION_TIMEOUT_MS`).
pub const TIMEOUT: f64 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resolution {
    /// Asked for at `since`; nothing knows it yet.
    Resolving { since: f64 },
    /// Its server knows it.
    Ready,
    /// Nothing answered within [`TIMEOUT`]; a late answer still decides.
    Unreachable,
    /// Its server doesn't have it (a 404, or deleted while open).
    NotFound,
}

#[derive(Clone, Copy, Debug)]
pub enum ResolutionEvent {
    /// The tab opened at `now`; `known`: its session's info is here.
    Opened { known: bool, now: f64 },
    /// Its info or a page landed.
    Known,
    /// Its server answered that it doesn't have it.
    Missing,
    /// The server is back: a target that didn't resolve tries again.
    Reconnected { now: f64 },
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolutionEffect {
    WakeAt(f64),
    Repaint,
}

impl Resolution {
    /// The state after `event` (`None` before the tab opened), and what to do.
    pub fn step(state: Option<Resolution>, event: ResolutionEvent) -> (Option<Resolution>, Effects<ResolutionEffect>) {
        use ResolutionEffect::*;
        match (state, event) {
            (Some(state), ResolutionEvent::Opened { .. }) => (Some(state), Effects::none()),
            (None, ResolutionEvent::Opened { known: true, .. }) => (Some(Resolution::Ready), Effects::none()),
            (None, ResolutionEvent::Opened { known: false, now }) => (Some(Resolution::Resolving { since: now }), Effects::one(WakeAt(now + TIMEOUT))),
            (Some(Resolution::Resolving { .. } | Resolution::Unreachable), ResolutionEvent::Known) => (Some(Resolution::Ready), Effects::one(Repaint)),
            (_, ResolutionEvent::Missing) => (Some(Resolution::NotFound), Effects::one(Repaint)),
            (Some(Resolution::Unreachable), ResolutionEvent::Reconnected { now }) => (Some(Resolution::Resolving { since: now }), Effects::two(Repaint, WakeAt(now + TIMEOUT))),
            (Some(Resolution::Resolving { since }), ResolutionEvent::Tick { now }) if now - since >= TIMEOUT => (Some(Resolution::Unreachable), Effects::one(Repaint)),
            (state, _) => (state, Effects::none()),
        }
    }
}

/// What an open session tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The session (or, while it resolves, what this window holds of it).
    Session,
    /// Its server refused our credentials (`ServerSignedOutPanel`).
    SignedOut,
    /// Its server runs a version this app can't use.
    Incompatible,
    Unreachable,
    NotFound,
}

/// Every open session's resolution, by (server, session).
#[derive(Default)]
pub struct States(HashMap<(ServerId, String), Resolution>);

impl States {
    pub fn get(&self, server: &str, id: &str) -> Option<Resolution> { self.0.get(&(server.to_owned(), id.to_owned())).copied() }

    fn apply(&mut self, server: &str, id: &str, event: ResolutionEvent) -> Effects<ResolutionEffect> {
        let key = (server.to_owned(), id.to_owned());
        let (next, effects) = Resolution::step(self.0.get(&key).copied(), event);
        match next {
            Some(next) => self.0.insert(key, next),
            None => self.0.remove(&key),
        };
        effects
    }

    /// A tab that closes forgets its session's resolution.
    pub fn forget(&mut self, server: &str, id: &str) { self.0.remove(&(server.to_owned(), id.to_owned())); }
}

fn server(s: &App) -> ServerId { crate::launch::window_server(s).unwrap_or_default() }

fn run(s: &mut App, effects: Effects<ResolutionEffect>, tx: &mut Tx) -> bool {
    let mut changed = false;
    for effect in effects {
        match effect {
            ResolutionEffect::WakeAt(at) => tx.wake_after(std::time::Duration::from_secs_f64((at - *s.clock).max(0.0))),
            ResolutionEffect::Repaint => {
                tx.instant();
                changed = true;
            }
        }
    }
    changed
}

/// The tab of `id` opened.
pub fn opened(s: &mut App, id: &str, tx: &mut Tx) {
    let (server, known, now) = (server(s), s.session(id).is_some(), *s.clock);
    let effects = s.services.session_states.apply(&server, id, ResolutionEvent::Opened { known, now });
    run(s, effects, tx);
}

/// `id`'s server said it doesn't have it: a read answered 404, or it was deleted while open.
pub fn missing(s: &mut App, id: &str, tx: &mut Tx) -> bool {
    let server = server(s);
    let effects = s.services.session_states.apply(&server, id, ResolutionEvent::Missing);
    run(s, effects, tx)
}

/// The server came back: what didn't resolve tries again.
pub fn reconnected(s: &mut App, tx: &mut Tx) {
    let (server, now) = (server(s), *s.clock);
    let ids: Vec<String> = s.services.session_states.0.iter().filter(|((k, _), r)| *k == server && **r == Resolution::Unreachable).map(|((_, id), _)| id.clone()).collect();
    for id in ids {
        let effects = s.services.session_states.apply(&server, &id, ResolutionEvent::Reconnected { now });
        run(s, effects, tx);
        s.reload_session(&id);
    }
}

/// `poll`: a resolving session its info or page reached is ready; one that waited too long can't
/// be reached.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let key = crate::servers::runtime::active(s).as_str();
    if !s.services.session_states.0.iter().any(|((k, _), r)| k == key && matches!(r, Resolution::Resolving { .. } | Resolution::Unreachable)) {
        return false;
    }
    let (server, now) = (server(s), *s.clock);
    let resolving: Vec<String> = s.services.session_states.0.iter().filter(|((k, _), r)| *k == server && matches!(r, Resolution::Resolving { .. } | Resolution::Unreachable)).map(|((_, id), _)| id.clone()).collect();
    let mut changed = false;
    for id in resolving {
        let event = if s.session(&id).is_some() { ResolutionEvent::Known } else { ResolutionEvent::Tick { now } };
        let effects = s.services.session_states.apply(&server, &id, event);
        changed |= run(s, effects, tx);
    }
    changed
}

/// What the tab of `id` shows: its resolution with its server's health, as `sessionRouteView`
/// (`resolution-machine.ts:151-179`), with one difference: a session whose messages are loaded
/// (from its server, or the boot record's preview at launch) stays readable whatever its server
/// says, and the header says why it isn't live ([`status`]). Desktop covers it with the signed
/// out or incompatible panel, and an unreachable one whose info it hadn't listed.
pub fn view(s: &App, id: &str) -> View {
    let key = crate::servers::runtime::active(s);
    let resolution = s.services.session_states.get(&server(s), id);
    if resolution == Some(Resolution::NotFound) {
        return View::NotFound;
    }
    if s.transcripts.get(id).is_some_and(|t| !t.messages.is_empty()) {
        return View::Session;
    }
    match server_unavailable::cover(key) {
        Some(Cover::SignedOut) => return View::SignedOut,
        Some(Cover::Incompatible) => return View::Incompatible,
        None => {}
    }
    let known = s.session(id).is_some();
    match resolution {
        Some(Resolution::Ready) => View::Session,
        _ if known && !offline(s) => View::Session,
        Some(Resolution::Unreachable) => View::Unreachable,
        // Nothing known of it and its server isn't answering (never reached, its stream is down,
        // or its health check failed): say so rather than wait blank, as Desktop's
        // `unreachable | stopped` health does.
        _ if offline(s) || crate::reconnect::link_of(s, key.as_str()).dropped() => View::Unreachable,
        _ => View::Session,
    }
}

/// The window's server didn't answer its launch, or its health says nothing answers it.
fn offline(s: &App) -> bool {
    crate::launch::offline(s) || server_unavailable::unreachable(crate::servers::runtime::active(s))
}

/// What the session header says beside a session that shows while its server can't serve it
/// live: signed out (with Sign in again), an unsupported version, offline, or the stream
/// reconnecting. Nothing while it's live.
pub fn status(s: &App, p: &Pal) -> Option<El> {
    use opencode_kit::servers::link::Link;
    let key = crate::servers::runtime::active(s);
    let small = |words: &str| rune::text(words.to_owned()).key("connection-status").font(crate::view::UI_SMALL).color(p.faint);
    // One machine says it (`opencode_kit::servers::link`), stepped by the stream, the other
    // calls, the launch, the health probe and the OS (`reconnect`).
    match crate::reconnect::link_of(s, key.as_str()) {
        Link::SignedOut => Some(server_unavailable::signed_out_status(p, key)),
        Link::Incompatible => Some(small(opencode_i18n::t!("session.error.incompatible"))),
        // Something on the network answers in the server's place (a sign-in page).
        Link::Intercepted { .. } => Some(small(opencode_i18n::t!("rune.session.interceptedCopy"))),
        Link::Offline { .. } => Some(small(opencode_i18n::t!("rune.session.offlineCopy"))),
        // It took the launch's connection and hasn't answered since (`launch::FIRST_PATIENCE`).
        Link::Slow { .. } => Some(small(opencode_i18n::t!("rune.session.slowCopy"))),
        link if link.reconnecting() => Some(small(opencode_i18n::t!("rune.session.reconnecting"))),
        _ => None,
    }
}

/// The open session's panel, when it can't show.
pub fn panel(s: &App, p: &Pal, id: &str) -> Option<El> {
    let Route::Session { id: open, server } = &*s.route else { return None };
    if open != id {
        return None;
    }
    let tab = s.current_tab();
    let close = tab.as_deref().unwrap_or(id);
    match view(s, id) {
        View::Session => None,
        View::SignedOut => Some(server_unavailable::signed_out(p, server)),
        View::Incompatible => Some(server_unavailable::unavailable(p, server_unavailable::Reason::Incompatible, server, Some(close))),
        View::Unreachable => Some(server_unavailable::unavailable(p, server_unavailable::Reason::Unreachable, server, Some(close))),
        View::NotFound => Some(server_unavailable::not_found(p, server, id, close)),
    }
}
