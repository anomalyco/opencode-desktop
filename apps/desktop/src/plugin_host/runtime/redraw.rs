//! Desktop redraw policy adapter over `opencode_plugin_runtime::redraw`.

pub use opencode_plugin_runtime::redraw::explicit;

use rune_luau::Script;

use super::plugin::Half;

impl Half {
    /// A new VM always redraws. A migrated VM redraws on its signal or a contribution change;
    /// a legacy VM additionally redraws on any script execution. Native reads are tracked apart.
    pub(crate) fn surface_version(&self, script: Option<&Script>) -> (Option<u64>, u64, u64) {
        opencode_plugin_runtime::redraw::surface_version(script, self.epoch.get(), self.owns_redraw)
    }
}
