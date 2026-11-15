//! The migration is opt-in until all bundled surfaces have been compared with their legacy
//! redraws. Choosing a mode only consults the source tier and manifest id, never a VM.

use std::sync::OnceLock;

use super::{Half, Tier};
use rune_luau::Script;

pub(super) fn explicit(tier: Tier, id: &str) -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    tier == Tier::Bundled && matches!(id,
        "browser" | "btw" | "context" | "debug" | "details" | "file" | "goal" | "microsoft-office" |
        "pairing" | "review" | "rune-ui" | "sources" | "ssh" | "terminal" | "updater" | "wsl"
    ) && *ENABLED.get_or_init(|| std::env::var_os("OPENCODE_PLUGIN_REDRAW").is_some_and(|v| v == "explicit"))
}

impl Half {
    /// A new VM always redraws. A migrated VM redraws on its signal or a contribution change;
    /// a legacy VM additionally redraws on any script execution. Native reads are tracked apart.
    pub(super) fn surface_version(&self, script: Option<&Script>) -> (Option<u64>, u64, u64) {
        (script.and_then(Script::vm_id), self.epoch.get(), if self.owns_redraw { 0 } else { script.map_or(0, Script::version) })
    }
}
