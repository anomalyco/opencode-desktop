//! The native menu bar's own items (macOS): Go's Back and Forward and Edit's Find items. Each
//! shows the keys its command has now, so a change in Settings › Shortcuts reaches the menu bar
//! at once, as Desktop rebuilds its menu from the keybinds; the app's first frame puts the saved
//! changes there too.

use std::sync::OnceLock;

use rune::{AppHandle, MenuItem};

use crate::App;
use crate::keymap::Bind;

static HANDLE: OnceLock<AppHandle<App>> = OnceLock::new();

/// `item` with `bind`'s first chord as its key equivalent, none while the command is unbound.
pub fn keyed(item: MenuItem<App>, bind: Bind) -> MenuItem<App> {
    match bind.shortcut() {
        Some(keys) => item.key_equivalent(keys),
        None => item,
    }
}

fn items() -> Vec<MenuItem<App>> {
    crate::history::menu_items().into_iter().chain(crate::find::menu_items()).collect()
}

pub fn install(app: rune::App<App>) -> rune::App<App> {
    let app = items().into_iter().fold(app, |a, i| a.menu_item(i));
    let _ = HANDLE.set(app.handle());
    app
}

/// Puts the commands' current keys on the menu bar. Off macOS, where the menu is drawn in the
/// window from the keymap each time it opens, Rune has no native menu and this does nothing.
pub fn refresh() {
    if let Some(handle) = HANDLE.get() {
        let _ = handle.set_menu(items());
    }
}
