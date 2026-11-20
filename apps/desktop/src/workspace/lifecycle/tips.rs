//! Desktop host adapter for `opencode-feature-tips` (`features/tips`).

pub use opencode_feature_tips::*;

use opencode_kernel::{Feature, KernelBuilder, Slot};

use crate::App;

static TIPS: Slot<Tips> = Slot::new();

impl TipsHost for App {
    fn tips(&self) -> &Tips {
        self.kernel.slots.get(&TIPS)
    }

    fn tips_mut(&mut self) -> &mut Tips {
        self.kernel.slots.get_mut(&TIPS)
    }

    fn is_connected(&self) -> bool {
        !crate::reconnect::down(self)
    }

    fn is_window_open(&self) -> bool {
        !self.plugins.window_blurred.get() && self.toast.stack.page() == rune_kit::ToastPage::Shown
    }

    fn route_key(&self) -> TipRoute {
        TipRoute::from_hash(rune::Tracked::untracked(&self.route))
    }

    fn browse_installed(&self) -> Option<bool> {
        self.mcp_servers("").map(|list| list.iter().any(|m| m.name == "browse"))
    }

    fn hover_none(&self) -> bool {
        session_view::style::hover_none()
    }

    fn sync_mcp(&mut self) {
        App::sync_mcp(self, "");
    }

    fn read_json(&self, ns: &str, key: &str) -> Option<serde_json::Value> {
        crate::storage::json(ns, key)
    }

    fn write_json(&self, ns: &str, key: &str, val: &serde_json::Value) {
        crate::storage::set_json(ns, key, val);
    }

    fn show_tip_toast(&mut self, tip: Tip) -> u64 {
        crate::status_toast::tip(self, tip)
    }

    fn dismiss_toast(&mut self, id: u64) {
        crate::status_toast::dismiss_id(self, id);
    }
}

/// Kernel feature registration for paced one-time tips.
pub struct TipsFeature;

impl Feature<App> for TipsFeature {
    fn id(&self) -> &'static str {
        "feature.tips"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&TIPS);
        // Known: reads `reconnect::down` via `is_connected()`.
        builder
            .schedule(crate::tick::stage::REACT_STATUS, "tips.poll", |s, _, tx| poll(s, tx))
            .after("reconnect.poll");
    }
}

fn force_scene(s: &mut App, tip: Tip) {
    crate::scenes::open_scene(s);
    s.finish_stream();
    let _ = opencode_feature_tips::force(s, tip);
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("tip-browse", |s| force_scene(s, Tip::Browse)),
    ("tip-hex", |s| force_scene(s, Tip::Hex)),
]);
