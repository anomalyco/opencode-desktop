//! Headless runtime primitives and explicit state machines shared across OpenCode Rune hosts.
//!
//! Kept strictly free of UI widgets, `rune::El`/`Cx`/`Color`, `session-view`, and storage
//! backends so desktop, web (`wasm32`), and embedded hosts can share the exact same domain
//! transitions and multi-server runtime structures.

#![forbid(unsafe_code)]

pub mod catalog;
pub mod draft;
pub mod idle;
pub mod machines;
pub mod pairing;
pub mod server_machine;
pub mod server_pool;
pub mod types;
pub mod window;

pub use catalog::Catalog;
pub use draft::{Entry, SavedDraft};
pub use server_pool::ServerPool;
pub use types::{
    ContextFile, DockPlacement, NotifyKind, NotifyKinds, SendResult, TabStatus, TabSummary,
    ZOOM_LEVELS, ZOOM_MAX, ZOOM_MIN, step_zoom,
};
