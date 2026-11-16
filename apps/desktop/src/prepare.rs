//! A session's first screen made ready before its tab is pressed: its projection built (and,
//! through TEXT-2's entry point, its text shaped and broken) in idle time, so the press's frame
//! does what a return's does. Prepared when the pointer has rested on a tab that isn't shown for
//! [`REST`], or after a switch for the tab beside the one shown. One session at a time, dropped
//! at the next rest, on Home, or when another is prepared; nothing for a session whose messages
//! aren't in memory, and never on a frame with input waiting.
//!
//! An explicit machine (`step`): the pointer resting on a tab is one wake, then one burst, then
//! nothing; an idle window that nobody points at wakes no more.

use rune::Tx;

use crate::{App, Route};

/// How long the pointer rests on a tab before its session is prepared.
pub const REST: f64 = 0.15;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    /// The pointer rests on session `session`'s tab, or a switch named it; prepared at `due`.
    Waiting { session: String, due: f64 },
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
    match (std::mem::take(state), event) {
        (State::Ready { session }, Event::Rest) => vec![Drop(session)],
        (_, Event::Rest) => vec![],
        (State::Ready { session }, Event::Hover { session: s, inside: true, .. }) if s == session => {
            *state = State::Ready { session };
            vec![]
        }
        // A resting pointer waits `REST`; a switch's neighbour goes on the next frame, after the
        // switch's own.
        (old, Event::Hover { session, inside: true, now }) => wait(state, old, session, now + REST),
        (old, Event::Switched { beside: Some(session), now }) => wait(state, old, session, now),
        // The pointer left the tab before it was prepared: nothing happens.
        (State::Waiting { session, .. }, Event::Hover { session: s, inside: false, .. }) if s == session => vec![],
        (old, Event::Hover { .. } | Event::Switched { beside: None, .. }) => {
            *state = old;
            vec![]
        }
        (State::Waiting { session, due }, Event::Clock { now, input }) if now + 1e-9 >= due => {
            if input {
                // The user's frame first; the next frame prepares.
                *state = State::Waiting { session, due: now };
                vec![WakeAt(now)]
            } else {
                *state = State::Ready { session: session.clone() };
                vec![Prepare(session)]
            }
        }
        (old, Event::Clock { .. }) => {
            *state = old;
            vec![]
        }
    }
}

/// Session `session` waits until `due`; a different one prepared before is dropped.
fn wait(state: &mut State, old: State, session: String, due: f64) -> Vec<Effect> {
    let drop = match old {
        State::Ready { session: r } if r != session => vec![Effect::Drop(r)],
        _ => vec![],
    };
    *state = State::Waiting { session, due };
    drop.into_iter().chain([Effect::WakeAt(due)]).collect()
}

impl App {
    fn prepare_event(&mut self, event: Event, tx: &mut Tx) {
        for effect in step(&mut self.prepare, event) {
            match effect {
                Effect::WakeAt(due) => tx.wake_at(due),
                Effect::Prepare(session) => self.prepare_session(&session),
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
        if matches!(self.prepare, State::Waiting { .. }) {
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

    /// Session `session`'s first screen: its projection (kept beside the shown session's, the
    /// projection's own bound of four), when its messages are in memory.
    fn prepare_session(&self, session: &str) {
        let Some(t) = self.transcripts.get(session) else { return };
        let _ = session_view::timeline::rows(self, t);
    }
}
