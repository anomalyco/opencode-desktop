//! When the lit model's flyout opens beside its row, as in Desktop's model dialog
//! (`select-dialog.tsx:723-1068`): [`machine`] is Desktop's `select-dialog-machine.ts`, and this
//! module feeds it the pointer, the wheel, the keys and its timers. Every model has a flyout: its
//! efforts, or Default alone.
//!
//! - The first flyout waits 100 ms of the pointer on a row; while one is open, and for 300 ms
//!   after the pointer leaves it, the next opens at once.
//! - A safe triangle, from the pointer's last point on the flyout's row to the flyout's near
//!   corners, keeps the flyout while the pointer crosses other rows on its way in. Resting on a
//!   row inside it for 300 ms hands over to that row, as on macOS.
//! - A wheel scroll lights the row it brings under the pointer and moves the flyout beside it. The
//!   arrow keys and typing close the flyout, and rows they scroll under a still pointer leave the
//!   highlight where the keys put it.
//! - A new query starts the list from the top; results that land later for the same query bring
//!   the lit row back into view unless it's the first (`search-list.tsx:189-207`).

pub mod machine;

use std::hash::{Hash, Hasher};

use rune::machine::Attempt;
use rune::{El, PointerEvent, Tx};

use super::{Kind, PickerHost};
use machine::{Effect, Event, Onto, Point, Rect, State, Timer};

/// The machine's one timer (Desktop's `timer`): `Schedule` replaces it and `Cancel` clears it.
/// It's due at `at` on the window's clock (`Tx::clock`), and the poll fires it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Wait {
    #[default]
    Idle,
    Due { kind: Timer, attempt: Attempt, at: f64 },
}

/// The pointer (Desktop's `pointer`): `Moved` in this frame's input, else `Still`. A row coming
/// under a still pointer is the list scrolling (Desktop's `scroll` listener and `follow`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Pointer {
    #[default]
    Unknown,
    Moved(Point),
    Still(Point),
}

