//! Find's explicit state machine: Desktop's `TimelineSearchState`
//! (`session/timeline/search-machine.ts:30-33`).

use rune::machine::Effects;

/// Find's state: Desktop's `TimelineSearchState` (`session/timeline/search-machine.ts:30-33`).
/// Its query is the bar's field (`input`), its matches the index [`crate::Find`] keeps;
/// `seeking` has no counterpart, as Rune scrolls to a row that isn't built yet in the frame it
/// asks.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Closed,
    /// Open on session `session`'s matches; `active` is the lit one's place in them (0 with none).
    Open { session: String, active: usize },
}

/// `TimelineSearchEvent`, less the seek's.
#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    /// `open`: Find opens on `session`, or focuses and selects its query when it is open.
    Open { session: &'a str },
    Close,
    /// `query.changed`: the first match lights.
    QueryChanged,
    /// `move`: the lit match steps by `delta` of `count`, wrapping.
    Move { delta: i32, count: usize },
    /// `session.changed`: another session's timeline shows; Find stays open on its matches.
    SessionChanged { session: &'a str },
    /// `matches.synced`: the matches changed; `kept` is where the lit one is among them now.
    MatchesSynced { kept: Option<usize> },
    /// `disposed`: no session's timeline shows (Home), so Find closes.
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    FocusInput,
    BlurInput,
    /// Bring the lit match into view.
    Reveal,
}

impl State {
    /// `transitionTimelineSearch`: pure transition over [`State`].
    pub fn step(&mut self, event: Event<'_>) -> Effects<Effect> {
        transition(self, event)
    }
}

/// `transitionTimelineSearch`.
pub fn transition(state: &mut State, event: Event<'_>) -> Effects<Effect> {
    match (&mut *state, event) {
        (State::Closed, Event::Open { session }) => {
            *state = State::Open { session: session.to_owned(), active: 0 };
            Effects::one(Effect::FocusInput)
        }
        (State::Closed, _) => Effects::none(),
        // Already open: focus and select its query again.
        (State::Open { .. }, Event::Open { .. }) => Effects::one(Effect::FocusInput),
        (State::Open { .. }, Event::Close) => {
            *state = State::Closed;
            Effects::one(Effect::BlurInput)
        }
        (State::Open { .. }, Event::Left) => {
            *state = State::Closed;
            Effects::none()
        }
        (State::Open { active, .. }, Event::QueryChanged) => {
            *active = 0;
            Effects::one(Effect::Reveal)
        }
        (State::Open { active, .. }, Event::Move { delta, count }) => {
            if delta == 0 || count == 0 {
                return Effects::none();
            }
            *active = (*active as i64 + i64::from(delta)).rem_euclid(count as i64) as usize;
            Effects::one(Effect::Reveal)
        }
        (State::Open { session, active }, Event::SessionChanged { session: next }) => {
            if session != next {
                *session = next.to_owned();
                *active = 0;
            }
            Effects::none()
        }
        // The lit match keeps its place by identity, so a list that shrinks and grows again
        // never brings an old index back.
        (State::Open { active, .. }, Event::MatchesSynced { kept }) => {
            *active = kept.unwrap_or(0);
            Effects::none()
        }
    }
}
