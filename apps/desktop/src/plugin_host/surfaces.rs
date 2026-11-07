//! The plugin contributions drawn into the shell: titlebar items, composer pills, shell slots,
//! file viewers and settings pages. Each is a plugin render on its own surface; a render that
//! panics draws nothing there (and shows on the plugin's Settings row), the rest of the shell
//! carries on.

use opencode_plugin::abi::{RENDER_ACTIVE, SURFACE_FILE, SURFACE_PILL, SURFACE_SETTINGS_PAGE, SURFACE_SLOT, SURFACE_TITLEBAR, contrib as kinds};
use rune::{El, Tx, col, row};

use super::contrib::active;
use super::icons::intern;
use super::instance::ContribRec;
use super::render::{self, Props};
use super::run;
use crate::App;
use crate::palette::Pal;

/// Draws contribution `c` of slot `i`; `press` wraps it in its `run` callback.
fn draw(app: &App, p: &Pal, i: usize, c: &ContribRec, props: &Props, press: bool) -> Option<El> {
    let el = render::surface(app, i, c.render.as_ref()?, props, p).ok()??;
    let Some(cb) = c.run.as_ref().map(|r| r.cb).filter(|_| press) else { return Some(el) };
    let scope = c.scope;
    Some(el.on_press_tx(move |s: &mut App, tx: &mut Tx| {
        tx.instant();
        run::action(s, tx, i, scope, cb, opencode_plugin::abi::EVENT_PRESS, "", "");
    }))
}

fn sorted(app: &App, kind: u32) -> Vec<(usize, &ContribRec)> {
    let mut list: Vec<_> = active(&app.plugins, kind).collect();
    list.sort_by_key(|(_, c)| c.order);
    list
}

/// Titlebar items at the trailing end of Windows' titlebar row, before the caption buttons.
/// Without a titlebar row (macOS) they belong in the sidebar's footer, which the vertical tab
/// layouts draw; until then they show nowhere.
pub fn titlebar_items(app: &App, p: &Pal) -> Option<El> {
    let chrome = crate::view::chrome();
    if chrome.row_h <= 0.0 {
        return None;
    }
    let items: Vec<El> = sorted(app, kinds::TITLEBAR_ITEM).into_iter().filter_map(|(i, c)| draw(app, p, i, c, &Props::new(SURFACE_TITLEBAR, &c.id), true)).collect();
    if items.is_empty() {
        return None;
    }
    Some(
        row()
            .key("plugin-titlebar-items")
            .pin(None, Some(0.0), Some(chrome.caption_w + 8.0), None)
            .h(chrome.row_h)
            .gap(6.0)
            .center_y()
            .no_window_drag()
            .children(items),
    )
}

/// The capsules above root session `session`'s composer.
pub fn composer_pills(app: &App, p: &Pal, session: &str) -> Option<El> {
    let directory = app.session(session).map_or("", |i| i.directory.as_str());
    let pills: Vec<El> = sorted(app, kinds::COMPOSER_PILL)
        .into_iter()
        .filter_map(|(i, c)| draw(app, p, i, c, &Props { session, directory, ..Props::new(SURFACE_PILL, &c.id) }, true))
        .collect();
    (!pills.is_empty()).then(|| row().key("plugin-composer-pills").w_full().max_w(crate::view::CONTENT_W).pb(8.0).gap(6.0).center().children(pills))
}

/// What plugins put in shell slot `name` (`window.bottom`, `session.header`; `session.panel.end`
/// and `session.location.menu` once those menus exist), for session `session`.
pub fn slot(app: &App, p: &Pal, name: &str, session: &str) -> Vec<El> {
    slot_with(app, p, name, session, RENDER_ACTIVE)
}

/// [`slot`] with the slot's `RENDER_*` flags (`session.header`'s `active` and `compact`).
pub fn slot_with(app: &App, p: &Pal, name: &str, session: &str, flags: u32) -> Vec<El> {
    let directory = app.session(session).map_or("", |i| i.directory.as_str());
    sorted(app, kinds::SLOT)
        .into_iter()
        .filter(|(_, c)| c.target == name)
        .filter_map(|(i, c)| draw(app, p, i, c, &Props { session, directory, flags, ..Props::new(SURFACE_SLOT, &c.id) }, false))
        .collect()
}

/// A plugin's view of file `path` (in session `session`'s folder `directory`, its loaded
/// `text` when it has loaded as text), when a file viewer takes its extension.
pub fn file_viewer(app: &App, p: &Pal, session: &str, directory: &str, path: &str, text: &str) -> Option<El> {
    let ext = std::path::Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
    let (i, c) = sorted(app, kinds::FILE_VIEWER).into_iter().find(|(_, c)| c.target.split_whitespace().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)))?;
    let abs = if std::path::Path::new(path).is_absolute() { path.to_owned() } else { format!("{}/{path}", directory.trim_end_matches(['/', '\\'])) };
    let props = Props { session, directory, path: &abs, text, ..Props::new(SURFACE_FILE, &c.id) };
    let body = match render::surface(app, i, c.render.as_ref()?, &props, p) {
        Ok(el) => el.unwrap_or_else(rune::el),
        Err(e) => render::card(p, &app.plugins.slots[i].manifest.name, &format!("Stopped drawing: {e}"), Some(&app.plugins.slots[i].manifest.id), true),
    };
    Some(col().key(format!("plugin-file-{}", c.key)).w_full().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(body))
}

/// The settings pages plugins add on host page `page` (`general`, `servers`), or their own
/// pages (`""`), as sections in Settings' one column.
pub fn settings_pages(app: &App, p: &Pal, page: &str) -> Vec<El> {
    sorted(app, kinds::SETTINGS_PAGE)
        .into_iter()
        .filter(|(_, c)| c.target == page)
        .filter_map(|(i, c)| {
            let body = draw(app, p, i, c, &Props::new(SURFACE_SETTINGS_PAGE, &c.id), false)?;
            let card = col().key(format!("plugin-page-{}", c.key)).w_full().px(20.0).py(16.0).radius(8.0).fill(p.layer1).border(0.5, p.border_muted).child(body);
            Some(crate::settings::section(p, intern(&c.title), card))
        })
        .collect()
}
