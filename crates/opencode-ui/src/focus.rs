//! The one focus ring (anomalyco/opencode#64909, `packages/ui/src/styles/base.css` `:focus-visible`):
//! 2 pt of `border-focus` 2 pt outside the control, keyboard focus only. Full-width rows, tabs and
//! anything a scrolling or clipping container would cut draw it 2 pt inside instead
//! (`outline-offset: -2px`).

use rune::{Color, El};

use crate::palette::{Pal, kit};

/// The ring's width.
pub const WIDTH: f32 = 2.0;
/// How far outside the control's edge the ring sits.
pub const OFFSET: f32 = 2.0;
/// How far inside a clipped row's edge the ring sits.
pub const INSET: f32 = -2.0;

/// `border-focus` in `p`'s theme.
pub fn color(p: &Pal) -> Color {
    kit(p).c.border_focus
}

/// The ring 2 pt outside `e`.
pub fn ring(p: &Pal, e: El) -> El {
    e.focus_ring(WIDTH, OFFSET, color(p))
}

/// The ring 2 pt inside `e`, for rows and tabs in a list that clips.
pub fn inset(p: &Pal, e: El) -> El {
    e.focus_ring(WIDTH, INSET, color(p))
}
