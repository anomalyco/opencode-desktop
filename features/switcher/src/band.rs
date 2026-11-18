//! Which of the palette's rows are built: those within a row or two of the scroll view, and
//! the highlighted one wherever it is (A11Y's contract, `notes\a11y-status.md`). The rest are
//! spacers of their exact height, so the list scrolls as it did and only ~17 rows are built
//! for a new query instead of ~50.
//!
//! The scroll view reports where it is from layout (`on_scroll_offset`), which can't change
//! state: the band keeps it in a cell, and when the view leaves the rows built it wakes the
//! window, whose next poll (`moved`) runs the view again.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use rune::{ScrollPos, Waker};

/// Rows built past each edge of the viewport, in points.
const MARGIN: f32 = 2.0 * 32.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    /// Where the list was last seen; nothing built against it yet.
    Seen { offset: f32, height: f32 },
    /// The view built the rows covering `built` for this offset.
    Built { offset: f32, height: f32, built: (f32, f32) },
    /// The list scrolled out of what was built: the view must run again.
    Moved { offset: f32, height: f32 },
}

impl Default for State {
    fn default() -> Self {
        State::Seen { offset: 0.0, height: 0.0 }
    }
}

enum Event {
    Scrolled { offset: f32, height: f32 },
    /// The view built rows covering this range.
    Built((f32, f32)),
    /// The poll took a move.
    Taken,
}

impl State {
    fn at(self) -> (f32, f32) {
        match self {
            State::Seen { offset, height } | State::Built { offset, height, .. } | State::Moved { offset, height } => (offset, height),
        }
    }

    /// True when the window must run its view again.
    fn step(&mut self, event: Event) -> bool {
        match (*self, event) {
            (State::Built { built, .. }, Event::Scrolled { offset, height }) => {
                let inside = offset >= built.0 && offset + height <= built.1;
                *self = if inside { State::Built { offset, height, built } } else { State::Moved { offset, height } };
                !inside
            }
            (State::Moved { .. }, Event::Scrolled { offset, height }) => {
                *self = State::Moved { offset, height };
                false
            }
            (State::Seen { .. }, Event::Scrolled { offset, height }) => {
                *self = State::Seen { offset, height };
                false
            }
            (state, Event::Built(built)) => {
                let (offset, height) = state.at();
                *self = State::Built { offset, height, built };
                false
            }
            (State::Moved { offset, height }, Event::Taken) => {
                *self = State::Seen { offset, height };
                true
            }
            (_, Event::Taken) => false,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Band(Rc<Cell<State>>);

impl Band {
    /// The span of the list, in its own points, whose rows the view builds now.
    pub fn window(&self, fallback_height: f32) -> Range<f32> {
        // The viewport fits its results. On a landing it can grow from one row to the full
        // dialog in this commit; its last measured height belongs to the old answer. Build
        // through the new viewport's limit rather than waking for a second band next frame.
        let (offset, _) = self.0.get().at();
        let height = fallback_height;
        (offset - MARGIN).max(0.0)..offset + height + MARGIN
    }

    pub fn built(&self, window: &Range<f32>) {
        let mut s = self.0.get();
        s.step(Event::Built((window.start, window.end)));
        self.0.set(s);
    }

    /// The scroll view's report: wakes the window when it left the rows built.
    pub fn watch(&self, waker: Option<Waker>) -> impl Fn(ScrollPos) + 'static {
        let cell = self.0.clone();
        move |at: ScrollPos| {
            let mut s = cell.get();
            let wake = s.step(Event::Scrolled { offset: at.offset[1], height: at.viewport[1] });
            cell.set(s);
            if wake && let Some(w) = &waker {
                w();
            }
        }
    }

    /// For the poll: true once when the list scrolled out of the rows built.
    pub fn moved(&self) -> bool {
        let mut s = self.0.get();
        let moved = s.step(Event::Taken);
        self.0.set(s);
        moved
    }
}
