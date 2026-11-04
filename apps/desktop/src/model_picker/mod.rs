//! The model list's own parts beyond the shared popover live in `session_view::pickers`; this
//! module keeps their story and tests.

pub use session_view::pickers::{hover, rail};

mod story;

#[cfg(test)]
mod tests;
