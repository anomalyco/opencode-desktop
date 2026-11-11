//! `--features static-builtins`: built-in plugins linked into the app, entered through the
//! same `extern "C"` entry point their libraries export, for headless and fuzzer builds that
//! want one binary. Release builds ship the same
//! plugins as libraries in `<app>/plugins/`.
//!
//! The app links each built-in's rlib (`plugins/<id>/impl`), which only declares the plugin;
//! the cdylib beside it (`plugins/<id>`) exports it. So a library built in the same workspace
//! build as this feature still exports `opencode_plugin_v1` and its own allocator.

use opencode_plugin::abi::PluginEntryFn;

/// Each linked built-in: its `plugin.toml` and its entry point.
#[cfg(feature = "static-builtins")]
pub fn all() -> Vec<(&'static str, PluginEntryFn)> {
    #[allow(unused_mut)]
    let mut all: Vec<(&'static str, PluginEntryFn)> = Vec::new();
    all
}

#[cfg(not(feature = "static-builtins"))]
pub fn all() -> Vec<(&'static str, PluginEntryFn)> {
    Vec::new()
}
