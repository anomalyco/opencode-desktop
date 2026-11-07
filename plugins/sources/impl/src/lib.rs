//! Desktop's built-in Sources plugin (`packages/gui-plugins/src/sources`) as a native plugin: the
//! Sources experiment's side panel, one transient tab per turn, keyed by the user message that
//! opened it. The timeline's "N sources" chip opens it with a link only this plugin takes
//! (`opencode-sources:<message id>`). Turning the experiment off stops listing the tabs, which
//! closes them.
//!
//! A built-in: this rlib declares it, `plugins/sources` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod panel;
mod turn;

use std::collections::BTreeSet;

use opencode_plugin::prelude::*;
use opencode_plugin::system;

/// The panel's id: tabs are `sources.turn/<message id>`, as Desktop's `sources:<id>`.
pub(crate) const PANEL: &str = "turn";
/// The links the timeline's chip opens: the turn's user message id after it.
pub const LINK: &str = "opencode-sources:";

pub struct Sources {
    /// Search steps shown open, by tool call id.
    open: BTreeSet<String>,
}

pub(crate) fn enabled() -> bool {
    system::experiment("sources.enabled").as_deref() == Some("on")
}

impl WindowPlugin for Sources {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        cx.add_link_handler(
            Contribution::link_handler("turn", 0),
            |_, href| enabled() && href.starts_with(LINK),
            |_, cx| {
                let id = cx.input().strip_prefix(LINK).unwrap_or_default().to_owned();
                cx.open_panel_instance(PANEL, &id);
            },
        );
        cx.add_panel_instances(PanelSpec::new(PANEL, "Sources").icon("globe").transient(true), list, panel::render, panel::label, |_, _| {});
        Ok(Sources { open: BTreeSet::new() })
    }
}

/// One tab per turn while the experiment is on, titled "Sources · N" (`tab.count`).
fn list(_: &Sources, _session: &str, open: &[String]) -> Vec<PanelTab> {
    if !enabled() {
        return Vec::new();
    }
    open.iter().map(|id| PanelTab { id: id.clone(), title: "Sources".into(), hidden: false }).collect()
}

opencode_plugin::declare_plugin!(window = Sources);
