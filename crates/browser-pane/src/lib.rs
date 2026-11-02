//! Pure-Rust Rune client for the on-demand `OpenCode Browser Engine` CEF sidecar.
//!
//! Keeps `OpenCode (Rune).app` small (~8 MB) and free of C++/CEF dependencies:
//! - Resolves or downloads the signed `OpenCode Browser Engine.app` into Application Support on first use.
//! - Speaks line-delimited JSON over a private Unix domain socket for tabs, input, resize, and CDP.
//! - Receives zero-copy 2x Retina `IOSurface` frames over a Mach port pipe on macOS into Rune's `NativeSurface`.

pub mod address;
mod client;
mod github_download;
mod site_zoom;
mod surface;
pub mod installer;
#[cfg(target_os = "macos")]
pub mod mach_pipe;
pub mod profile;
pub mod protocol;
#[cfg(test)]
mod tests;
mod transport;

pub use address::{bare_url, host_of, resolve_address};
pub use client::BrowserHost;
pub use client::BrowserTab;
pub use installer::{
    ENGINE_DEFAULT_SHA256, ENGINE_DEFAULT_URL, ENGINE_DOWNLOAD_MB, ENGINE_VERSION, EngineStatus,
};
pub use github_download::{ERR_DOWNLOAD, ERR_SIGN_IN, err_no_access};
pub use profile::shared_profile_dir;
pub use protocol::CursorKind;
pub use protocol::PickedElement;
