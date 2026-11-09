//! Whether a session's timeline follows its end, as Desktop's follow machine
//! (`app/src/session/timeline/machine.ts:22-30`, `FollowState`) and its virtualizer
//! (`virtualizer.tsx:379-385`: `anchorTo` "end" while following, else "start").
//!
//! - `Following`: the newest content holds at the bottom of the view; Rune's `stick_bottom`
//!   keeps it there.
//! - `Detached`: the reader is scrolled up; the rows above what shows keep their place.
//! - `Anchored`: a pressed disclosure asked the timeline to hold one element where it shows
//!   across its commit (`Tx::keep_on_screen`), as Desktop's `anchored` holds a revealed tool call.
//!
//! The viewport reports whether it shows its end ([`Event::AtEnd`], `El::on_at_bottom`), and a
//! disclosure opening or shutting is [`Event::Disclosure`]; [`step`] returns the hold to make.
//! Desktop's gesture and return-attempt states have no Rune counterpart: Rune's scroll owns
//! wheel, touch and its glide to the end, and reports only where it lands.

/// Where a session's timeline holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Follow {
    #[default]
    Following,
    Detached,
    /// `key`: the element held where it shows.
    Anchored { key: String },
}

#[derive(Clone, Copy, Debug)]
pub enum Event<'a> {
    /// The viewport shows its end, or stopped showing it.
    AtEnd(bool),
    /// A disclosure opens (`opening`) or shuts: `row`, the opened row, and `header`, the
    /// pressed header.
    Disclosure { opening: bool, row: &'a str, header: &'a str },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Hold this element where it shows across the commit (`Tx::keep_on_screen`).
    Keep(String),
}

impl Follow {
    pub fn following(&self) -> bool {
        matches!(self, Follow::Following)
    }
}

/// One transition.
///
/// - Arriving at the end follows it again, whatever held (`machine.ts:212-224`, `scrolled`:
///   arriving at the end resumes following); leaving it detaches.
/// - A disclosure while following holds nothing: the end holds, so what opens grows up past the
///   pressed row and what shuts lets the rows above come down (`anchorTo: "end"`;
///   `virtualizer.tsx:467-471` scrolls a following view to its end after each resize).
/// - A disclosure while detached or anchored holds what Desktop's `anchorTo: "start"` keeps
///   still: opening, the opened row keeps its top, as only rows wholly above the first one
///   shown are compensated (`virtualizer.tsx:476-492`); shutting, the pressed header, stuck to
///   the top or not, as the group scrolls up by its stuck offset before it collapses
///   (`tool-renderer.tsx:586-595`).
pub fn step(state: Follow, event: Event) -> (Follow, Option<Effect>) {
    match (state, event) {
        (_, Event::AtEnd(true)) => (Follow::Following, None),
        (Follow::Following, Event::AtEnd(false)) => (Follow::Detached, None),
        (state, Event::AtEnd(false)) => (state, None),
        (Follow::Following, Event::Disclosure { .. }) => (Follow::Following, None),
        (_, Event::Disclosure { opening, row, header }) => {
            let key = if opening { row } else { header }.to_owned();
            (Follow::Anchored { key: key.clone() }, Some(Effect::Keep(key)))
        }
    }
}