impl Pointer {
    fn point(self) -> Option<Point> {
        match self {
            Pointer::Unknown => None,
            Pointer::Moved(p) | Pointer::Still(p) => Some(p),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Hover {
    state: State,
    wait: Wait,
    pointer: Pointer,
    /// The pointer's last point on a row: a row's move at the same point is rows moving under a
    /// still pointer, which Desktop's row skips (`movementX === 0 && movementY === 0`).
    row_at: Option<Point>,
    /// The arrow keys have moved through the results since the query last changed.
    pub navigated: bool,
    /// The query and the rows the list last revealed its highlight for.
    revealed: Option<(String, u64)>,
    /// Where the list and the flyout draw, in window points ([`El::on_drawn_layout_tx`]): the
    /// flyout flips beside its row where it doesn't fit, which layout doesn't know.
    list: Option<Rect>,
    flyout: Option<Rect>,
    /// The flyout's option under the pointer, by the model whose flyout it is.
    lit: Option<(String, String)>,
}

impl Hover {
    /// As the list opens: the highlight is the keys' until the pointer moves.
    pub fn fresh() -> Hover {
        Hover::default()
    }

    /// The model whose flyout shows (`selectFlyoutOpenId`).
    pub fn flyout_model(&self) -> Option<&str> {
        machine::flyout_id(&self.state)
    }

    /// The flyout's option under the pointer.
    pub fn lit(&self) -> Option<&str> {
        self.lit.as_ref().filter(|(model, _)| self.flyout_model() == Some(model)).map(|(_, option)| option.as_str())
    }

    fn timer_due(&mut self, now: f64, tx: &mut Tx) -> Option<(Timer, Attempt)> {
        let Wait::Due { kind, attempt, at } = self.wait else { return None };
        if now + 1e-6 < at {
            tx.wake_at(at);
            return None;
        }
        self.wait = Wait::Idle;
        Some((kind, attempt))
    }
}

/// Whether `p` is inside the triangle from `apex` to the flyout's corners nearest it.
pub fn in_triangle(apex: Point, flyout: Rect, p: Point) -> bool {
    machine::inside(p, apex, Some(flyout))
}

/// What the pointer at `at` is over, as Desktop's `relatedTarget` and `event.target` say: the
/// flyout, a model row (`[data-slot="search-list-row"]`) rather than a header or the list's
/// padding, or anything else. `then` hears it after the handler.
fn onto<S: PickerHost>(s: &S, at: Point, tx: &mut Tx, then: impl FnOnce(&mut S, Onto, &mut Tx) + 'static) {
    if s.pickers().hover.flyout.is_some_and(|f| at[0] >= f[0] && at[0] <= f[0] + f[2] && at[1] >= f[1] && at[1] <= f[1] + f[3]) {
        return tx.element_at_in("effort-flyout", at[0], at[1], move |s: &mut S, _, tx: &mut Tx| then(s, Onto::Flyout, tx));
    }
    tx.element_at_in("picker-scroll", at[0], at[1], move |s: &mut S, picked, tx: &mut Tx| {
        let row = picked.is_some_and(|p| p.path().split('/').any(|seg| seg.starts_with("option-")));
        then(s, if row { Onto::Row } else { Onto::Outside }, tx)
    });
}

/// The model list's pointer, wheel and keys, for any [`PickerHost`].
pub trait ModelHover: PickerHost + Sized + 'static {
    /// Runs `event` through the flyout's machine and its effects (`runFlyoutEffect`).
    fn flyout_event(&mut self, event: Event<'_>, tx: &mut Tx) {
        let before = self.pickers().hover.flyout_model().map(str::to_owned);
        let effects = machine::step(&mut self.pickers_mut().hover.state, event);
        for effect in effects {
            match effect {
                Effect::Activate(id) => {
                    tx.instant();
                    crate::popover::jump_highlight();
                    self.pickers_mut().activate(id);
                }
                Effect::Schedule { kind, attempt, delay } => {
                    let at = tx.clock() + delay;
                    self.pickers_mut().hover.wait = Wait::Due { kind, attempt, at };
                    tx.wake_at(at);
                }
                Effect::Cancel => self.pickers_mut().hover.wait = Wait::Idle,
            }
        }
        if self.pickers().hover.flyout_model() != before.as_deref() {
            tx.instant();
        }
    }

    /// The pointer moved on the model row `key` (Desktop's row `onMouseMove`).
    fn model_row_moved(&mut self, key: &str, at: Point, tx: &mut Tx) {
        let h = &mut self.pickers_mut().hover;
        if h.row_at == Some(at) {
            return;
        }
        h.row_at = Some(at);
        let flyout = h.flyout;
        let active = self.picker_active();
        self.flyout_event(Event::RowMoved { id: key, active: active.as_deref(), point: at, flyout }, tx);
    }

    /// The pointer came onto (`inside`) or left the model row `key`. Onto it under a still
    /// pointer, the list scrolled it there (Desktop's `follow`); off it, the row the pointer
    /// enters, the flyout or anything else decides (Desktop's row `onMouseLeave`).
    fn model_row_hovered(&mut self, key: &str, inside: bool, tx: &mut Tx) {
        let pointer = self.pickers().hover.pointer;
        if inside {
            tx.instant();
            crate::popover::jump_highlight();
            if let Pointer::Still(point) = pointer
                && machine::follows_pointer(&self.pickers().hover.state)
            {
                self.flyout_event(Event::Scrolled { target: Some((key, point)) }, tx);
            }
            return;
        }
        let Some(point) = pointer.point() else { return };
        match &self.pickers().hover.state {
            // Rune hears a row's move before the last row's leave, so an opening that has moved
            // on to the next row is that row's.
            State::Opening { id, .. } if id == key => {
                let flyout = self.pickers().hover.flyout;
                self.flyout_event(Event::RowLeft { onto: Onto::Outside, point, flyout }, tx);
            }
            State::Open { .. } | State::Aiming { .. } => onto(self, point, tx, move |s: &mut Self, onto, tx: &mut Tx| {
                let flyout = s.pickers().hover.flyout;
                s.flyout_event(Event::RowLeft { onto, point, flyout }, tx);
            }),
            _ => {}
        }
    }

    /// A wheel scroll over the list: the rows it brings under the pointer light.
    fn model_list_wheeled(&mut self, tx: &mut Tx) {
        self.flyout_event(Event::Wheel, tx);
    }

    /// The arrow keys moved the highlight, the query changed, a favorite was toggled or an
    /// effort staged from the keyboard: the flyout closes, and the next waits again.
    fn model_list_keyed(&mut self) {
        self.model_keyed(&mut Tx::default());
    }

    /// [`ModelHover::model_list_keyed`] in a handler that commits what it changes.
    fn model_keyed(&mut self, tx: &mut Tx) {
        self.flyout_event(Event::Keyboard, tx);
    }

    /// The flyout's row moved or went without the pointer: it closes, and nothing opens at once.
    fn model_flyout_dismissed(&mut self, tx: &mut Tx) {
        self.flyout_event(Event::Dismissed, tx);
    }

    /// The pointer anywhere in the window (Desktop's document `pointermove`): off the rows and
    /// the flyout, leaving the triangle closes the flyout.
    fn model_pointer_moved(&mut self, at: Point, tx: &mut Tx) {
        let h = &mut self.pickers_mut().hover;
        if h.pointer.point() != Some(at) {
            h.pointer = Pointer::Moved(at);
        }
        if h.flyout_model().is_none() {
            return;
        }
        onto(self, at, tx, move |s: &mut Self, onto, tx: &mut Tx| {
            if onto == Onto::Outside {
                let flyout = s.pickers().hover.flyout;
                s.flyout_event(Event::PointerOutside { point: at, flyout }, tx);
            }
        });
    }

    /// Opens the model `key`'s flyout beside its row at once, or closes it when it's open (the
    /// storybook's O; Desktop's flyout opens only on hover).
    fn pin_effort_flyout(&mut self, key: &str, tx: &mut Tx) {
        if self.pickers().hover.flyout_model() == Some(key) {
            return self.flyout_event(Event::Dismissed, tx);
        }
        let point = self.pickers().hover.pointer.point().unwrap_or_default();
        self.flyout_event(Event::Scrolled { target: Some((key, point)) }, tx);
    }
}

impl<S: PickerHost + 'static> ModelHover for S {}

/// Fires the flyout's due timer, follows the query and the results, and closes a flyout whose
/// row left the results or moved with the window.
pub fn poll<S: PickerHost + 'static>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    if s.pickers().open != Some(Kind::Model) {
        return false;
    }
    // What the pointer did, it did in the input before this frame.
    let h = &mut s.pickers_mut().hover;
    if let Pointer::Moved(p) = h.pointer {
        h.pointer = Pointer::Still(p);
    }
    let query = s.pickers().search.text.clone();
    let items = s.picker_items();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    items.iter().for_each(|i| i.key.hash(&mut hasher));
    let rows = hasher.finish();
    let revealed = s.pickers().hover.revealed.clone();
    let fresh = revealed.as_ref().is_none_or(|(q, _)| *q != query);
    let changed = revealed.as_ref().is_some_and(|(q, r)| *q == query && *r != rows);
    if fresh {
        // A new query starts from its first row at the top, the flyout shut (`search-list.tsx:132-140`,
        // `select-dialog.tsx:689-692`).
        if revealed.is_some() {
            s.model_keyed(tx);
        }
        s.pickers_mut().hover.navigated = false;
        tx.scroll_to_y("picker-scroll", 0.0);
    } else if changed {
        // Results that land later for the same query bring the lit row back into view, unless
        // it's the first, which shows from the top.
        let active = s.picker_active();
        match active.filter(|a| items.first().is_some_and(|i| i.key != *a)) {
            Some(key) => tx.scroll_into_view_in("picker-scroll", format!("option-{key}")),
            None => tx.scroll_to_y("picker-scroll", 0.0),
        }
    }
    s.pickers_mut().hover.revealed = Some((query, rows));
    // The flyout's row left the results, such as when the models change.
    if let Some(model) = s.pickers().hover.flyout_model()
        && !items.iter().any(|i| i.key == model)
    {
        s.model_flyout_dismissed(tx);
    }
    let Some((kind, attempt)) = s.pickers_mut().hover.timer_due(now, tx) else { return fresh || changed };
    let point = s.pickers().hover.pointer.point();
    let event = match kind {
        Timer::Open => Event::OpenDue { attempt },
        Timer::Aim => Event::AimDue { attempt, point },
        Timer::Warm => Event::WarmExpired { attempt },
    };
    s.flyout_event(event, tx);
    true
}

