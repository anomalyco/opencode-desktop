//! The window's rows, read once as the window opens, in Desktop's mixed-server shape. A window
//! from before that shape was migrated once, as the database took the files in
//! (`opencode_storage::native::import`, MS-1's `migration::window`).

use opencode_kit::servers::tabs::WindowDoc;

use crate::App;

/// Window `app`'s saved tabs, page, shown draft and panes (its closed tabs and other drafts come
/// in once the storage is ready, `App::hydrate_restore`).
pub fn read_window(app: &App) -> WindowDoc {
    WindowDoc::from_json(&crate::windows::store::load(&app.window))
}
