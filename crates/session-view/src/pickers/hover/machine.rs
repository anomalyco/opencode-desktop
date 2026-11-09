//! The flyout beside the model dialog's lit row: Desktop's `DialogFlyoutState` and
//! `transitionDialogFlyout` (`providers/models/select-dialog-machine.ts:30-328`), state for state
//! and transition for transition. The flyout is laid out against its row's anchor, so Desktop's
//! `place` effect and `placed` event, which measure the row by hand, have no counterpart here.

/// `FLYOUT_OPEN_DELAY_MS`: how long the pointer rests on a row before its flyout opens, unless
/// a flyout was just open.
pub const OPEN_DELAY: f64 = 0.1;
/// `FLYOUT_AIM_TIMEOUT_MS`: how long the pointer may rest on a row inside the safe triangle
/// before the flyout moves to that row.
pub const AIM_TIMEOUT: f64 = 0.3;
/// `FLYOUT_WARM_MS`: how long after a flyout closes that the next row's flyout still opens at once.
pub const WARM: f64 = 0.3;

/// A point in window points.
pub type Point = [f32; 2];
/// A rect in window points: x, y, width, height.
pub type Rect = [f32; 4];

/// What moved the highlight last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Keyboard,
    Pointer,
}

/// `DialogFlyoutState`, less `disposed`: the dialog makes a fresh one each time it opens.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// `warm` holds for a moment after a flyout closes under the pointer.
    Closed { attempt: u32, input: Input, warm: bool },
    /// The pointer rests on row `id`; its flyout opens after the delay.
    Opening { attempt: u32, id: String, apex: Point },
    Open { attempt: u32, id: String, apex: Point },
    /// The pointer crossed onto row `target` on its way to the flyout; the flyout stays on `id`
    /// while it keeps moving.
    Aiming { attempt: u32, id: String, apex: Point, target: String },
}

impl Default for State {
    /// `createDialogFlyoutState`.
    fn default() -> State {
        State::Closed { attempt: 0, input: Input::Keyboard, warm: false }
    }
}

/// Where the pointer went when it left a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Onto {
    Row,
    Flyout,
    Outside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timer {
    Open,
    Aim,
    Warm,
}

/// `DialogFlyoutEvent`, less `placed` and `disposed`.
#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    /// Arrow keys, typing, a favorite toggled or an effort staged from the keyboard.
    Keyboard,
    Wheel,
    RowMoved { id: &'a str, active: Option<&'a str>, point: Point, flyout: Option<Rect> },
    RowLeft { onto: Onto, point: Point, flyout: Option<Rect> },
    /// The pointer moved somewhere other than a row or the flyout while a flyout is open.
    PointerOutside { point: Point, flyout: Option<Rect> },
    FlyoutEntered,
    /// The flyout's row moved or vanished without the pointer: a resize, a favorite toggled
    /// from the flyout, the row leaving the results.
    Dismissed,
    /// Rows scrolled under the pointer; `target` is the row now under it.
    Scrolled { target: Option<(&'a str, Point)> },
    OpenDue { attempt: u32 },
    AimDue { attempt: u32, point: Option<Point> },
    WarmExpired { attempt: u32 },
}

/// `DialogFlyoutEffect`, less `place`.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Activate(String),
    /// Replaces the one pending timer: `kind` fires `delay` seconds from now with `attempt`.
    Schedule { kind: Timer, attempt: u32, delay: f64 },
    Cancel,
}

pub type Effects = Vec<Effect>;

