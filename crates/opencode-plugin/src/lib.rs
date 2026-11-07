//! Experimental SDK for native OpenCode (Rune) GUI plugins.
//!
//! A plugin is a Rust `cdylib` that depends on this crate alone and exports itself with
//! [`export_plugin!`]. The host loads it, checks the C ABI handshake ([`abi`]), and calls into it
//! through `#[repr(C)]` vtables. The plugin builds real `rune::El` trees in the host's arena with
//! [`row`], [`col`], [`text`] and friends, so it links no UI, GPU, text or HTTP code: a release
//! build with plain `cargo build --release` is a small library.
//!
//! ```ignore
//! use opencode_plugin::prelude::*;
//!
//! struct Hello { count: u32 }
//!
//! impl WindowPlugin for Hello {
//!     fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
//!         cx.add_command(CommandSpec::new("bump", "Hello: Bump").bind("mod+alt+h"), |s, _| s.count += 1);
//!         cx.add_panel(PanelSpec::new("main", "Hello"), |s, cx, _| {
//!             cx.memo("root", s.count, |_| text(format!("{} bumps", s.count)))
//!         });
//!         Ok(Hello { count: 0 })
//!     }
//! }
//!
//! opencode_plugin::export_plugin!(window = Hello);
//! ```
//!
//! Every entry point runs under `catch_unwind`: a panic reaches the host as a status, never as
//! an unwind across the boundary. Everything here is experimental (`api = "0.1-experimental"`).
#![allow(unsafe_code)]

pub mod abi;
pub mod alloc;
mod boundary;
mod cx;
mod el;
#[doc(hidden)]
pub mod export;
mod layout_hash;
mod registry;

pub use abi::{EditorHandle, ElHandle, Rgba};
pub use cx::{ActionCx, FxHasher, Palette, PluginError, PluginWaker, PollCx, Props, ViewCx, WindowPlugin, WindowSetupCx};
pub use el::{El, Handler, col, el, icon, row, spacer, text, text_input, ui};
pub use registry::{CommandSection, CommandSpec, PanelSpec, SettingSpec};

/// Everything a plugin usually needs.
pub mod prelude {
    pub use crate::{
        ActionCx, CommandSection, CommandSpec, EditorHandle, El, PanelSpec, PluginError, PluginWaker, PollCx, Props, Rgba, SettingSpec, ViewCx,
        WindowPlugin, WindowSetupCx, col, el, icon, row, spacer, text, text_input, ui,
    };
}
