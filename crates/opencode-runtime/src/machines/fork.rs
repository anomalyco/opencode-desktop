//! One fork at a time, after Desktop's `session/fork-machine.ts`: a fork from the `/fork` picker
//! (`Dialog`) keeps the picker open and still until the forked session opens, then closes it; one
//! from the timeline's "Fork from here" (`Inline`) can fork again once it opens. Each fork has its
//! attempt and a 30 s bound (`FORK_TIMEOUT_MS`); moving to another session cancels it.

use rune::machine::{Attempt, Effects, Seq};

use crate::draft::SavedDraft;

pub const TIMEOUT_SECS: f64 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Dialog,
    Inline,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State<D = SavedDraft> {
    #[default]
    Idle,
    Forking { attempt: Attempt, surface: Surface, session: String, message: String, draft: D, due: f64 },
}

#[derive(Clone, Debug)]
pub struct Fork<D = SavedDraft> {
    pub state: State<D>,
    seq: Seq,
}

impl<D> Default for Fork<D> {
    fn default() -> Self {
        Self { state: State::Idle, seq: Seq::default() }
    }
}

pub enum Event<D = SavedDraft> {
    Requested { surface: Surface, session: String, message: String, draft: D, now: f64 },
    Succeeded { attempt: Attempt, forked: String },
    Failed { attempt: Attempt, why: String },
    Due { now: f64 },
    /// The route moved to another session, or the picker closed.
    Left,
}

#[derive(Debug, PartialEq)]
pub enum Effect<D = SavedDraft> {
    Start { attempt: Attempt, session: String, message: String },
    /// Open the forked session with the prompt it was forked before in its composer.
    Open { forked: String, draft: D },
    CloseDialog,
    Fail { why: String },
}

impl<D> Fork<D> {
    /// The prompt being forked from, which the picker shows as pending.
    pub fn pending(&self) -> Option<&str> {
        match &self.state {
            State::Forking { message, .. } => Some(message),
            State::Idle => None,
        }
    }
}

impl<D: Clone> Fork<D> {
    pub fn step(&mut self, event: Event<D>) -> Effects<Effect<D>> {
        match (&self.state, event) {
            (State::Idle, Event::Requested { surface, session, message, draft, now }) => {
                let attempt = self.seq.next();
                self.state = State::Forking { attempt, surface, session: session.clone(), message: message.clone(), draft, due: now + TIMEOUT_SECS };
                Effects::one(Effect::Start { attempt, session, message })
            }
            (State::Forking { attempt, surface, draft, .. }, Event::Succeeded { attempt: a, forked }) if *attempt == a => {
                let (surface, draft) = (*surface, draft.clone());
                self.state = State::Idle;
                let open = Effect::Open { forked, draft };
                if surface == Surface::Dialog { Effects::two(open, Effect::CloseDialog) } else { Effects::one(open) }
            }
            (State::Forking { attempt, .. }, Event::Failed { attempt: a, why }) if *attempt == a => {
                self.state = State::Idle;
                Effects::one(Effect::Fail { why })
            }
            (State::Forking { due, .. }, Event::Due { now }) if now + 1e-9 >= *due => {
                self.state = State::Idle;
                Effects::one(Effect::Fail { why: String::new() })
            }
            (State::Forking { .. }, Event::Left) => {
                self.state = State::Idle;
                Effects::none()
            }
            _ => Effects::none(),
        }
    }
}
