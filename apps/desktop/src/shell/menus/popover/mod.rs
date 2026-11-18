//! The one popover every menu and picker is built from lives in `session_view::popover`, shared
//! with Seer; this module adds its story and its tests.

mod story;

pub use session_view::popover::*;

rune_register::register!(crate::knobs::GROUP_LIST: rune_knobs::Group = [motion::GROUP]);

