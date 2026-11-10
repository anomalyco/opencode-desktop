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
    let mut items: Vec<El> = sorted(app, kinds::TITLEBAR_ITEM).into_iter().filter_map(|(i, c)| draw(app, p, i, c, &Props::new(SURFACE_TITLEBAR, &c.id), true)).collect();
    // In side tabs and the inbox, Desktop puts them in the sidebar's footer instead.
    if app.tab_layout() != crate::layout_state::TabLayout::Horizontal {
        return None;
    }
    items.extend(super::luau::surfaces::titlebar_items(app, p, false));
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

/// What a host-drawn composer pill shows (`ComposerPillView`), as its plugin answered.
pub struct PillView {
    pub label: String,
    pub icon: String,
    pub tone: String,
    pub title: String,
}

/// Composer pill `c` of slot `i` for session `session`, when it shows one.
pub fn pill_view(app: &App, i: usize, c: &ContribRec, session: &str) -> Option<PillView> {
    let query = c.query.as_ref()?;
    let directory = app.session(session).map_or("", |s| s.directory.as_str());
    let answer = run::peek(app, i, &query.cb, &format!("{session}\n{directory}"))?;
    let mut f = answer.split('\t').map(str::to_owned);
    Some(PillView { label: f.next()?, icon: f.next().unwrap_or_default(), tone: f.next().unwrap_or_default(), title: f.next().unwrap_or_default() })
}

/// The pills plugins put in the composer lane after the app's own (`SessionPluginPills`): the
/// capsule every lane pill shares, its icon in its tone, its label cut past 260. A pill with a
/// menu opens it as a popover above; one without runs its press.
pub fn pill_views(app: &App, p: &Pal, session: &str) -> Vec<El> {
    let luau = super::luau::surfaces::pill_views(app, p, session);
    sorted(app, kinds::COMPOSER_PILL)
        .into_iter()
        .filter_map(|(i, c)| {
            let view = pill_view(app, i, c, session)?;
            let key = format!("plugin-pill-{}", c.key);
            let open = app.plugins.popover.as_ref().is_some_and(|o| o.slot == i && o.id == c.id && o.pill);
            let name = if view.title.is_empty() { view.label.clone() } else { view.title.clone() };
            let ink = match view.tone.as_str() {
                "success" | "warning" | "danger" => session_view::palette::token(p.dark, &format!("state-fg-{}", view.tone)).unwrap_or(p.icon_muted),
                _ => p.icon_muted,
            };
            let mut pill = crate::subagents::chip(app, p, &key, name, open).shrink().min_w(0.0);
            if !view.icon.is_empty() {
                pill = pill.child(rune::icon(super::icons::resolve(&app.plugins.slots[i].manifest.id, &view.icon), 14.0).color(ink));
            }
            pill = pill.child(rune::text(view.label).font(crate::view::UI).color(p.text).truncate().max_w(260.0).shrink().min_w(0.0));
            let (scope, menu, run) = (c.scope, c.render.is_some(), c.run.as_ref().map(|r| r.cb));
            let id = c.id.clone();
            Some(pill.on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if menu {
                    super::popover::toggle_pill(s, i, &id, &key, tx);
                } else if let Some(cb) = run {
                    run::action(s, tx, i, scope, cb, opencode_plugin::abi::EVENT_PRESS, "", "");
                }
            }))
        })
        .chain(luau)
        .collect()
}

/// The capsules above root session `session`'s composer.
pub fn composer_pills(app: &App, p: &Pal, session: &str) -> Option<El> {
    let directory = app.session(session).map_or("", |i| i.directory.as_str());
    let pills: Vec<El> = sorted(app, kinds::COMPOSER_PILL)
        .into_iter()
        .filter(|(_, c)| c.query.is_none())
        .filter_map(|(i, c)| draw(app, p, i, c, &Props { session, directory, ..Props::new(SURFACE_PILL, &c.id) }, true))
        .collect();
    (!pills.is_empty()).then(|| row().key("plugin-composer-pills").w_full().max_w(session_view::style::content_w()).pb(8.0).gap(6.0).center().children(pills))
}

/// What plugins put in shell slot `name` (`window.bottom`, `session.header`; `session.panel.end`
/// and `session.location.menu` once those menus exist), for session `session`.
pub fn slot(app: &App, p: &Pal, name: &str, session: &str) -> Vec<El> {
    slot_with(app, p, name, session, RENDER_ACTIVE)
}

/// [`slot`] with the slot's `RENDER_*` flags (`session.header`'s `active` and `compact`).
pub fn slot_with(app: &App, p: &Pal, name: &str, session: &str, flags: u32) -> Vec<El> {
    let directory = app.session(session).map_or("", |i| i.directory.as_str());
    let native = sorted(app, kinds::SLOT)
        .into_iter()
        .filter(|(_, c)| c.target == name)
        .filter_map(|(i, c)| Some((i64::from(c.order), draw(app, p, i, c, &Props { session, directory, flags, ..Props::new(SURFACE_SLOT, &c.id) }, false)?)));
    let luau = super::luau::surfaces::slot(app, p, name, session, flags & RENDER_ACTIVE != 0, flags & opencode_plugin::abi::RENDER_COMPACT != 0);
    let mut all: Vec<(i64, El)> = native.chain(luau).collect();
    all.sort_by_key(|(order, _)| *order);
    all.into_iter().map(|(_, el)| el).collect()
}