/// The model list's row `key`: the pointer moving on it lights it and opens its efforts.
pub fn row<S: PickerHost + 'static>(e: El, key: &str) -> El {
    let (moved, hovered) = (key.to_owned(), key.to_owned());
    e.on_pointer_move_local(move |s: &mut S, p: PointerEvent, tx: &mut Tx| s.model_row_moved(&moved, [p.window_x, p.window_y], tx))
        .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| s.model_row_hovered(&hovered, inside, tx))
}

/// The model list's scroll: where it draws, and a wheel scroll lighting the rows it brings under
/// the pointer. The list moving (Desktop's window `resize`) closes the flyout, which was placed
/// beside its row.
pub fn scroll<S: PickerHost + 'static>(e: El) -> El {
    e.on_drawn_layout_tx(|s: &mut S, r, tx: &mut Tx| {
        let was = s.pickers_mut().hover.list.replace(r);
        if was.is_some_and(|w| w != r) {
            s.model_flyout_dismissed(tx);
        }
    })
    .on_wheel(|s: &mut S, _, tx: &mut Tx| {
        s.model_list_wheeled(tx);
        false
    })
}

/// The flyout: where it draws, and the pointer reaching it.
pub fn flyout<S: PickerHost + 'static>(e: El) -> El {
    e.on_drawn_layout_tx(|s: &mut S, r, tx: &mut Tx| {
        s.pickers_mut().hover.flyout = Some(r);
        tx.live_only();
    })
    .on_hover_change(|s: &mut S, inside, tx: &mut Tx| {
        if inside {
            s.flyout_event(Event::FlyoutEntered, tx);
        }
    })
}

/// One of the flyout's options: the pointer on it lights it.
pub fn option_hovered<S: PickerHost>(s: &mut S, option: &str, inside: bool) {
    let h = &mut s.pickers_mut().hover;
    let Some(model) = h.flyout_model().map(str::to_owned) else { return };
    if inside {
        h.lit = Some((model, option.to_owned()));
    } else if h.lit() == Some(option) {
        h.lit = None;
    }
}

/// The layer under the open list: the pointer leaving the rows, the flyout and the triangle
/// closes the flyout.
pub fn watch<S: PickerHost + 'static>(e: El) -> El {
    e.on_pointer_move_local(|s: &mut S, p: PointerEvent, tx: &mut Tx| s.model_pointer_moved([p.window_x, p.window_y], tx))
}
