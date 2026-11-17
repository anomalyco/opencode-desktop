//! A session's first screen made ready before its tab is pressed: the window laid out ahead with
//! it in the open session's place (`Tx::prepare_text`), so its projection is kept and its text
//! shaped and broken at the widths the press will ask, and the press's frame does what a return's
//! does. Prepared when the pointer has rested on a tab that isn't shown for [`REST`], or after a
//! switch for the tab beside the one shown. One session at a time, dropped at the next rest, on
//! Home, or when another is prepared; nothing for a session whose messages aren't in memory, and
//! never on a frame with input waiting.
//!
//! An explicit machine (`step`): the pointer resting on a tab is one wake, then one burst, then
//! nothing; an idle window that nobody points at wakes no more. A pointer sweeping the strip
//! wakes it about once per [`REST`] of sweeping and once after, not once per tab ([`State`]).

use rune::Tx;

use crate::{App, Route};

/// How long the pointer rests on a tab before its session is prepared.
pub const REST: f64 = 0.15;

/// A wake asked for can't be taken back, so every state with one pending knows when (`wake`): a
/// pointer crossing tab after tab moves what waits, not the wake, and the wake, when it comes,
/// asks once more for what now waits. A sweep across the strip then wakes about once per
/// [`REST`] of sweeping, plus once after it, however many tabs it crosses.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    /// Nothing waits; a wake asked for before is still to come at `wake`.
    Armed { wake: f64 },
    /// Session `session` is prepared at `due`, the wake asked for coming at `wake`, at or before
    /// `due`. `rest`: the resting pointer asked (its leaving the tab takes it back); otherwise a
    /// switch named the tab beside it, which a pointer crossing that tab doesn't change.
    Waiting { session: String, due: f64, wake: f64, rest: bool },
    /// `session`'s first screen is ready.
    Ready { session: String },
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The pointer came onto (`inside`) or left a session's tab.
    Hover { session: String, inside: bool, now: f64 },
    /// A switch showed a session; `beside` is the tab next to it to prepare.
    Switched { beside: Option<String>, now: f64 },
    /// The clock, from `poll`; `input` when the user's input waits for this frame.
    Clock { now: f64, input: bool },
    /// The window rested, or the route left every session (Home, Settings).
    Rest,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    WakeAt(f64),
    Prepare(String),
    Drop(String),
}

pub fn step(state: &mut State, event: Event) -> Vec<Effect> {
    use Effect::*;
    let reached = |now: f64, at: f64| now + 1e-9 >= at;
    match (std::mem::take(state), event) {
        (State::Ready { session }, Event::Rest) => vec![Drop(session)],
        (_, Event::Rest) => vec![],
        (State::Ready { session }, Event::Hover { session: s, inside: true, .. }) if s == session => {
            *state = State::Ready { session };
            vec![]
        }
        // The pointer reaching the tab that already waits doesn't put it off.
        (State::Waiting { session, due, wake, rest }, Event::Hover { session: s, inside: true, .. }) if s == session => {
            *state = State::Waiting { session, due, wake, rest };
            vec![]
        }
        // A resting pointer waits `REST`, on the wake already coming if there is one.
        (State::Armed { wake } | State::Waiting { wake, .. }, Event::Hover { session, inside: true, now }) => {
            *state = State::Waiting { session, due: now + REST, wake, rest: true };
            vec![]
        }
        (old, Event::Hover { session, inside: true, now }) => wait(state, old, session, now + REST, true),
        // A switch's neighbour goes on the next frame, after the switch's own.
        (old, Event::Switched { beside: Some(session), now }) => wait(state, old, session, now, false),
        // The resting pointer left the tab before it was prepared: nothing waits; the wake still
        // comes.
        (State::Waiting { session, wake, rest: true, .. }, Event::Hover { session: s, inside: false, .. }) if s == session => {
            *state = State::Armed { wake };
            vec![]
        }
        (old, Event::Hover { .. } | Event::Switched { beside: None, .. }) => {
            *state = old;
            vec![]
        }
        (State::Waiting { session, due, rest, .. }, Event::Clock { now, input }) if reached(now, due) => {
            if input {
                // The user's frame first; the next frame prepares.
                *state = State::Waiting { session, due: now, wake: now, rest };
                vec![WakeAt(now)]
            } else {
                *state = State::Ready { session: session.clone() };
                vec![Prepare(session)]
            }
        }
        // The wake came for a tab the pointer since crossed: once more, for the one it's on.
        (State::Waiting { session, due, wake, rest }, Event::Clock { now, .. }) if reached(now, wake) => {
            *state = State::Waiting { session, due, wake: due, rest };
            vec![WakeAt(due)]
        }
        (State::Armed { wake }, Event::Clock { now, .. }) if reached(now, wake) => vec![],
        (old, Event::Clock { .. }) => {
            *state = old;
            vec![]
        }
    }
}

