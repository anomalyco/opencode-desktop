//! The one popover every menu and picker is built from lives in `opencode_ui::popover`
//! (re-exported through `session_view::popover` for Seer); this module adds its story.

mod story;

pub use session_view::popover::*;

rune_register::register!(crate::knobs::GROUP_LIST: rune_knobs::Group = [motion::GROUP]);

