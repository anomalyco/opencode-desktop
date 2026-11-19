//! Which plugins redraw only when they ask (`writing-a-plugin.md`, section 4). A built-in does
//! once each state it draws has been driven live and drew the same frames as the legacy redraw,
//! on each platform apart: the "live" cells in `apps/desktop/src/plugin_host/redraw-audit.md`. Keep `LIVE` and that table in
//! step. Every other built-in, and installed, override and `--plugin-dir` plugins, keep the
//! legacy redraw after any script execution. Choosing a mode only consults the source tier and
//! manifest id, never a VM.
//!
//! Debug builds compare the two on one run: `OPENCODE_PLUGIN_REDRAW=implicit` puts every
//! built-in on the legacy redraw, and `OPENCODE_PLUGIN_REDRAW=explicit:updater,browser` also
//! moves the named built-ins to their own redraws, for proving a row before it says "live".

use rune_luau::Script;

use crate::sources::Tier;

/// The built-ins whose rows in `apps/desktop/src/plugin_host/redraw-audit.md` say "live" in this platform's column: each was
/// driven on that platform. A platform where none has run keeps every built-in on the legacy
/// redraw.
#[cfg(windows)]
const LIVE: &[&str] = &["context", "goal", "rune-ui", "sources", "ssh", "terminal", "updater", "wsl"];
#[cfg(target_os = "macos")]
const LIVE: &[&str] = &["context", "goal", "rune-ui", "terminal"];
#[cfg(not(any(windows, target_os = "macos")))]
const LIVE: &[&str] = &[];

/// Whether a plugin from `tier` with manifest id `id` owns explicit surface redraws.
pub fn explicit(tier: Tier, id: &str) -> bool {
    tier == Tier::Bundled && comparison(id).unwrap_or_else(|| LIVE.contains(&id))
}

/// The debug override for built-in `id`, if one is set.
#[cfg(debug_assertions)]
fn comparison(id: &str) -> Option<bool> {
    static MODE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    #[allow(clippy::disallowed_methods)]
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

/// A new VM always redraws. A migrated VM (`owns_redraw = true`) redraws on its signal or a
/// contribution change (`epoch`); a legacy VM additionally redraws on any script execution.
/// Native reads are tracked apart.
pub fn surface_version(script: Option<&Script>, epoch: u64, owns_redraw: bool) -> (Option<u64>, u64, u64) {
    (
        script.and_then(Script::vm_id),
        epoch,
        if owns_redraw { 0 } else { script.map_or(0, Script::version) },
    )
}
