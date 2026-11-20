//! Shared host platform utilities for Rune apps (`rune-desktop` and other embedder shells).
//!
//! Collects platform-facing primitives so feature crates depend on value types and standalone
//! helpers rather than reaching into `apps/desktop` globals:
//! - [`wall`]: process wall clock (`wall_ms`), with headless `--clock` support.
//! - [`telemetry`]: local OTLP/HTTP trace helpers for launch, session open, and stream latency.
//! - [`askpass`]: `SSH_ASKPASS` loopback helper entrypoint.
//! - [`web_fetch`]: cached unauthenticated web GET for images and favicons (native + wasm32).
//! - [`host`]: [`PlatformPaths`] and [`PlatformInfo`] value descriptors.
//! - [`storage`]: process-wide [`StoragePort`] persistence callbacks.

#![forbid(unsafe_code)]

pub mod askpass;
pub mod host;
pub mod storage;
pub mod telemetry;
pub mod wall;
pub mod web_fetch;

pub use host::{PlatformInfo, PlatformPaths};
pub use storage::{DocLookup, StoragePort};
pub use wall::wall_ms;
