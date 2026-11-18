//! Plugin runtime: VM loading, app halves, lifecycle, dormancy, hot reload, trust, memory, and sources.

pub mod app_half;
pub(crate) mod boot;
pub(crate) mod dormant;
mod first_screen;
pub(crate) mod hot;
pub(crate) mod lifecycle;
pub(crate) mod memory;
pub(crate) mod plugin;
mod redraw;
pub mod sources;
pub mod trust;
#[cfg(not(target_arch = "wasm32"))]
pub mod types;
pub(crate) mod vm;
