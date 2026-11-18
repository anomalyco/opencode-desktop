//! Reusable, `App`-independent Luau plugin runtime primitives for Rune hosts (`opencode-desktop`,
//! `opencode-web`, Seer, and future Rune apps).
//!
//! Contains the pure state machines, resource pools, background workers, hot-reload compiler,
//! supervised child-process manager, display-sleep blocker, QR encoder, and Luau service helpers
//! that have zero dependency on any specific application shell.

pub mod catalogs;
pub mod console;
pub mod flyout;
pub mod hot;
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
pub mod sources;
pub mod tasks;