/// A plugin's view of file `path` (in session `session`'s folder `directory`, its loaded
/// `text` when it has loaded as text), when a file viewer takes its extension.
pub fn file_viewer(app: &App, p: &Pal, session: &str, directory: &str, path: &str, text: &str) -> Option<El> {
    if let Some(el) = super::luau::surfaces::file_viewer(app, p, session, directory, path) {
        return Some(el);
    }
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

/// The settings pages plugins add on host page `page` (`general`, `servers`), as sections of
/// that page (`PluginSettingsSections`).
pub fn settings_pages(app: &App, p: &Pal, page: &str) -> Vec<El> {
    sorted(app, kinds::SETTINGS_PAGE)
        .into_iter()
        .filter(|(_, c)| c.target == page && !page.is_empty())
        .filter_map(|(i, c)| {
            let body = draw(app, p, i, c, &Props::new(SURFACE_SETTINGS_PAGE, &c.id), false)?;
            let card = col().key(format!("plugin-page-{}", c.key)).w_full().px(20.0).py(16.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted)).child(body);
            Some(crate::settings::section(p, intern(&c.title), card))
        })
        // A Luau section draws its own title and list (`ui.settingsList`), as Desktop's do.
        .chain(super::luau::surfaces::settings_pages(app, Some(page)).into_iter().filter_map(|(i, key, spec, f)| {
            let body = super::luau::surfaces::settings_page_body(app, p, i, &spec, f.as_ref()?, None);
            Some(col().key(format!("plugin-page-{key}")).w_full().child(body))
        }))
        .collect()
}

/// The plugins' own Settings pages, in order, by key, each with whether it belongs beside
/// Experimental and About (`group: "about"`).
pub fn settings_page_ids(app: &App) -> Vec<(String, bool)> {
    sorted(app, kinds::SETTINGS_PAGE)
        .into_iter()
        .filter(|(_, c)| c.target.is_empty())
        .map(|(_, c)| (c.key.to_owned(), c.flags & opencode_plugin::abi::SETTINGS_PAGE_ABOUT != 0))
        .chain(super::luau::surfaces::settings_pages(app, None).into_iter().map(|(_, key, spec, _)| (key.to_owned(), spec.about == Some(true))))
        .collect()
}

fn own_page<'a>(app: &'a App, key: &str) -> Option<(usize, &'a ContribRec)> {
    active(&app.plugins, kinds::SETTINGS_PAGE).find(|(_, c)| c.target.is_empty() && (c.key == key || c.id == key))
}

/// The key of the plugin page a manifest's search entry names by its id.
pub fn settings_page_key(app: &App, id: &str) -> String {
    own_page(app, id).map_or_else(|| id.to_owned(), |(_, c)| c.key.to_owned())
}

pub fn settings_page_title(app: &App, key: &str) -> Option<String> {
    own_page(app, key).map(|(_, c)| c.title.clone()).or_else(|| luau_page(app, key).map(|(_, _, spec, _)| spec.title))
}

fn luau_page(app: &App, key: &str) -> Option<(usize, &'static str, opencode_plugin_spec::SettingsPageSpec, Option<mlua::Function>)> {
    super::luau::surfaces::settings_pages(app, None).into_iter().find(|(_, k, spec, _)| *k == key || spec.id == key)
}

/// The controls a Luau plugin's page draws at the end of its header (`renderActions`).
pub fn settings_page_actions(app: &App, p: &Pal, key: &str) -> Option<El> {
    let (i, _, spec, _) = luau_page(app, key)?;
    super::luau::surfaces::settings_page_actions(app, p, i, &spec)
}

/// A plugin page's rail icon: one it interned or a host icon by name.
pub fn settings_page_icon(app: &App, key: &str) -> Option<&'static rune::Icon> {
    let Some((i, c)) = own_page(app, key) else { return luau_page(app, key).and_then(|(_, _, spec, _)| spec.icon).map(|icon| super::icons::resolve("", &icon)) };
    (!c.icon.is_empty()).then(|| super::icons::resolve(&app.plugins.slots[i].manifest.id, &c.icon))
}

/// A plugin page: its title and what it draws, a failed render's card in its place.
pub fn settings_page(app: &App, p: &Pal, key: &str) -> Option<(String, String, El)> {
    let Some((i, c)) = own_page(app, key) else {
        let (i, _, spec, f) = luau_page(app, key)?;
        let body = super::luau::surfaces::settings_page_body(app, p, i, &spec, f.as_ref()?, None);
        return Some((spec.title.clone(), spec.description.clone().unwrap_or_default(), body));
    };
    let body = match render::surface(app, i, c.render.as_ref()?, &Props::new(SURFACE_SETTINGS_PAGE, &c.id), p) {
        Ok(el) => el.unwrap_or_else(rune::el),
        Err(e) => render::card(p, &app.plugins.slots[i].manifest.name, &format!("Stopped drawing: {e}"), Some(&app.plugins.slots[i].manifest.id), true),
    };
    Some((c.title.clone(), c.description.clone(), body))
}
