//! Desktop host adapter for `opencode_ui::keymap` (`crates/opencode-ui`).

pub use opencode_ui::keymap::*;

impl KeymapHost for crate::App {
    fn default_command_chord(&self, id: &str) -> Option<Chord> {
        self.catalog()
            .into_iter()
            .find(|c| c.id == id)
            .and_then(|c| c.keys.and_then(Bind::chord).or(c.chord))
    }
}

/// Installs desktop's native menu bar refresh hook for keybind override updates.
pub fn init() {
    overrides::set_on_changed(crate::menubar::refresh);
}
