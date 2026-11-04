pub use session_view::scrollbar::{Bars, FADE_DELAY, GROUP, HOT_WIDTH, WIDTH};
#[cfg(test)]
use session_view::scrollbar::{INSET, MIN_LEN, thumb};

#[cfg(test)]
mod tests;
pub mod stories;

rune_register::register!(crate::knobs::GROUP_LIST: rune_knobs::Group = [GROUP]);

pub fn bar(s: &crate::App, p: &crate::palette::Pal, key: &str, bottom: f32) -> rune::El {
    session_view::scrollbar::bar(s, p, key, bottom)
}
