//! Which plugins redraw only when they ask (`writing-a-plugin.md`, section 4). The 16 built-ins
//! do; installed, override and `--plugin-dir` plugins keep the legacy redraw after any script
//! execution. Debug builds can put the built-ins back on the legacy redraw with
//! `OPENCODE_PLUGIN_REDRAW=implicit`, to compare the two on the same run. Choosing a mode only
//! consults the source tier and manifest id, never a VM.

use super::{Half, Tier};
use rune_luau::Script;

pub(super) fn explicit(tier: Tier, id: &str) -> bool {
    tier == Tier::Bundled && matches!(id,
        "browser" | "btw" | "context" | "debug" | "details" | "file" | "goal" | "microsoft-office" |
        "pairing" | "review" | "rune-ui" | "sources" | "ssh" | "terminal" | "updater" | "wsl"
    ) && !legacy_comparison()
}

#[cfg(debug_assertions)]
fn legacy_comparison() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("OPENCODE_PLUGIN_REDRAW").is_some_and(|v| v == "implicit"))
}

#[cfg(not(debug_assertions))]
fn legacy_comparison() -> bool {
    false
}

impl Half {
    /// A new VM always redraws. A migrated VM redraws on its signal or a contribution change;
    /// a legacy VM additionally redraws on any script execution. Native reads are tracked apart.
    pub(super) fn surface_version(&self, script: Option<&Script>) -> (Option<u64>, u64, u64) {
        (script.and_then(Script::vm_id), self.epoch.get(), if self.owns_redraw { 0 } else { script.map_or(0, Script::version) })
    }
}
