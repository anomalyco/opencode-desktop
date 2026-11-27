//! The one keyboard focus ring (anomalyco/opencode#64909, `packages/ui/src/styles/base.css`
//! `:focus-visible`): 2 points of `border-focus` (`blue-500` light, `blue-400` dark, the theme's
//! own colour elsewhere), keyboard focus only, following the control's corners.
//!
//! - **Outside (`focus_default`):** 2 points out, for a control with room around it. It is the
//!   view's default ring too (`Cx::focus_ring` in the app's root view), so most controls set
//!   nothing.
//! - **Inside (`focus_inset`):** 2 points in (`outline-offset: -2px`), for full-width rows, tabs,
//!   scroll viewports and controls flush with an edge that clips, where an outside ring would be
//!   cut off or cover a neighbour.
//!
//! A selection is never drawn in `border-focus`: that colour means keyboard focus.

use rune::{Color, El};

use crate::palette::Pal;

/// The ring's width in points.
pub const WIDTH: f32 = 2.0;
/// The gap between a control and its ring, in points.
pub const OFFSET: f32 = 2.0;
/// The ring drawn inside the control's edge, in points.
pub const INSET: f32 = -2.0;

/// The ring's colour in `p`'s theme: `border-border-focus`.
pub fn color(p: &Pal) -> Color {
    p.v2.border_focus
}

/// One half of a split button: it rounds its own outer corners and keeps the ring inside its
/// edge, so the ring follows them where the group's clip would cut an outside ring
/// (`split-button.css`, opencode#65783). `trailing` is the menu trigger's half.
pub fn split_half(half: El, p: &Pal, radius: f32, trailing: bool) -> El {
    let rtl = opencode_i18n::direction() == opencode_i18n::Direction::Rtl;
    let corners = if trailing == rtl { [radius, 0.0, 0.0, radius] } else { [0.0, radius, radius, 0.0] };
    half.corners(corners).focus_inset(p)
}

/// The shared ring on an element, as one call in its builder chain.
pub trait FocusExt {
    /// The ring 2 points outside the element.
    fn focus_default(self, p: &Pal) -> Self;
    /// The ring 2 points inside the element's edge.
    fn focus_inset(self, p: &Pal) -> Self;
}

impl FocusExt for El {
    fn focus_default(self, p: &Pal) -> Self {
        self.focus_ring(WIDTH, OFFSET, color(p))
    }

    fn focus_inset(self, p: &Pal) -> Self {
        self.focus_ring(WIDTH, INSET, color(p))
    }
}
