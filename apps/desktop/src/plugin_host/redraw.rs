//! Which plugins redraw only when they ask (`writing-a-plugin.md`, section 4). A built-in does
//! once each state it draws has been driven live and drew the same frames as the legacy redraw:
//! the rows marked "live" in `redraw-audit.md`, beside this file. Keep `LIVE` and that table in
//! step. Every other built-in, and installed, override and `--plugin-dir` plugins, keep the
//! legacy redraw after any script execution. Choosing a mode only consults the source tier and
//! manifest id, never a VM.
//!
//! Debug builds compare the two on one run: `OPENCODE_PLUGIN_REDRAW=implicit` puts every
//! built-in on the legacy redraw, and `OPENCODE_PLUGIN_REDRAW=explicit:updater,browser` also
//! moves the named built-ins to their own redraws, for proving a row before it says "live".

use super::{Half, Tier};
use rune_luau::Script;

/// The built-ins whose rows in `redraw-audit.md` say "live".
const LIVE: [&str; 6] = ["context", "goal", "rune-ui", "sources", "terminal", "updater"];

pub(super) fn explicit(tier: Tier, id: &str) -> bool {
    tier == Tier::Bundled && comparison(id).unwrap_or_else(|| LIVE.contains(&id))
}

/// The debug override for built-in `id`, if one is set.
#[cfg(debug_assertions)]
fn comparison(id: &str) -> Option<bool> {
    static MODE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let mode = MODE.get_or_init(|| std::env::var("OPENCODE_PLUGIN_REDRAW").ok()).as_deref()?;
    if mode == "implicit" {
        return Some(false);
    }
    mode.strip_prefix("explicit:").and_then(|ids| ids.split(',').any(|n| n == id).then_some(true))
}

#[cfg(not(debug_assertions))]
fn comparison(_: &str) -> Option<bool> {
    None
}

impl Half {
    /// A new VM always redraws. A migrated VM redraws on its signal or a contribution change;
    /// a legacy VM additionally redraws on any script execution. Native reads are tracked apart.
    pub(super) fn surface_version(&self, script: Option<&Script>) -> (Option<u64>, u64, u64) {
        (script.and_then(Script::vm_id), self.epoch.get(), if self.owns_redraw { 0 } else { script.map_or(0, Script::version) })
    }
}
