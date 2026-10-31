//! Parts agents compose into notes and chat replies: one catalog, one outline grammar, one checker.
//!
//! A ```` ```ui ```` fence holds an outline, a part name first on every line and kids indented two
//! spaces under it. Seer cards and OpenCode (Rune) sessions draw the same outline with their own
//! theme ([`view`], behind the `view` feature), and the checker gives agents errors they can act on.
//!
//! ```text
//! row
//!   box Today
//!     item {risk} Voice reply · pasted into Kit's bubble
//!   arrow
//!   box After
//!     say Kit {voice}: filter Seer to Hex
//! ```
//!
//! Without `view` the crate is plain Rust, so the `seer` CLI, Workers and Wasm link it too.

mod catalog;
mod check;
mod html;
mod note;
mod outline;
#[cfg(feature = "view")]
pub mod view;

pub use catalog::{CATALOG, Kids, PartSpec, Tone, docs, spec};
pub use check::{BudgetProfile, Issue, check, check_outline};
pub use html::to_html;
pub use note::{Block, parse_note};
pub use outline::{Node, Part, parse_outline, to_plain};

#[cfg(test)]
mod tests;
