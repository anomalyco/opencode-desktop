//! Native Ghostty terminal core (`libghostty-vt` `v1.4.0`) and off-main-thread PTY session
//! worker for OpenCode (Rune).

mod connection;
pub mod core;
pub mod ffi;
mod ffi_fns;
#[cfg(target_arch = "wasm32")]
mod ffi_web;
mod input;
pub mod persistent_pty;
pub mod pty;
mod render_extract;
pub mod theme;
pub mod types;

pub use core::VtCore;
pub use ffi::GhosttyKey;
pub use pty::{
    MAX_PERSIST_SCROLLBACK, PersistedTerminal, RestoreState, SessionPersistHandle, SessionStatus,
    StreamHandle, TerminalConfig, TerminalSession,
};
pub use theme::{Rgb, TerminalTheme};
pub use types::{
    BgSpan, CellFlags, CursorShape, CursorSnapshot, GridPoint, KeyAction, KeyInput, MouseBtn,
    MouseInput, MouseKind, RenderedCell, ScrollOp, ScrollbarSnapshot, SelectionOp, TerminalRow,
    TerminalSnapshot, UnderlineKind, VtEvent,
};

