//! Native Ghostty terminal core (`libghostty-vt` `v1.4.0`) and off-main-thread PTY session
//! worker for OpenCode (Rune).

pub mod core;
pub mod ffi;
mod ffi_fns;
mod input;
pub mod pty;
mod render_extract;
pub mod server_pty;
pub mod theme;
pub mod types;

pub use core::VtCore;
pub use ffi::GhosttyKey;
pub use pty::{
    PersistedTerminal, RestoreState, SessionStatus, StreamHandle, TerminalConfig, TerminalSession,
};
pub use theme::{Rgb, TerminalTheme};
pub use types::{
    BgSpan, CellFlags, CursorShape, CursorSnapshot, GridPoint, KeyAction, KeyInput, MouseBtn,
    MouseInput, MouseKind, RenderedCell, ScrollOp, ScrollbarSnapshot, SelectionOp, TerminalRow,
    TerminalSnapshot, UnderlineKind, VtEvent,
};

#[cfg(test)]
mod abi_tests;
#[cfg(test)]
mod pty_tests;
#[cfg(test)]
mod vt_tests;
