//! Desktop's built-in Context plugin (`packages/gui-plugins/src/context`) as a native plugin: the
//! session header's context ring (its `session.header` slot, order 10) with the session's
//! tokens and spend, which opens the Context tab; and that tab, with the session's token and
//! cost figures, its system prompt and its raw messages, which it exports.
//!
//! A built-in: this rlib declares it, `plugins/context` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod format;
mod indicator;
mod json;
mod tab;

use opencode_plugin::abi::{ALIGN_END, SIDE_BOTTOM};
use opencode_plugin::prelude::*;

/// The Context tab's panel id, as Desktop's `context:main`.
pub(crate) const PANEL: &str = "main";

pub struct Context {
    /// Raw message rows shown open, by message id.
    open: Vec<String>,
    /// An export in flight.
    export: Option<tab::Export>,
}

impl WindowPlugin for Context {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        cx.add_slot(Contribution::slot(SlotName::SessionHeader, "usage").order(10), indicator::header);
        cx.add_popover(Contribution::popover("usage", "View context usage", SIDE_BOTTOM, ALIGN_END, 4), indicator::menu);
        // Opened from its header button, so it has no New tab card (Desktop gives it no menu item).
        let panel = PanelSpec::new(PANEL, "Context").description("Usage").icon("status").launcher(false).first(true).fallback(true).fixed(true);
        cx.add_panel_with_label(panel, tab::panel, indicator::tab_label);
        Ok(Context { open: Vec::new(), export: None })
    }

    fn save_hot_state(&self) -> Vec<u8> {
        self.open.join("\n").into_bytes()
    }

    fn load_hot_state(&mut self, bytes: &[u8]) {
        self.open = String::from_utf8_lossy(bytes).lines().filter(|l| !l.is_empty()).map(str::to_owned).collect();
    }
}

opencode_plugin::declare_plugin!(window = Context);
