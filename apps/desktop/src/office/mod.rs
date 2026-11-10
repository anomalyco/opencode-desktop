//! Word, Excel and PowerPoint previews for the Microsoft Office plugin, which drives them through
//! the `Office` service and the `ui.officePages` and `ui.officeSheet` nodes. Each open file runs
//! on a thread of its own (`worker.rs`) under a state machine the windows share
//! (`registry.rs`); pages draw as Vello scenes into effect slots (`compositor.rs`).

mod cjk;
mod compositor;
pub mod pages;
pub mod registry;
pub mod sheet;
mod worker;

pub use compositor::OfficeCompositor;

/// Takes the file threads' and the font fetch's news, and does what a sheet's grid asked of the
/// window (a copy, a toast, a scroll); true when a view should draw again.
pub fn poll(app: &mut crate::App, tx: &mut rune::Tx) -> bool {
    let changed = registry::poll() | cjk::poll();
    for effect in registry::take_ui() {
        sheet::run(app, effect, tx);
    }
    changed
}
