//! Whether an open session tab can show: its session is known, still resolving, gone from its
//! server, or its server isn't answering. One state per (server, session), as OpenCode Desktop's
//! `packages/app/src/session/resolution-machine.ts` (`resolving | unreachable | ready |
//! not-found`, 30 s `SESSION_RESOLUTION_TIMEOUT_MS`), combined with the server's health into what
//! the tab shows (`sessionRouteView`): a session it holds shows even while its server is away; one
//! it doesn't shows "Can't reach this server" (`session/server-unavailable-panel.tsx`), and one its
//! server deleted "This session cannot be found" (`session/route-error.tsx`).

use std::collections::HashMap;

use rune::machine::Effects;
use rune::{El, Tx, col, icon, text};
use rune_theme_opencode::icons as ic;

use crate::palette::Pal;
use crate::reconnect::ServerId;
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
            ResolutionEffect::WakeAt(at) => tx.wake_after(std::time::Duration::from_secs_f64((at - s.clock).max(0.0))),
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
    let (server, known, now) = (server(s), s.session(id).is_some(), s.clock);
    let effects = s.session_states.apply(&server, id, ResolutionEvent::Opened { known, now });
    run(s, effects, tx);
}

/// `id`'s server said it doesn't have it: a read answered 404, or it was deleted while open.
pub fn missing(s: &mut App, id: &str, tx: &mut Tx) -> bool {
    let server = server(s);
    let effects = s.session_states.apply(&server, id, ResolutionEvent::Missing);
    run(s, effects, tx)
}

/// A read's error says the session isn't on its server.
pub fn not_found_error(e: &str) -> bool { e.contains("http status: 404") }

/// The server came back: what didn't resolve tries again.
pub fn reconnected(s: &mut App, tx: &mut Tx) {
    let (server, now) = (server(s), s.clock);
    let ids: Vec<String> = s.session_states.0.iter().filter(|((k, _), r)| *k == server && **r == Resolution::Unreachable).map(|((_, id), _)| id.clone()).collect();
    for id in ids {
        let effects = s.session_states.apply(&server, &id, ResolutionEvent::Reconnected { now });
        run(s, effects, tx);
        s.reload_session(&id);
    }
}

/// `poll`: a resolving session its info or page reached is ready; one that waited too long can't
/// be reached.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let key = crate::servers::runtime::active(s).as_str();
    if !s.session_states.0.iter().any(|((k, _), r)| k == key && matches!(r, Resolution::Resolving { .. } | Resolution::Unreachable)) {
        return false;
    }
    let (server, now) = (server(s), s.clock);
    let resolving: Vec<String> = s.session_states.0.iter().filter(|((k, _), r)| *k == server && matches!(r, Resolution::Resolving { .. } | Resolution::Unreachable)).map(|((_, id), _)| id.clone()).collect();
    let mut changed = false;
    for id in resolving {
        let event = if s.session(&id).is_some() { ResolutionEvent::Known } else { ResolutionEvent::Tick { now } };
        let effects = s.session_states.apply(&server, &id, event);
        changed |= run(s, effects, tx);
    }
    changed
}

/// What the tab of `id` shows: its resolution with its server's health, as `sessionRouteView`.
pub fn view(s: &App, id: &str) -> View {
    let known = s.session(id).is_some();
    match s.session_states.get(&server(s), id) {
        Some(Resolution::NotFound) => View::NotFound,
        Some(Resolution::Ready) => View::Session,
        _ if known => View::Session,
        Some(Resolution::Unreachable) => View::Unreachable,
        // Nothing known of it and its server isn't answering (never reached, or its stream is
        // down): say so rather than wait blank, as Desktop's `unreachable | stopped` health does.
        _ if crate::launch::offline(s) || s.server_links.get(&server(s)).dropped() => View::Unreachable,
        _ => View::Session,
    }
}

/// The open session's panel, when it can't show.
pub fn panel(s: &App, p: &Pal, id: &str) -> Option<El> {
    let Route::Session { id: open, .. } = &s.route else { return None };
    if open != id {
        return None;
    }
    match view(s, id) {
        View::Session => None,
        View::Unreachable => Some(unavailable(s, p, id)),
        View::NotFound => Some(not_found(s, p, id)),
    }
}

/// `text-16-medium`: 16 on 28.8 at 530.
const TITLE: rune::TextStyle = rune::TextStyle::new(16.0, 530.0).lh(28.8);
/// The panel's body, 13 on 24 at 440 (`text-13-regular` inherits the page's line height).
const BODY: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(24.0);
/// The session id: IBM Plex Mono 13 at 530 on 16.
const ID: rune::TextStyle = rune::TextStyle::new(13.0, 530.0).mono().lh(16.0);

/// The server's name as the panel says it: the app's own server's address without its scheme,
/// another server's name (`servers::runtime::label`).
fn server_label(s: &App) -> String {
    crate::servers::runtime::label(crate::servers::runtime::active(s))
}

/// `SessionStatePanel`'s frame: centred, 24 in from the sides, 168 clear below, 16 up.
fn frame(key: &str, children: Vec<El>) -> El {
    col().key(key.to_owned()).grow(1.0).w_full().min_h(0.0).px(24.0).pb(168.0 + 32.0).items_center().justify_center().gap(16.0).children(children)
}

fn close_tab(id: &str, p: &Pal) -> El {
    let id = id.to_owned();
    crate::widgets::button(opencode_i18n::t!("session.error.notFound.closeTab"), rune_kit::ButtonVariant::Neutral)
        .icon(&ic::XMARK_SMALL)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.close_tab(&id, tx))
        .build(crate::palette::kit(p))
        .key("session-state-close")
}

fn heading(p: &Pal, title: String, description: String) -> El {
    col()
        .items_center()
        .gap(8.0)
        .child(text(title).font(TITLE).color(p.text).max_w(448.0).wrap().align(rune::Align::Center))
        .child(text(description).font(BODY).color(p.v1_text_weak).max_w(448.0).wrap().align(rune::Align::Center))
}

/// `ServerUnavailablePanel reason="unreachable"`.
fn unavailable(s: &App, p: &Pal, id: &str) -> El {
    let description = opencode_i18n::t!("session.error.unreachable.description", server = server_label(s)).to_owned();
    frame("session-unreachable", vec![
        icon(&ic::OUTLINE_HEXAGONAL_WARNING, 20.0).color(p.danger),
        heading(p, opencode_i18n::t!("session.error.unreachable").to_owned(), description),
        close_tab(id, p),
    ])
}

/// `SessionNotFoundPanel`.
fn not_found(s: &App, p: &Pal, id: &str) -> El {
    let ident = col()
        .items_center()
        .gap(4.0)
        .child(text(server_label(s)).font(BODY).color(p.text))
        .child(text(id.to_owned()).font(ID).color(p.v1_text_base).fill(p.text.alpha(0.08)).radius(4.0).px(4.0).py(2.0));
    frame("session-not-found", vec![
        heading(p, opencode_i18n::t!("session.error.notFound").to_owned(), opencode_i18n::t!("session.error.notFound.description").to_owned()),
        ident,
        close_tab(id, p),
    ])
}
