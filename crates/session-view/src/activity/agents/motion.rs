//! How the subagents line moves when what it shows changes (Rune-only, see [`super`]).
//!
//! Rune moves only what an input in the window caused. A commit a server event brings is an
//! unprompted one (`rune::Tx::animated`'s docs, `Commit::Unprompted`): its layout, its
//! `opacity_motion` targets and the elements it mounts or drops all land at once, so a crossfade
//! on `opacity_motion`, a presence `transition`, and `opencode_ui::odometer`'s roll (each glyph a
//! presence transition) never play for a subagent starting or finishing. A one-shot keyframe
//! `El::sequence` runs on the render clock whatever caused its commit, landing at once only
//! under Reduce Motion or a settle, and a new key replays it. So each change the line draws
//! starts a move: what changed draws under keys of that move (its `nth`), playing sequences,
//! and the first draw after [`MOVE`] draws the line at rest again, sequences gone.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::Status;

/// How long a move plays: the crossfade and the roll, with room to land.
pub const MOVE: f64 = 0.4;

/// Lines remembered per thread; past it they're forgotten and draw their next look at rest.
const KEPT: usize = 256;

/// Whether the line names its agents or counts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    Names,
    Counts,
}

/// What the line shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub shows: Shows,
    pub total: usize,
    /// By [`Status`], in its count places' order.
    pub counts: [usize; 4],
    /// Each agent (its id) and where it is, in the line's order.
    pub agents: Vec<(u64, Status)>,
}

/// The line's motion: an explicit machine (AGENTS.md), one per line.
#[derive(Clone, Debug)]
pub enum State {
    /// Never drawn: its first look shows at rest, as a timeline opening shows what's there.
    Unseen,
    /// At rest on `look`; `nth`: the last move's.
    Still { look: Look, nth: u64 },
    /// Moving from `from` to `to` since `at` on the app clock.
    Moving { from: Look, to: Look, nth: u64, at: f64 },
}

/// What happens to the line.
pub enum Event {
    /// The line drew `look` at `now` on the app clock; `calm`: Reduce Motion is on.
    Drawn { look: Look, now: f64, calm: bool },
}

/// The line's one transition. It has no effects for its caller to run: a move plays on Rune's
/// render clock, and lands on the first draw after [`MOVE`], which is when the time arrives as
/// an event (the row draws again then, [`moving`]). Under Reduce Motion every change lands.
pub fn step(state: &mut State, event: Event) {
    let Event::Drawn { look, now, calm } = event;
    if let State::Moving { to, nth, at, .. } = state
        && (calm || now >= *at + MOVE)
    {
        *state = State::Still { look: to.clone(), nth: *nth };
    }
    let next = match &*state {
        State::Unseen => State::Still { look, nth: 0 },
        State::Still { look: was, .. } | State::Moving { to: was, .. } if *was == look => return,
        State::Still { nth, .. } | State::Moving { nth, .. } if calm => State::Still { look, nth: *nth },
        State::Still { look: was, nth } => State::Moving { from: was.clone(), to: look, nth: nth + 1, at: now },
        // A change during a move moves on from where that one was headed.
        State::Moving { to, nth, .. } => State::Moving { from: to.clone(), to: look, nth: nth + 1, at: now },
    };
    *state = next;
}

thread_local! {
    static LINES: RefCell<HashMap<u64, State>> = RefCell::new(HashMap::new());
}

/// Which line: the row `key` of session `session`.
fn line_id(session: &str, key: &str) -> u64 {
    let mut h = DefaultHasher::new();
    (session, key).hash(&mut h);
    h.finish()
}

/// Steps line `key` of `session` on its drawing `look`, and returns its state. Every build of
/// one commit steps from where the commit found it (`rune::memo::snapshot`), so a layout pass
/// that's thrown away doesn't take the move's start from the one that stays.
pub fn drawn(session: &str, key: &str, look: Look, now: f64, calm: bool) -> State {
    let id = line_id(session, key);
    let found = LINES.with_borrow(|m| m.get(&id).cloned()).unwrap_or(State::Unseen);
    let mut state = (*rune::memo::snapshot(&format!("subagent-line-{id:x}"), &found)).clone();
    step(&mut state, Event::Drawn { look, now, calm });
    LINES.with_borrow_mut(|m| {
        if m.len() >= KEPT && !m.contains_key(&id) {
            m.clear();
        }
        m.insert(id, state.clone());
    });
    state
}

/// The move line `key` of `session` is playing at `now`, for its row's memo: when the move is
/// over the row builds again and draws it at rest, so mounting the row again later (scrolled back
/// into view) doesn't play it over.
pub fn moving(session: &str, key: &str, now: f64) -> Option<u64> {
    LINES.with_borrow(|m| match m.get(&line_id(session, key)) {
        Some(State::Moving { nth, at, .. }) if now < at + MOVE => Some(*nth),
        _ => None,
    })
}
