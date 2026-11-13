//! What the pin board does with input, Desktop's `new-session/pin-board-machine.ts`: where each
//! pin is, and the board's own phase.

/// Where a pin is: in its socket, flying, piled on the composer or floor, flying home, or knocked
/// off the panel by the Brick Breaker ball (its socket stays empty until the game ends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinPhase {
    Home,
    Air,
    Rest,
    Return,
    Out,
}

#[derive(Clone, Copy, Debug)]
pub struct Pin {
    pub c: u8,
    pub r: u8,
    /// 1 = counter, 2 = "open", 3 = "code".
    pub lv: u8,
    pub hx: f64,
    pub hy: f64,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub z: f64,
    pub zv: f64,
    pub e: f64,
    pub rot: f64,
    pub spin: f64,
    pub phase: PinPhase,
    pub pop: f64,
    pub pop_at: f64,
    pub bucket: usize,
    pub on_composer: bool,
    pub level: i16,
    pub x0: f64,
    pub y0: f64,
    pub rot0: f64,
    pub t0: f64,
    pub duration: f64,
    pub lift: f64,
    pub fd: f64,
    pub bd: f64,
}

impl Pin {
    pub fn new(c: u8, r: u8, lv: u8) -> Pin {
        Pin {
            c, r, lv, hx: 0.0, hy: 0.0, x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, z: 0.0, zv: 0.0, e: 0.0, rot: 0.0, spin: 0.0,
            phase: PinPhase::Home, pop: 0.0, pop_at: 0.0, bucket: 0, on_composer: false, level: 0, x0: 0.0, y0: 0.0, rot0: 0.0,
            t0: 0.0, duration: 0.0, lift: 0.0, fd: 0.0, bd: 1.0,
        }
    }
}

/// The board's phase (`PinBoardState`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Board {
    /// A held press keeps knocking along its path from the last knock at `drag_x`, `drag_y`.
    Interactive { dragging: bool, drag_x: f64, drag_y: f64, knocked_at: f64 },
    Game,
    Closing { closed_at: f64 },
    Disposed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    PointerDown { x: f64, y: f64 },
    PointerDragged { x: f64, y: f64 },
    PointerUp,
    Knocked { now: f64 },
    Play,
    Restore { now: f64 },
    Close { now: f64 },
    Dispose,
}

impl Board {
    pub fn new() -> Board {
        Board::Interactive { dragging: false, drag_x: 0.0, drag_y: 0.0, knocked_at: -99.0 }
    }
}

/// `transitionPinBoard`: the same state when nothing changes.
pub fn step(state: Board, event: Event) -> Board {
    if state == Board::Disposed {
        return state;
    }
    if event == Event::Dispose {
        return Board::Disposed;
    }
    if matches!(state, Board::Closing { .. }) {
        return state;
    }
    if let Event::Close { now } = event {
        return Board::Closing { closed_at: now };
    }
    if state == Board::Game {
        return if let Event::Restore { now } = event { Board::Interactive { dragging: false, drag_x: 0.0, drag_y: 0.0, knocked_at: now } } else { state };
    }
    if event == Event::Play {
        return Board::Game;
    }
    let Board::Interactive { dragging, drag_x, drag_y, knocked_at } = state else { return state };
    // The knock a press or drag makes reports its own hits through `Knocked`.
    match event {
        Event::PointerDown { x, y } => Board::Interactive { dragging: true, drag_x: x, drag_y: y, knocked_at },
        Event::PointerDragged { x, y } if dragging => Board::Interactive { dragging, drag_x: x, drag_y: y, knocked_at },
        Event::PointerUp if dragging => Board::Interactive { dragging: false, drag_x, drag_y, knocked_at },
        Event::Knocked { now } => Board::Interactive { dragging, drag_x, drag_y, knocked_at: now },
        _ => state,
    }
}

/// Pointer input moves and knocks pins only on the interactive board, not in a game or while packing up.
pub fn accepts_pointer(state: Board) -> bool {
    matches!(state, Board::Interactive { .. })
}

pub fn accepts_game_input(state: Board) -> bool {
    state == Board::Game
}

pub fn dragging(state: Board) -> bool {
    matches!(state, Board::Interactive { dragging: true, .. })
}

/// A held press knocks again once it moves far enough from its last knock to read as a trail.
pub fn drag_knock_due(state: Board, x: f64, y: f64, pitch: f64) -> bool {
    matches!(state, Board::Interactive { dragging: true, drag_x, drag_y, .. } if (x - drag_x).hypot(y - drag_y) > pitch * 1.2)
}

/// Loose pins fly home once they settle, or after a while even if some are still bouncing.
pub fn should_reassemble(state: Board, air: usize, loose: usize, now: f64) -> bool {
    let Board::Interactive { dragging: false, knocked_at, .. } = state else { return false };
    if loose == 0 {
        return false;
    }
    let idle = now - knocked_at;
    (air == 0 && idle > 2.0) || idle > 6.0
}

pub fn close_age(state: Board, now: f64) -> f64 {
    if let Board::Closing { closed_at } = state { now - closed_at } else { -1.0 }
}

/// Moves every socket to the new grid after a resize, so a game, the closing wave or loose pins
/// survive it. Socketed pins follow their socket; flying, returning and knocked-out pins keep
/// their phase; piled pins drop to pile again on the new surfaces, since the piles are counted
/// per bucket of the old grid.
pub fn relayout(pins: &mut [Pin], x0: f64, y0: f64, pitch: f64) {
    for p in pins {
        p.hx = x0 + (f64::from(p.c) + 0.5) * pitch;
        p.hy = y0 + (f64::from(p.r) + 0.5) * pitch;
        if p.phase == PinPhase::Home {
            p.x = p.hx;
            p.y = p.hy;
            continue;
        }
        if p.phase != PinPhase::Rest {
            continue;
        }
        p.phase = PinPhase::Air;
        p.vx = 0.0;
        p.vy = 0.0;
    }
}

pub fn home_count(pins: &[Pin]) -> usize {
    pins.iter().filter(|p| p.phase == PinPhase::Home).count()
}
