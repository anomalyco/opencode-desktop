//! Compatibility re-export of [`opencode_ui::popover`] with a [`TimelineHost`]-bound [`scrim`].

pub use opencode_ui::popover::*;

/// A window-sized layer under the popover (mount it just before): a press outside closes it
/// (`close`), in the same frame unless the popover has an exit to play.
pub fn scrim<S: crate::TimelineHost>(key: impl Into<String>, close: impl Fn(&mut S, &mut rune::Tx) + 'static) -> rune::El {
    opencode_ui::popover::scrim_with(key, |s: &S| s.knobs(), close)
}