/// `transitionDialogFlyout`.
pub fn step(state: &mut State, event: Event<'_>) -> Effects {
    let was_timed = timed(state);
    let cancel = move || if was_timed { vec![Effect::Cancel] } else { vec![] };
    let next = attempt(state) + was_timed as u32;
    match event {
        Event::Keyboard => {
            if matches!(state, State::Closed { input: Input::Keyboard, warm: false, .. }) {
                return vec![];
            }
            let effects = cancel();
            *state = State::Closed { attempt: next, input: Input::Keyboard, warm: false };
            effects
        }
        Event::Wheel => {
            if let State::Closed { input, .. } = state {
                *input = Input::Pointer;
            }
            vec![]
        }
        Event::RowMoved { id, active, point, flyout } => moved(state, id, active, point, flyout),
        Event::RowLeft { onto, point, flyout } => match state {
            State::Opening { .. } => {
                let effects = cancel();
                *state = State::Closed { attempt: next, input: Input::Pointer, warm: false };
                effects
            }
            State::Open { .. } | State::Aiming { .. } => match onto {
                // The row the pointer enters decides.
                Onto::Row => vec![],
                Onto::Flyout => settle(state),
                Onto::Outside if inside(point, apex(state), flyout) => settle(state),
                Onto::Outside => cool(state),
            },
            State::Closed { .. } => vec![],
        },
        Event::PointerOutside { point, flyout } => match state {
            State::Open { .. } | State::Aiming { .. } if !inside(point, apex(state), flyout) => cool(state),
            _ => vec![],
        },
        Event::FlyoutEntered => settle(state),
        Event::Dismissed => {
            if matches!(state, State::Closed { warm: false, .. }) {
                return vec![];
            }
            let input = match state {
                State::Closed { input, .. } => *input,
                _ => Input::Pointer,
            };
            let effects = cancel();
            *state = State::Closed { attempt: next, input, warm: false };
            effects
        }
        Event::Scrolled { target } => {
            // Scrolled by the arrow keys: the highlight stays, and nothing opens.
            if matches!(state, State::Closed { input: Input::Keyboard, .. }) {
                return vec![];
            }
            match target {
                None => step(state, Event::Dismissed),
                Some((id, _)) if matches!(state, State::Open { id: open, .. } if open.as_str() == id) => vec![],
                Some((id, point)) => show(state, id, point),
            }
        }
        Event::OpenDue { attempt: due } => {
            let State::Opening { attempt, id, apex } = state else { return vec![] };
            if *attempt != due {
                return vec![];
            }
            *state = State::Open { attempt: *attempt, id: std::mem::take(id), apex: *apex };
            vec![]
        }
        Event::AimDue { attempt: due, point } => {
            let State::Aiming { attempt, apex, target, .. } = state else { return vec![] };
            if *attempt != due {
                return vec![];
            }
            let apex = point.unwrap_or(*apex);
            let target = std::mem::take(target);
            *state = State::Open { attempt: *attempt + 1, id: target.clone(), apex };
            vec![Effect::Activate(target)]
        }
        Event::WarmExpired { attempt: due } => {
            if let State::Closed { attempt, warm, .. } = state
                && *warm
                && *attempt == due
            {
                *warm = false;
            }
            vec![]
        }
    }
}

/// `moved`: the pointer moved on row `id`.
fn moved(state: &mut State, id: &str, active: Option<&str>, point: Point, flyout: Option<Rect>) -> Effects {
    let open = matches!(state, State::Open { .. } | State::Aiming { .. });
    // On its way to the flyout, the pointer crosses other rows without taking the flyout with it.
    if open && flyout_id(state) != Some(id) && inside(point, apex(state), flyout) {
        // Each move restarts the wait: the flyout moves only once the pointer rests on the row.
        if let State::Aiming { attempt, target, .. } = state
            && target.as_str() == id
        {
            return vec![Effect::Schedule { kind: Timer::Aim, attempt: *attempt, delay: AIM_TIMEOUT }];
        }
        let attempt = attempt(state) + 1;
        let (from, apex) = (flyout_id(state).unwrap_or_default().to_owned(), apex(state));
        *state = State::Aiming { attempt, id: from, apex, target: id.to_owned() };
        return vec![Effect::Schedule { kind: Timer::Aim, attempt, delay: AIM_TIMEOUT }];
    }
    // Back on the flyout's own row: the pointer becomes the triangle's apex again.
    if let State::Open { id: on, apex, .. } | State::Opening { id: on, apex, .. } = state
        && on.as_str() == id
        && active == Some(id)
    {
        *apex = point;
        return vec![];
    }
    if let State::Aiming { attempt, id: on, .. } = state
        && on.as_str() == id
    {
        let attempt = *attempt + 1;
        *state = State::Open { attempt, id: id.to_owned(), apex: point };
        return if active == Some(id) { vec![Effect::Cancel] } else { vec![Effect::Cancel, Effect::Activate(id.to_owned())] };
    }
    if warm(state) {
        return show(state, id, point);
    }
    let attempt = attempt(state) + 1;
    let mut effects = if timed(state) { vec![Effect::Cancel] } else { vec![] };
    *state = State::Opening { attempt, id: id.to_owned(), apex: point };
    effects.push(Effect::Activate(id.to_owned()));
    effects.push(Effect::Schedule { kind: Timer::Open, attempt, delay: OPEN_DELAY });
    effects
}

