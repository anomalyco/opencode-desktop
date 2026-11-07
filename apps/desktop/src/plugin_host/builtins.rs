//! `--features static-builtins`: built-in plugins linked into the app, entered through the
//! same `extern "C"` entry point their libraries export, for headless and fuzzer builds that
//! want one binary. Release builds ship the same plugins as libraries in `<app>/plugins/`.

use opencode_plugin::abi::PluginEntryFn;

/// Each linked built-in: its `plugin.toml` and its entry point.
#[cfg(feature = "static-builtins")]
pub fn all() -> Vec<(&'static str, PluginEntryFn)> {
    vec![(include_str!("../../../../plugins/hello/plugin.toml"), opencode_plugin_hello::__opencode_plugin_entry)]
}

#[cfg(not(feature = "static-builtins"))]
pub fn all() -> Vec<(&'static str, PluginEntryFn)> {
    Vec::new()
}
