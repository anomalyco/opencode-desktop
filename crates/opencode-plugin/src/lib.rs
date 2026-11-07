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
//! A plugin has a window half ([`WindowPlugin`], one per window: its UI), an app half
//! ([`AppPlugin`], one per process: workers and [`Service`]s), or both. Plugins share typed
//! values in-process through [`Contract`]s read as [`Live`] values, keep state in host [`Store`]s,
//! and run background work as [`LatestTask`]s.
//!
//! Every entry point runs under `catch_unwind`: a panic reaches the host as a status, never as
//! an unwind across the boundary. Everything here is experimental (`api = "0.1-experimental"`).
#![allow(unsafe_code)]

pub mod abi;
pub mod alloc;
mod app;
mod boundary;
pub mod contrib;
mod cx;
mod el;
#[doc(hidden)]
pub mod export;
pub mod host;
mod layout_hash;
mod registry;
pub mod shared;

pub use abi::{EditorHandle, ElHandle, Rgba};
pub use app::{AppPlugin, AppSetupCx};
pub use contrib::{Contribution, HostPage, Menu, Menubar, ServerEntry, ServerState, SlotName};
pub use cx::{ActionCx, FxHasher, Palette, PluginError, PluginWaker, PollCx, Props, ViewCx, WindowPlugin, WindowSetupCx};
pub use el::{El, Handler, col, el, icon, icon_id, odometer, row, spacer, text, text_input, ui};
pub use host::{ModelInfo, PanelState, Resource, Session};
pub use registry::{CommandSection, CommandSpec, PanelSpec, SettingSpec};
pub use shared::{Contract, IconId, Inactive, LatestTask, Live, LiveState, Service, Store, StoreValue, intern_icon, spawn_after};

/// Everything a plugin usually needs.
pub mod prelude {
    pub use crate::{
        ActionCx, AppPlugin, AppSetupCx, CommandSection, CommandSpec, Contract, Contribution, EditorHandle, El, HostPage, Inactive, LatestTask, Live,
        LiveState, Menu, Menubar, ModelInfo, Palette, PanelSpec, PanelState, PluginError, PluginWaker, PollCx, Props, Resource, Rgba, ServerEntry,
        ServerState, Service, Session, SettingSpec, SlotName, Store, StoreValue, ViewCx, WindowPlugin, WindowSetupCx, col, el, icon, icon_id,
        intern_icon, odometer, row, spacer, text, text_input, ui,
    };
}
