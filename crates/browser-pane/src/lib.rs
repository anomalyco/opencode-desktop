//! Pure-Rust Rune client for the on-demand `OpenCode Browser Engine` CEF sidecar.
//!
//! Keeps `OpenCode (Rune).app` small (~8 MB) and free of C++/CEF dependencies:
//! - Resolves or downloads the signed `OpenCode Browser Engine.app` into Application Support on first use.
//! - Speaks line-delimited JSON over a private Unix domain socket for tabs, input, resize, and CDP.
//! - Receives zero-copy 2x Retina `IOSurface` frames over a Mach port pipe on macOS into Rune's `NativeSurface`.
//!
//! The sidecar needs the `engine` feature and a Unix host. Without either, `BrowserHost` keeps its
//! tab model and mock tabs, and reports `EngineStatus::Unsupported` instead of starting an engine.

pub mod address;
mod client;
#[cfg(all(feature = "engine", unix))]
mod github_download;
pub mod installer;
mod keys;
#[cfg(all(feature = "engine", target_os = "macos"))]
pub mod mach_pipe;
#[cfg(all(feature = "engine", unix))]
pub mod profile;
pub mod protocol;
#[cfg(all(feature = "engine", unix))]
mod sidecar;
mod site_zoom;
mod surface;
#[cfg(test)]
mod tests;
#[cfg(all(feature = "engine", unix))]
mod transport;

pub use address::{bare_url, host_of, resolve_address};
pub use client::BrowserHost;
pub use client::BrowserTab;
pub use installer::{
    ENGINE_DEFAULT_SHA256, ENGINE_DEFAULT_URL, ENGINE_DOWNLOAD_MB, ENGINE_VERSION, ERR_DOWNLOAD, ERR_SIGN_IN,
    EngineStatus, err_no_access,
};
#[cfg(all(feature = "engine", unix))]
pub use profile::shared_profile_dir;
pub use protocol::CursorKind;
pub use protocol::PickedElement;
