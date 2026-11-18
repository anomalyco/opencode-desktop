//! Brick Breaker's phases, Desktop's `new-session/brick-breaker-machine.ts` state for state. Each
//! timed phase counts from `started_at`, on the frame loop's clock (seconds). `landed` marks the
//! ball resting on the paddle, which it must before it can be served.

pub const LIVES: u8 = 3;

/// Phase lengths in seconds, shared with the animations that fill them (`BRICK_BREAKER_TIMING`).
pub mod timing {
    /// The ball lands on the shrinking composer, which has become the paddle by `ENTER`.
    pub const ENTER_LAND: f64 = 0.38;
    pub const ENTER: f64 = 0.85;
    /// After a miss the ball drops back onto the paddle before it can be served.
    pub const SERVE_DROP: f64 = 0.42;
    pub const MISS: f64 = 0.55;
    pub const EXIT: f64 = 1.0;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Win,
    Lose,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Enter { started_at: f64, landed: bool },
    Serve { started_at: f64, landed: bool, lives: u8 },
    Play { lives: u8 },
    Miss { started_at: f64, lives: u8 },
    Exit { started_at: f64, kind: Exit },
    /// The exit morph finished or the game unmounted.
    Disposed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Frame { now: f64 },
    Launch,
    Cleared { now: f64 },
    Missed { now: f64 },
    Quit { now: f64 },
    Dispose,
}

impl State {
    pub fn new(now: f64) -> State {
        State::Enter { started_at: now, landed: false }
    }
}

/// `transitionBrickBreaker`: the same state when no phase threshold is crossed.
pub fn step(state: State, event: Event) -> State {
    use timing::*;
    if state == State::Disposed {
        return state;
    }
    match event {
        Event::Dispose => return State::Disposed,
        Event::Quit { now } => return if matches!(state, State::Exit { .. }) { state } else { State::Exit { started_at: now, kind: Exit::Quit } },
        Event::Launch => return if let State::Serve { landed: true, lives, .. } = state { State::Play { lives } } else { state },
        Event::Cleared { now } => return if matches!(state, State::Play { .. }) { State::Exit { started_at: now, kind: Exit::Win } } else { state },
        Event::Missed { now } => return if let State::Play { lives } = state { State::Miss { started_at: now, lives } } else { state },
        Event::Frame { .. } => {}
    }
    let Event::Frame { now } = event else { return state };
    match state {
        State::Play { .. } | State::Disposed => state,
        State::Enter { started_at, landed } => {
            let elapsed = now - started_at;
            if elapsed >= ENTER {
                return State::Serve { started_at: now, landed: true, lives: LIVES };
            }
            if !landed && elapsed >= ENTER_LAND { State::Enter { started_at, landed: true } } else { state }
        }
        State::Serve { started_at, landed, lives } => {
            if !landed && now - started_at >= SERVE_DROP { State::Serve { started_at, landed: true, lives } } else { state }
        }
        State::Miss { started_at, lives } => {
            if now - started_at < MISS {
                return state;
            }
            if lives <= 1 {
                return State::Exit { started_at: now, kind: Exit::Lose };
            }
            State::Serve { started_at: now, landed: false, lives: lives - 1 }
        }
        State::Exit { started_at, .. } => if now - started_at >= EXIT { State::Disposed } else { state },
    }
}

/// Input steers and serves until the exit morph starts.
pub fn accepts_input(state: State) -> bool {
    !matches!(state, State::Exit { .. } | State::Disposed)
}

/// The paddle follows input once the composer has shrunk into it, until it grows back.
pub fn moves_paddle(state: State) -> bool {
    matches!(state, State::Serve { .. } | State::Play { .. } | State::Miss { .. })
}

pub fn ball_moving(state: State) -> bool {
    matches!(state, State::Play { .. } | State::Miss { .. })
}

/// Spare lives shown beside the paddle, not counting the ball in play.
pub fn spares(state: State) -> u8 {
    match state {
        State::Enter { .. } => LIVES - 1,
        State::Serve { lives, .. } | State::Play { lives } | State::Miss { lives, .. } => lives - 1,
        _ => 0,
    }
}
