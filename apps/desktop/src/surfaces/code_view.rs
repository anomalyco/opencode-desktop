//! Desktop host adapter for `opencode-feature-code-view` (`features/code-view`).

pub use opencode_feature_code_view::*;
pub use opencode_feature_code_view::diff::{
    find as diff_find, nav as diff_nav, state as diff_state, view as diff_view,
};
pub use opencode_feature_code_view::file::{state as file_state, view as file_view};

use opencode_kernel::{Feature, KernelBuilder, SurfaceDescriptor, SurfaceKind};
use rune::{KeyEvent, Tx};

use crate::App;
use crate::keymap::Bind;

/// Desktop keybinding adapter for `opencode_feature_code_view::KeyMatch`.
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

/// Kernel feature registration for the virtualized code & diff surface.
pub struct CodeViewFeature;

impl Feature<App> for CodeViewFeature {
    fn id(&self) -> &'static str {
        "feature.code-view"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_surface(
            SurfaceDescriptor::new("code-view", SurfaceKind::CodeView, "Code Viewer")
                .with_multi_instance(true),
        );
        builder.add_surface(
            SurfaceDescriptor::new("diff", SurfaceKind::Diff, "Diff Viewer")
                .with_multi_instance(true),
        );
    }
}
