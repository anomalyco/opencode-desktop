//! Find's explicit state machine: Desktop's `TimelineSearchState`
//! (`session/timeline/search-machine.ts:30-33`).

use std::sync::Arc;

use rune::machine::Effects;

use crate::scan::{Answer, CopyKey, Copying, Indexing, Matching, Target};
use crate::transcript_index::{Index, Source};

/// Pipeline state owned by an open Find session; dropped structurally when Find closes.
pub struct OpenFind {
    pub session: String,
    pub active: usize,
    /// The open session's transcript as last checked, and its shape.
    pub(crate) checked: Option<(CopyKey, u64)>,
    pub(crate) copying: Copying,
    /// Its rows, copied when its shape changes while Find is open, and their index.
    pub(crate) source: Option<Arc<Source>>,
    pub(crate) index: Option<Arc<Index>>,
    pub(crate) answer: Answer,
    pub(crate) indexing: Indexing,
    pub(crate) matching: Matching,
    pub(crate) target: Target,
}

impl OpenFind {
    pub(crate) fn new(session: String) -> Self {
        Self {
            session,
            active: 0,
            checked: None,
            copying: Copying::Idle,
            source: None,
            index: None,
            answer: Answer::None,
            indexing: Indexing::Idle,
            matching: Matching::Idle,
            target: Target::First,
        }
    }
}

/// Find's state: Desktop's `TimelineSearchState` (`session/timeline/search-machine.ts:30-33`).
/// Its query is the bar's field (`input`), and its open variant owns the incremental indexing and
/// matching pipeline so closing drops it structurally; `seeking` has no counterpart, as Rune
/// scrolls to a row that isn't built yet in the frame it asks.
#[derive(Default)]
pub enum State {
    #[default]
    Closed,
    /// Open on a session's matches, owning its search pipeline until closed.
    Open(Box<OpenFind>),
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

/// Superseded match results to drop off-thread via [`crate::retire`].
pub struct RetiredAnswer(#[allow(dead_code)] Answer);

pub enum Effect {
    FocusInput,
    BlurInput,
    /// Bring the lit match into view.
    Reveal,
    /// Cancel workers, clear input, and drop the closed pipeline off-thread.
    Release(Box<OpenFind>),
    /// Drop superseded session match results off-thread.
    RetireAnswer(RetiredAnswer),
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
            *state = State::Open(Box::new(OpenFind::new(session.to_owned())));
            Effects::one(Effect::FocusInput)
        }
        (State::Closed, _) => Effects::none(),
        // Already open: focus and select its query again.
        (State::Open(_), Event::Open { .. }) => Effects::one(Effect::FocusInput),
        (State::Open(_), Event::Close) => {
            let State::Open(open) = std::mem::replace(state, State::Closed) else { unreachable!() };
            Effects::two(Effect::Release(open), Effect::BlurInput)
        }
        (State::Open(_), Event::Left) => {
            let State::Open(open) = std::mem::replace(state, State::Closed) else { unreachable!() };
            Effects::one(Effect::Release(open))
        }
        (State::Open(open), Event::QueryChanged) => {
            open.active = 0;
            Effects::one(Effect::Reveal)
        }
        (State::Open(open), Event::Move { delta, count }) => {
            if delta == 0 || count == 0 {
                return Effects::none();
            }
            open.active = (open.active as i64 + i64::from(delta)).rem_euclid(count as i64) as usize;
            Effects::one(Effect::Reveal)
        }
        (State::Open(open), Event::SessionChanged { session: next }) => {
            if open.session != next {
                open.session = next.to_owned();
                open.active = 0;
                let old = std::mem::take(&mut open.answer);
                if !matches!(old, Answer::None) {
                    return Effects::one(Effect::RetireAnswer(RetiredAnswer(old)));
                }
            }
            Effects::none()
        }
        // The lit match keeps its place by identity, so a list that shrinks and grows again
        // never brings an old index back.
        (State::Open(open), Event::MatchesSynced { kept }) => {
            open.active = kept.unwrap_or(0);
            Effects::none()
        }
    }
}
