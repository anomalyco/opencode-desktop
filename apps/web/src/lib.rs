//! Browser entry for OpenCode (Rune): compiles `opencode-desktop` as a library for `wasm32`.

#[cfg(target_arch = "wasm32")]
mod browser_history;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
pub use web::start;
