//! The new-session wordmark and the lodger that lives in its final "e", Desktop's
//! `new-session/wordmark-machine.ts` state for state. `Board` is the static pin board (Desktop's
//! Storybook `pins` start; never shown in the app) with no lodger. Pulling the lodger out (`Drag`)
//! either snaps it back (`Snap`) or pops the pin board out (`Show`); from its perch (`Idle`) it can
//! be carried (`Carry`) back into the hole (`Pack`), dropped on the composer to play Brick Breaker
//! (`Game`), or let go to hop home (`Hop`, `Return`).
//!
//! `board` says the pin board is mounted (Desktop holds its handle while it is). `cue` counts the
//! timed steps of an animation already run, so each runs once. In `Game`, `ball` follows the ball
//! after Brick Breaker ends: it flies home (`Returning`), then waits there (`Landed`) until Brick
//! Breaker finishes its exit and unmounts.

use rune::machine::Effects;

/// Pulling the lodger further than this, in logo units, pops every pin out into the pin board.
pub const LODGER_POP: f64 = 26.0;
/// A carried lodger released closer than this to where it was picked up hops in place.
const LODGER_TAP: f64 = 4.0;

/// The pin board's mounted handle, which the machine only holds and hands over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ball {
    Active,
    Returning,
    Landed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Board,
    Hidden,
    Drag,
    Snap,
    Show { board: Option<Board>, cue: u8 },
    Idle { board: Option<Board> },
    Hop { board: Option<Board>, cue: u8 },
    Carry { board: Option<Board>, over: bool },
    Game { board: Board, ball: Ball },
    Return { board: Option<Board> },
    Pack { board: Option<Board>, cue: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Snap,
    Show,
    Hop,
    Return,
    Pack,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    BoardMounted,
    Grab,
    DragSlip { reduced_motion: bool },
    /// `near_hole`: the lodger, its drop point or the pointer is within the "e" hole's pull.
    Release { distance: f64, near_hole: bool, reduced_motion: bool },
    /// Sent only when the carried lodger reaches the hole or crosses the composer's edge.
    CarryFrame { in_hole: bool, over: bool, reduced_motion: bool },
    CueStep { phase: Phase, cue: u8 },
    Settled { phase: Phase },
    /// Brick Breaker ended and sent the ball home.
    BallDone,
    /// Brick Breaker finished its exit and unmounts.
    GameDone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    FocusComposer,
}

impl State {
    pub fn new(pins: bool) -> State {
        if pins { State::Board } else { State::Hidden }
    }

    fn board(self) -> Option<Option<Board>> {
        match self {
            State::Show { board, .. } | State::Idle { board } | State::Hop { board, .. } | State::Carry { board, .. } | State::Return { board } | State::Pack { board, .. } => Some(board),
            State::Game { board, .. } => Some(Some(board)),
            _ => None,
        }
    }
}

/// `transitionWordmark`.
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let effects = if event == Event::GameDone && matches!(state, State::Game { .. }) { Effects::one(Effect::FocusComposer) } else { Effects::none() };
    *state = next(*state, event);
    effects
}

fn next(state: State, event: Event) -> State {
    match event {
        Event::BoardMounted => {
            if state.board().is_none_or(|b| b == Some(Board)) {
                return state;
            }
            return match state {
                State::Show { cue, .. } => State::Show { board: Some(Board), cue },
                State::Idle { .. } => State::Idle { board: Some(Board) },
                State::Hop { cue, .. } => State::Hop { board: Some(Board), cue },
                State::Carry { over, .. } => State::Carry { board: Some(Board), over },
                State::Return { .. } => State::Return { board: Some(Board) },
                State::Pack { cue, .. } => State::Pack { board: Some(Board), cue },
                s => s,
            };
        }
        Event::Grab => {
            return match state {
                State::Hidden => State::Drag,
                State::Idle { board } => State::Carry { board, over: false },
                s => s,
            };
        }
        Event::DragSlip { reduced_motion } => return if state == State::Drag { pop(reduced_motion) } else { state },
        Event::Release { distance, near_hole, reduced_motion } => return release(state, distance, near_hole, reduced_motion),
        Event::CarryFrame { in_hole, over, reduced_motion } => {
            let State::Carry { board, over: was } = state else { return state };
            if in_hole {
                return pack(board, reduced_motion);
            }
            return if was == over { state } else { State::Carry { board, over } };
        }
        Event::CueStep { phase, cue } => {
            return match state {
                State::Show { board, cue: at } if phase == Phase::Show && cue > at => State::Show { board, cue },
                State::Hop { board, cue: at } if phase == Phase::Hop && cue > at => State::Hop { board, cue },
                // The pin board unmounts once the last pin slots back in.
                State::Pack { board, cue: at } if phase == Phase::Pack && cue > at => State::Pack { board: if cue >= 2 { None } else { board }, cue },
                s => s,
            };
        }
        Event::Settled { phase } => return settle(state, phase),
        Event::BallDone | Event::GameDone => {}
    }
    let State::Game { board, ball } = state else { return state };
    if event == Event::BallDone {
        return if ball == Ball::Active { State::Game { board, ball: Ball::Returning } } else { state };
    }
    if ball == Ball::Returning {
        return State::Return { board: Some(board) };
    }
    State::Idle { board: Some(board) }
}

