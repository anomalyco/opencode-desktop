//! Pure-Rust Rune client for the on-demand `OpenCode Browser Engine` CEF sidecar.
//!
//! Keeps `OpenCode (Rune).app` small (~8 MB) and free of C++/CEF dependencies:
//! - Finds the installed engine, or fetches it from the build's source (`source.rs`) and checks it against
//!   the build's trust rule before anything of it runs: in the background when the app asks
//!   (`BrowserHost::prefetch`, resumable and throttled), on first use, or as an update beside a running one.
//! - Speaks line-delimited JSON over a private Unix domain socket (AF_UNIX on Windows too) for tabs,
//!   input, resize, and CDP.
//! - Receives zero-copy 2x Retina `IOSurface` frames over a Mach port pipe on macOS, and named D3D
//!   shared textures on Windows, into Rune's `NativeSurface`.
//!
//! The sidecar needs the `engine` feature. Without it, `BrowserHost` keeps its tab model and mock
//! tabs, and reports `EngineStatus::Unsupported` instead of starting an engine.

pub mod address;
#[cfg(feature = "engine")]
mod background;
mod client;
#[cfg(feature = "engine")]
mod download;
pub mod installer;
mod keys;
#[cfg(all(feature = "engine", target_os = "macos"))]
pub mod mach_pipe;
#[cfg(feature = "engine")]
mod prefetch;
#[cfg(feature = "engine")]
pub mod profile;
pub mod protocol;
#[cfg(feature = "engine")]
mod sidecar;
mod site_zoom;
#[cfg(feature = "engine")]
mod source;
mod surface;
#[cfg(feature = "engine")]
mod transport;
#[cfg(feature = "engine")]
mod trust;
#[cfg(feature = "engine")]
mod unpack;

pub use address::{bare_url, host_of, resolve_address};
pub use client::BrowserHost;
pub use client::BrowserTab;
pub use installer::{ENGINE_DOWNLOAD_MB, ENGINE_REVISION, ERR_DOWNLOAD, ERR_STOPPED, EngineStatus, Failure, PauseReason, Version};
#[cfg(feature = "engine")]
pub use profile::shared_profile_dir;
pub use protocol::CursorKind;
pub use protocol::PickedElement;
pub use site_zoom::ZoomStore;
