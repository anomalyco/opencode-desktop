//! Pure-Rust Rune client for the on-demand `OpenCode Browser Engine` CEF sidecar.
//!
//! Keeps `OpenCode (Rune).app` small (~8 MB) and free of C++/CEF dependencies:
//! - Resolves or downloads the signed `OpenCode Browser Engine.app` into Application Support on first use.
//! - Speaks line-delimited JSON over a private Unix domain socket for tabs, input, resize, and CDP.
//! - Receives zero-copy 2x Retina `IOSurface` frames over a Mach port pipe on macOS into Rune's `NativeSurface`.

pub mod address;
mod client;
pub mod installer;
#[cfg(target_os = "macos")]
pub mod mach_pipe;
pub mod protocol;
#[cfg(test)]
mod tests;
mod transport;

pub use address::resolve_address;
pub use client::BrowserHost;
pub use client::BrowserTab;
pub use installer::{
    ENGINE_DEFAULT_SHA256, ENGINE_DEFAULT_URL, ENGINE_DOWNLOAD_MB, ENGINE_VERSION, EngineStatus,
};
pub use protocol::CursorKind;
pub use protocol::PickedElement;
