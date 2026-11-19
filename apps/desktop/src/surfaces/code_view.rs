//! Desktop host adapter for `opencode-code-view` (`crates/opencode-code-view`).

pub use opencode_code_view::*;
pub use opencode_code_view::diff::{
    find as diff_find, nav as diff_nav, state as diff_state, view as diff_view,
};
pub use opencode_code_view::file::{state as file_state, view as file_view};

use rune::{KeyEvent, Tx};

use crate::keymap::Bind;

/// Desktop keybinding adapter for `opencode_code_view::KeyMatch`.
#[derive(Clone, Copy, Default)]
pub struct DesktopKeys;

impl KeyMatch for DesktopKeys {
    fn is_find(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        Bind::Find.matches_tx(ev, tx)
    }

    fn is_copy_selection(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        Bind::CopySelection.matches_tx(ev, tx)
    }
}

/// Desktop frame-trace hooks for `CodeViewer::trace`.
pub fn trace_hooks() -> TraceHooks {
    TraceHooks {
        on: crate::files::trace::on,
        view: crate::files::trace::view,
        row: crate::files::trace::row,
    }
}