fn release(state: State, distance: f64, near_hole: bool, reduced_motion: bool) -> State {
    if state == State::Drag {
        if distance > LODGER_POP {
            return pop(reduced_motion);
        }
        return if reduced_motion { State::Hidden } else { State::Snap };
    }
    let State::Carry { board, over } = state else { return state };
    if distance < LODGER_TAP {
        return if reduced_motion { State::Idle { board } } else { State::Hop { board, cue: 0 } };
    }
    if over && !reduced_motion && let Some(board) = board {
        return State::Game { board, ball: Ball::Active };
    }
    if near_hole {
        return pack(board, reduced_motion);
    }
    if reduced_motion { State::Idle { board } } else { State::Return { board } }
}

fn pop(reduced_motion: bool) -> State {
    if reduced_motion { State::Idle { board: None } } else { State::Show { board: None, cue: 0 } }
}

fn pack(board: Option<Board>, reduced_motion: bool) -> State {
    if reduced_motion { State::Hidden } else { State::Pack { board, cue: 0 } }
}

fn settle(state: State, phase: Phase) -> State {
    if phase == Phase::Return && let State::Game { board, ball } = state {
        return if ball == Ball::Returning { State::Game { board, ball: Ball::Landed } } else { state };
    }
    match (state, phase) {
        (State::Snap, Phase::Snap) | (State::Pack { .. }, Phase::Pack) => State::Hidden,
        (State::Show { board, .. }, Phase::Show) | (State::Hop { board, .. }, Phase::Hop) | (State::Return { board }, Phase::Return) => State::Idle { board },
        _ => state,
    }
}

/// How the pin board enters, and whether it is closing, while it is mounted: from the moment the
/// lodger pops it out until the last pin slots back into the "e".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinBoardSel {
    pub sweep: bool,
    pub closing: bool,
}

pub fn pin_board(state: State) -> Option<PinBoardSel> {
    match state {
        State::Hidden | State::Drag | State::Snap => None,
        State::Pack { cue, .. } if cue >= 2 => None,
        s => Some(PinBoardSel { sweep: s == State::Board, closing: matches!(s, State::Pack { .. }) }),
    }
}

pub fn brick_breaker(state: State) -> bool {
    matches!(state, State::Game { .. })
}

pub fn flat_visible(state: State) -> bool {
    match state {
        State::Show { cue, .. } => cue < 2,
        s => matches!(s, State::Hidden | State::Drag | State::Snap | State::Pack { .. }),
    }
}

/// Where the lodger can be grabbed: in the "e" counter, on its perch beside the board, or nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    Hidden,
    Idle,
    None,
}

pub fn grip(state: State) -> Grip {
    match state {
        State::Hidden | State::Drag | State::Snap => Grip::Hidden,
        State::Idle { .. } | State::Carry { .. } | State::Hop { .. } | State::Return { .. } => Grip::Idle,
        _ => Grip::None,
    }
}

/// Carried, returning, or in play, the lodger crosses the composer, so it is lifted above it.
pub fn lifted(state: State) -> bool {
    match state {
        State::Game { ball, .. } => ball != Ball::Landed,
        s => matches!(s, State::Carry { .. } | State::Return { .. }),
    }
}

pub fn drop_over(state: State) -> bool {
    matches!(state, State::Carry { over: true, .. })
}

pub fn animating(state: State) -> bool {
    match state {
        State::Game { ball, .. } => ball == Ball::Returning,
        s => !matches!(s, State::Board | State::Hidden | State::Idle { .. }),
    }
}

/// The lodger follows the pointer and can be grabbed only while it rests.
pub fn can_interact(state: State) -> bool {
    matches!(state, State::Hidden | State::Idle { .. })
}