/// Session `session` waits until `due` on a wake of its own; a different one prepared before is
/// dropped.
fn wait(state: &mut State, old: State, session: String, due: f64, rest: bool) -> Vec<Effect> {
    let drop = match old {
        State::Ready { session: r } if r != session => vec![Effect::Drop(r)],
        _ => vec![],
    };
    *state = State::Waiting { session, due, wake: due, rest };
    drop.into_iter().chain([Effect::WakeAt(due)]).collect()
}

impl App {
    fn prepare_event(&mut self, event: Event, tx: &mut Tx) {
        for effect in step(&mut self.prepare, event) {
            match effect {
                Effect::WakeAt(due) => tx.wake_at(due),
                Effect::Prepare(session) => self.prepare_session(&session, tx),
                Effect::Drop(session) => {
                    if !self.shows(&session) {
                        session_view::timeline::projection_forget(&session);
                    }
                }
            }
        }
    }

    /// The pointer came onto or left the tab (or Recent row) for `key`: a tab's key, or a
    /// session's id.
    pub(crate) fn prepare_hover(&mut self, key: &str, inside: bool, tx: &mut Tx) {
        let session = match self.tab(key) {
            Some(t) if t.is_draft() => return,
            Some(t) => t.id.clone(),
            None => key.to_owned(),
        };
        if self.shows(&session) {
            return;
        }
        let now = tx.clock();
        self.prepare_event(Event::Hover { session, inside, now }, tx);
    }

    /// A switch showed tab `key`: the tab after it (or before it, at the end) is prepared next.
    pub(crate) fn prepare_beside(&mut self, key: &str, tx: &mut Tx) {
        let at = self.tabs.iter().position(|t| t.uid() == key || t.id == key);
        let beside = at.and_then(|i| self.tabs.get(i + 1).or_else(|| i.checked_sub(1).and_then(|j| self.tabs.get(j)))).filter(|t| !t.is_draft()).map(|t| t.id.clone());
        let now = tx.clock();
        self.prepare_event(Event::Switched { beside, now }, tx);
    }

    /// From `poll`. Changes nothing the view draws.
    pub(crate) fn poll_prepare(&mut self, now: f64, tx: &mut Tx) {
        if !matches!(&*self.route, Route::Session { .. }) && self.prepare != State::Idle {
            return self.prepare_event(Event::Rest, tx);
        }
        if matches!(self.prepare, State::Waiting { .. } | State::Armed { .. }) {
            let input = tx.input_waiting();
            self.prepare_event(Event::Clock { now, input }, tx);
        }
    }

    /// From the window's rest.
    pub(crate) fn rest_prepare(&mut self, tx: &mut Tx) {
        self.prepare_event(Event::Rest, tx);
    }

    fn shows(&self, session: &str) -> bool {
        matches!(&*self.route, Route::Session { id, .. } if id == session)
    }

    /// Session `session`'s first screen, laid out by Rune in the window's next idle turn
    /// (`Tx::prepare_text`) from the window's own root with it in the open session's place
    /// ([`view`]): its projection kept beside the shown session's (the projection's own bound of
    /// four) and its text shaped and broken at the widths the press will ask. Nothing for a session
    /// whose messages aren't in memory: the trigger goes back to idle and fetches nothing.
    fn prepare_session(&mut self, session: &str, tx: &mut Tx) {
        if !self.transcripts.contains_key(session) {
            self.prepare = State::Idle;
            return;
        }
        tx.prepare_text(view, None);
    }
}

thread_local! {
    /// Whether [`view`] is running: the page shows the prepared session (`preparing`).
    static PREPARING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The session the page shows in place of the route's while Rune lays the window out ahead.
pub(crate) fn preparing(s: &App) -> Option<&str> {
    match &s.prepare {
        State::Ready { session } if PREPARING.get() => Some(session),
        _ => None,
    }
}

/// The window's tree as the next frame will lay it out, the prepared session in the open
/// session's place: `view::app`'s frame around `view::shell`, without `app`'s window-wide
/// settings (chrome, narrowness, titlebar), which hold from the last frame.
fn view(s: &App, cx: &mut rune::Cx<App>) -> rune::El {
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            PREPARING.set(false);
        }
    }
    PREPARING.set(true);
    let _done = Done;
    crate::view::prepared_root(s, cx)
}