/// `show`: opens the flyout on row `id` at once.
fn show(state: &mut State, id: &str, point: Point) -> Effects {
    let attempt = attempt(state) + 1;
    let mut effects = if timed(state) { vec![Effect::Cancel] } else { vec![] };
    *state = State::Open { attempt, id: id.to_owned(), apex: point };
    effects.push(Effect::Activate(id.to_owned()));
    effects
}

/// `settle`: the pointer reached the flyout or rests in its triangle, so a pending move to
/// another row is called off.
fn settle(state: &mut State) -> Effects {
    let State::Aiming { attempt, id, apex, .. } = state else { return vec![] };
    *state = State::Open { attempt: *attempt + 1, id: std::mem::take(id), apex: *apex };
    vec![Effect::Cancel]
}

/// `cool`: the pointer left the flyout behind; it closes, and the next row's flyout opens at
/// once for a moment.
fn cool(state: &mut State) -> Effects {
    let attempt = attempt(state) + 1;
    *state = State::Closed { attempt, input: Input::Pointer, warm: true };
    vec![Effect::Schedule { kind: Timer::Warm, attempt, delay: WARM }]
}

fn attempt(state: &State) -> u32 {
    match state {
        State::Closed { attempt, .. } | State::Opening { attempt, .. } | State::Open { attempt, .. } | State::Aiming { attempt, .. } => *attempt,
    }
}

fn apex(state: &State) -> Point {
    match state {
        State::Opening { apex, .. } | State::Open { apex, .. } | State::Aiming { apex, .. } => *apex,
        State::Closed { .. } => [0.0, 0.0],
    }
}

fn timed(state: &State) -> bool {
    matches!(state, State::Opening { .. } | State::Aiming { .. } | State::Closed { warm: true, .. })
}

fn warm(state: &State) -> bool {
    matches!(state, State::Open { .. } | State::Aiming { .. } | State::Closed { warm: true, .. })
}

/// `selectFlyoutOpenId`: the row whose flyout shows.
pub fn flyout_id(state: &State) -> Option<&str> {
    match state {
        State::Open { id, .. } | State::Aiming { id, .. } => Some(id),
        _ => None,
    }
}

/// `selectFlyoutFollowsPointer`: whether scrolling moves the highlight and flyout to the row
/// under the pointer; a keyboard scroll keeps both.
pub fn follows_pointer(state: &State) -> bool {
    !matches!(state, State::Closed { input: Input::Keyboard, .. })
}

/// `inside`: whether `p` is in the triangle from `apex` to the flyout's near corners, as on macOS.
pub fn inside(p: Point, apex: Point, flyout: Option<Rect>) -> bool {
    let Some(f) = flyout else { return false };
    let x = if f[0] > apex[0] { f[0] } else { f[0] + f[2] };
    let side = |a: Point, b: Point| (p[0] - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (p[1] - b[1]);
    let t = [apex, [x, f[1]], [x, f[1] + f[3]]];
    let s = [side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0])];
    !(s.iter().any(|v| *v < 0.0) && s.iter().any(|v| *v > 0.0))
}
