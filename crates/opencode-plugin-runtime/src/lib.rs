//! Reusable, `App`-independent Luau plugin runtime primitives for Rune hosts (`opencode-desktop`,
//! `opencode-web`, and other Rune apps).
//!
//! Contains the pure state machines, resource pools, background workers, hot-reload compiler,
//! supervised child-process manager, display-sleep blocker, QR encoder, and Luau service helpers
//! that have zero dependency on any specific application shell.

pub mod askpass;
pub mod boot;
pub mod catalogs;
pub mod console;
pub mod contracts;
pub mod contribs;
pub mod dormant;
pub mod flyout;
pub mod hot;
pub mod i18n;
pub mod icons;
pub mod images;
pub mod luau;
pub mod machines;
pub mod native_icons;
pub mod power;
pub mod process;
pub mod qr;
pub mod redraw;
pub mod reveal;
pub mod server_sources;
pub mod sources;
pub mod tasks;
pub mod theme;
pub mod ui_nodes;

pub use contribs::Kind;
