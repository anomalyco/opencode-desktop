//! The plugin contributions drawn into the shell: titlebar items, composer pills, shell slots,
//! file viewers and settings pages. Each is a plugin render on its own surface; a render that
//! fails draws nothing there (and shows in the console), the rest of the shell carries on.

use rune::{El, col, row};

use crate::App;
use crate::palette::Pal;

/// Titlebar items at the trailing end of Windows' titlebar row, before the caption buttons.
/// Without a titlebar row (macOS), and in side tabs and the inbox, they belong in the
/// sidebar's footer ([`sidebar_items`]).
pub fn titlebar_items(app: &App, p: &Pal) -> Option<El> {
    let chrome = crate::view::chrome();
    if chrome.row_h <= 0.0 || app.tab_layout() != crate::layout_state::TabLayout::Horizontal {
        return None;
    }
    let items = super::luau::surfaces::titlebar_items(app, p, false);
    if items.is_empty() {
        return None;
    }
    Some(
        row()
            .key("plugin-titlebar-items")
            .pin(None, (!chrome.bottom).then_some(0.0), Some(chrome.caption_w + 8.0), chrome.bottom.then_some(0.0))
            .h(chrome.row_h)
            .gap(6.0)
            .center_y()
            .no_window_drag()
            .children(items),
    )
}

/// The sidebar's footer of titlebar items, in side tabs and the inbox (`vertical-tabs-footer`):
/// 8 under the list, a column of pills.
pub fn sidebar_items(app: &App, p: &Pal) -> Option<El> {
    let items = super::luau::surfaces::titlebar_items(app, p, true);
    (!items.is_empty()).then(|| col().key("vertical-tabs-footer").mt(8.0).w_full().items_start().children(items))
}

/// The pills plugins put in the composer lane after the app's own (`SessionPluginPills`).
pub fn pill_views(app: &App, p: &Pal, session: &str) -> Vec<El> {
    super::luau::surfaces::pill_views(app, p, session)
}

/// What plugins put in shell slot `name` (`window.bottom`, `session.header`) for session
/// `session`.
pub fn slot(app: &App, p: &Pal, name: &str, session: &str) -> Vec<El> {
    slot_with(app, p, name, session, true, false)
}

/// [`slot`] as the session header draws it: `active` for the routed session, `compact` in a
/// narrow header.
pub fn slot_with(app: &App, p: &Pal, name: &str, session: &str, active: bool, compact: bool) -> Vec<El> {
    let mut all = super::luau::surfaces::slot(app, p, name, session, active, compact);
    all.sort_by_key(|(order, _)| *order);
    all.into_iter().map(|(_, el)| el).collect()
}

/// A plugin's view of file `path` (in session `session`'s folder `directory`), when a file
/// viewer takes its extension.
pub fn file_viewer(app: &App, p: &Pal, session: &str, directory: &str, path: &str) -> Option<El> {
    super::luau::surfaces::file_viewer(app, p, session, directory, path)
}

/// The settings pages plugins add on host page `page` (`general`, `servers`), as sections of
/// that page (`PluginSettingsSections`). A section draws its own title and list
/// (`ui.settingsList`), as Desktop's do.
pub fn settings_pages(app: &App, p: &Pal, page: &str) -> Vec<El> {
    super::luau::surfaces::settings_pages(app, Some(page))
        .into_iter()
        .filter_map(|(i, key, spec, f)| {
            let body = super::luau::surfaces::settings_page_body(app, p, i, &spec, f.as_ref()?, None);
            Some(col().key(format!("plugin-page-{key}")).w_full().child(body))
        })
        .collect()
}

/// The plugins' own Settings pages, in order, by key, each with whether it belongs beside
/// Experimental and About (`group: "about"`).
pub fn settings_page_ids(app: &App) -> Vec<(String, bool)> {
    super::luau::surfaces::settings_pages(app, None).into_iter().map(|(_, key, spec, _)| (key.to_owned(), spec.about == Some(true))).collect()
}

fn own_page(app: &App, key: &str) -> Option<(usize, &'static str, opencode_plugin_spec::SettingsPageSpec, Option<mlua::Function>)> {
    super::luau::surfaces::settings_pages(app, None).into_iter().find(|(_, k, spec, _)| *k == key || spec.id == key)
}

/// The key of the plugin page a manifest's search entry names by its id.
pub fn settings_page_key(app: &App, id: &str) -> String {
    own_page(app, id).map_or_else(|| id.to_owned(), |(_, key, _, _)| key.to_owned())
}

pub fn settings_page_title(app: &App, key: &str) -> Option<String> {
    own_page(app, key).map(|(_, _, spec, _)| spec.title)
}

/// The controls a plugin's page draws at the end of its header (`renderActions`).
pub fn settings_page_actions(app: &App, p: &Pal, key: &str) -> Option<El> {
    let (i, _, spec, _) = own_page(app, key)?;
    super::luau::surfaces::settings_page_actions(app, p, i, &spec)
}

/// A plugin page's rail icon: one it interned or a host icon by name.
pub fn settings_page_icon(app: &App, key: &str) -> Option<&'static rune::Icon> {
    own_page(app, key).and_then(|(_, _, spec, _)| spec.icon).map(|icon| super::icons::resolve("", &icon))
}

/// A plugin page: its title, description and what it draws.
pub fn settings_page(app: &App, p: &Pal, key: &str) -> Option<(String, String, El)> {
    let (i, _, spec, f) = own_page(app, key)?;
    let body = super::luau::surfaces::settings_page_body(app, p, i, &spec, f.as_ref()?, None);
    Some((spec.title.clone(), spec.description.clone().unwrap_or_default(), body))
}
