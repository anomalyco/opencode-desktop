//! How an open submenu outlives the pointer crossing its menu (`menu/submenu-machine.ts` and the
//! Kobalte pointer-grace patch, anomalyco/opencode#63739).
//!
//! - **Aiming:** the pointer left the submenu's row heading for it. The rows it crosses on the
//!   way don't light while each move still points into the submenu's near edge; each such move
//!   renews [`GRACE_S`]. A move that heads past the edge, or the grace running out, ends it, and
//!   the row under the pointer lights.
//! - **Leaving:** the pointer lit another row of the menu that opened the submenu. The submenu
//!   stays drawn for [`LEAVE_S`], so a row that opens its own submenu replaces it in the same
//!   frame, and the pointer can still reach it, instead of every move closing it and leaving the
//!   menu without one until the next opens.

/// Longer than the hover that opens a submenu, so the next one replaces the leaving one in one frame.
pub(super) const LEAVE_S: f64 = 0.15;
/// How long a move toward the submenu keeps the rows it crosses dark.
pub(super) const GRACE_S: f64 = 0.3;
/// Moves shorter than this are judged with the next one.
const STEP: f32 = 4.0;
/// The grace area starts this far behind the pointer as it leaves the row.
const BEHIND: f32 = 5.0;

/// The submenu opened from a row of the menu `level` deep, as the pointer moves.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) enum State {
    #[default]
    Steady,
    /// Heading for the submenu from `from`; row `row` of that menu is under the pointer, unlit.
    Aiming { level: usize, from: [f32; 2], row: usize, until: f64 },
    Leaving { level: usize, until: f64 },
}

/// The submenu's edge facing the menu that opened it, in window points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Target {
    pub(super) edge: f32,
    pub(super) top: f32,
    pub(super) bottom: f32,
    /// It opened to the right of its row (to the left in a right-to-left window).
    pub(super) right: bool,
}

pub(super) enum Event {
    /// Row `row` of the menu `level` deep, not the open submenu's, is under the pointer at `at`.
    /// `left` is where the pointer was when it was last on the submenu's row, if it just left it.
    Crossed { level: usize, row: usize, at: [f32; 2], left: Option<[f32; 2]>, target: Option<Target>, now: f64 },
    /// The pointer moved to `at` while aiming.
    Moved { at: [f32; 2], target: Option<Target>, now: f64 },
    /// The pointer is back on the submenu's row or in the submenu, or another submenu opened.
    Returned,
    Tick { now: f64 },
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    None,
    /// Light `row` of the menu `level` deep: the pointer is no longer on its way.
    Light { level: usize, row: usize, wake: f64 },
    /// Close the submenu opened from the menu `level` deep.
    Close(usize),
    WakeAt(f64),
}

pub(super) fn step(state: &mut State, event: Event) -> Effect {
    match event {
        Event::Crossed { level, row, at, left, target, now } => {
            let from = match (*state, left, target) {
                (State::Aiming { level: l, from, until, .. }, _, _) if l == level && now < until => Some(from),
                (_, Some(left), Some(t)) => Some(if t.right { [left[0] - BEHIND, left[1]] } else { [left[0] + BEHIND, left[1]] }),
                _ => None,
            };
            match from {
                Some(from) => aim(state, level, row, from, at, target, now),
                None => leave(state, level, row, now),
            }
        }
        Event::Moved { at, target, now } => match *state {
            State::Aiming { level, from, row, until } if now < until => aim(state, level, row, from, at, target, now),
            State::Aiming { level, row, .. } => leave(state, level, row, now),
            _ => Effect::None,
        },
        Event::Returned => {
            *state = State::Steady;
            Effect::None
        }
        Event::Tick { now } => match *state {
            State::Aiming { level, row, until, .. } if now + 1e-3 >= until => leave(state, level, row, now),
            State::Leaving { level, until } if now + 1e-3 >= until => {
                *state = State::Steady;
                Effect::Close(level)
            }
            State::Aiming { until, .. } | State::Leaving { until, .. } => Effect::WakeAt(until),
            State::Steady => Effect::None,
        },
    }
}

/// A move from `from` to `at` still on its way into `target`'s edge keeps the row dark.
fn aim(state: &mut State, level: usize, row: usize, from: [f32; 2], at: [f32; 2], target: Option<Target>, now: f64) -> Effect {
    let Some(t) = target else { return leave(state, level, row, now) };
    let (dx, dy) = (at[0] - from[0], at[1] - from[1]);
    if dx.hypot(dy) < STEP {
        let until = now + GRACE_S;
        *state = State::Aiming { level, from, row, until };
        return Effect::WakeAt(until);
    }
    let advancing = if t.right { dx > 0.0 } else { dx < 0.0 };
    let edge_y = from[1] + dy * ((t.edge - from[0]) / dx).max(0.0);
    if !advancing || edge_y < t.top || edge_y > t.bottom {
        return leave(state, level, row, now);
    }
    let until = now + GRACE_S;
    *state = State::Aiming { level, from: at, row, until };
    Effect::WakeAt(until)
}

fn leave(state: &mut State, level: usize, row: usize, now: f64) -> Effect {
    let until = now + LEAVE_S;
    *state = State::Leaving { level, until };
    Effect::Light { level, row, wake: until }
}
