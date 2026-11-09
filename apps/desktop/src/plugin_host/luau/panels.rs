//! Luau plugins' panels in a session's side region. Each running plugin that adds panels is a
//! provider of its own (`App::all_panels`), keyed as Desktop keys a plugin's tabs: a panel
//! without `list` is one tab, `<plugin>:<panel>`; an instance panel's tabs are
//! `<plugin>:<instance>`, as its `list` names them, and share one mounted render.
//!
//! Its `session.panel` menu items are its entries in the `+` menu and the New tab view.

use mlua::Function;
use opencode_plugin_spec::{PanelPane, PanelSpec, PanelTab};
use rune::{El, Tx, col};
use rune_luau::Data;

use super::cx;
use super::services::map;
use super::surfaces::render as render_surface;
use crate::App;
use crate::palette::Pal;
use crate::panels::{ClosedPanelTab, ClosedTabInfo, PanelLauncherDesc, PanelRegistration, PanelSessionCtx, PanelTabDesc, PanelViewCtx, SessionPane};
use crate::plugin_host::icons::{intern, resolve};

/// One panel of a plugin: its spec and the functions it handed over.
struct Panel {
    spec: PanelSpec,
    list: Option<Function>,
    render: Option<Function>,
    label: Option<Function>,
    on_focus: Option<Function>,
    on_close: Option<Function>,
    is_wide: Option<Function>,
}

/// A provider per running plugin with panels or `session.panel` menu items, ordered by its
/// panels' lowest `order`.
pub fn providers(app: &App) -> Vec<PanelRegistration> {
    app.plugins
        .luau
        .running()
        .filter_map(|(_, p)| {
            let c = p.half.contribs.borrow();
            let items = c.menu_items.iter().any(|m| m.spec.menu == opencode_plugin_spec::MenuName::SessionPanel);
            if c.panels.is_empty() && !items {
                return None;
            }
            let order = c.panels.iter().filter_map(|c| c.spec.order).min().unwrap_or(40) as i32;
            Some(PanelRegistration::luau(intern(&p.manifest.id), order, pane(c.panels.first().and_then(|f| f.spec.pane))))
        })
        .collect()
}

fn pane(p: Option<PanelPane>) -> SessionPane {
    match p {
        Some(PanelPane::Bottom) => SessionPane::Bottom,
        _ => SessionPane::Side,
    }
}

/// The running plugin `plugin` and its panels.
fn panels(app: &App, plugin: &str) -> Option<(usize, Vec<Panel>)> {
    let (i, p) = app.plugins.luau.running().find(|(_, p)| p.manifest.id == plugin)?;
    let c = p.half.contribs.borrow();
    let all = c
        .panels
        .iter()
        .map(|c| Panel {
            spec: c.spec.clone(),
            list: c.f("list").cloned(),
            render: c.f("render").cloned(),
            label: c.f("label").cloned(),
            on_focus: c.f("onFocus").cloned(),
            on_close: c.f("onClose").cloned(),
            is_wide: c.f("isWide").cloned(),
        })
        .collect();
    Some((i, all))
}

/// The panel that draws tab `tab`: the panel of that id, else the instance panel.
fn owner(all: &[Panel], tab: &str) -> Option<usize> {
    all.iter().position(|p| p.list.is_none() && p.spec.id == tab).or_else(|| all.iter().position(|p| p.list.is_some()))
}

fn icon_of(spec: &PanelSpec) -> &'static rune::Icon {
    resolve("", spec.icon.as_deref().unwrap_or("window-analytics"))
}

/// The plugin's tabs for the routed session: one per single panel, and the instance panels'
/// open tabs as their `list` names them.
pub fn list(app: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let Some((i, all)) = panels(app, ctx.plugin) else { return Vec::new() };
    let mut out = Vec::new();
    for p in &all {
        let Some(list) = &p.list else {
            let mut tab = PanelTabDesc::new(p.spec.id.clone(), p.spec.title.clone()).icon(icon_of(&p.spec));
            if p.spec.transient == Some(true) {
                tab = tab.transient();
            }
            if p.spec.first == Some(true) {
                tab = tab.first();
            }
            if p.spec.fallback == Some(true) {
                tab = tab.fallback();
            }
            out.push(tab);
            continue;
        };
        let open: Vec<String> = ctx.open_ids.iter().filter(|id| !all.iter().any(|q| q.list.is_none() && q.spec.id == **id)).cloned().collect();
        if open.is_empty() {
            continue;
        }
        let script = app.plugins.luau.plugins[i].script();
        let tabs: Vec<PanelTab> = script.and_then(|s| cx::with_app(app, || s.eval_fn::<Data>(list, (ctx.session_id.to_owned(), open.clone())).ok())).and_then(|d| d.decode().ok()).unwrap_or_default();
        let group = intern(&p.spec.id);
        out.extend(tabs.into_iter().filter(|t| open.contains(&t.instance)).map(|t| instance_tab(&p.spec, group, t)));
    }
    out
}

fn instance_tab(spec: &PanelSpec, group: &'static str, t: PanelTab) -> PanelTabDesc {
    let icon = match (&t.icon, &t.file) {
        (Some(name), _) => resolve("", name),
        (None, Some(file)) => rune_theme_opencode::file_icons::file(opencode_kit::path::basename(file)),
        (None, None) => icon_of(spec),
    };
    let mut tab = PanelTabDesc::new(t.instance, t.title).icon(icon).group(group).missing(t.missing == Some(true));
    if let Some(file) = t.file {
        tab = tab.file(file);
    }
    if t.pinned == Some(true) {
        tab = tab.pinned();
    }
    if t.transient == Some(true) {
        tab = tab.transient();
    }
    if t.draggable == Some(false) {
        tab = tab.undraggable();
    }
    if t.fallback == Some(true) {
        tab = tab.fallback();
    }
    tab
}

/// What a panel's functions receive about tab `tab` of `session`.
fn props(app: &App, all: &[Panel], k: usize, session: &str, tab: &str, active: bool, placement: &str, size: (f32, f32)) -> Data {
    let directory = app.session(session).map_or("", |s| s.directory.as_str()).to_owned();
    let instance = if all[k].list.is_some() { Data::String(tab.to_owned()) } else { Data::Nil };
    map([
        ("sessionId", Data::String(session.to_owned())),
        ("directory", Data::String(directory)),
        ("instance", instance),
        ("active", Data::Bool(active)),
        ("placement", Data::String(placement.into())),
        ("width", Data::Integer(size.0.round() as i64)),
        ("height", Data::Integer(size.1.round() as i64)),
    ])
}

/// A tab's content.
pub fn render(app: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
    let Some((i, all)) = panels(app, ctx.plugin) else { return rune::el() };
    let Some(k) = owner(&all, &ctx.tab.id) else { return rune::el() };
    let Some(f) = &all[k].render else { return rune::el() };
    let placement = if ctx.mobile { "mobile" } else { "side" };
    let data = props(app, &all, k, ctx.session_id, &ctx.tab.id, ctx.visible, placement, ctx.pane_size);
    let instance = if all[k].list.is_some() { ctx.tab.id.as_str() } else { "" };
    let suffix = if ctx.mobile { ":mobile" } else { "" };
    let spec = &all[k].spec;
    let body = render_surface(app, p, i, &format!("panel:{}:{}:{instance}{suffix}", spec.id, ctx.session_id), f, data);
    let key = if instance.is_empty() { format!("plugin-{}-{}", ctx.plugin, spec.id) } else { format!("plugin-{}-{}-{instance}", ctx.plugin, spec.id) };
    col().key(key).w_full().h_full().child(body)
}

/// A tab's own label in the strip (`PanelSpec.label`).
pub fn tab_lead(app: &App, p: &Pal, plugin: &str, tab: &PanelTabDesc, selected: bool) -> Option<El> {
    let (i, all) = panels(app, plugin)?;
    let k = owner(&all, &tab.id)?;
    let label = all[k].label.as_ref()?;
    let session = app.active_session_id().unwrap_or_default();
    let instance = if all[k].list.is_some() { Data::String(tab.id.clone()) } else { Data::Nil };
    let data = map([("sessionId", Data::String(session.to_owned())), ("instance", instance), ("active", Data::Bool(selected)), ("preview", Data::Bool(false))]);
    Some(render_surface(app, p, i, &format!("label:{}:{session}:{}", all[k].spec.id, tab.id), label, data))
}

/// The plugin's `+` and New tab entries: each single panel that wants a launcher, then its
/// `session.panel` menu items (`menu:<key>`).
pub fn launchers(app: &App, ctx: &PanelSessionCtx) -> Vec<PanelLauncherDesc> {
    let Some((i, all)) = panels(app, ctx.plugin) else { return Vec::new() };
    let manifest = &app.plugins.luau.plugins[i].manifest;
    let own = all.iter().filter(|p| p.list.is_none() && p.spec.launcher != Some(false)).map(|p| {
        let description = p.spec.description.clone().or_else(|| manifest.description.clone());
        PanelLauncherDesc::new(intern(&p.spec.id), p.spec.title.clone(), icon_of(&p.spec), p.spec.order.unwrap_or(0) as i32)
            .description(description)
            .opened(ctx.open_ids.iter().any(|o| *o == p.spec.id))
    });
    let mut out: Vec<PanelLauncherDesc> = own.collect();
    out.extend(super::commands::panel_menu_items(app, i, ctx.session_id));
    out
}

/// Opens entry `id` of plugin `plugin`: runs its menu item, or opens its panel's tab in `pane`.
pub fn open_entry(app: &mut App, plugin: &str, id: &str, pane: SessionPane, tx: &mut Tx) {
    match id.strip_prefix("menu:") {
        Some(key) => {
            let session = app.active_session_id().unwrap_or_default().to_owned();
            super::commands::run_menu_item(app, key, &session, tx);
        }
        None => app.open_side_tab(&crate::panels::panel_key(plugin, id), Some(pane), false, false, tx),
    }
}

/// Tab `tab` of `session` became selected, or the side region opened onto it: its panel's
/// `onFocus` runs at the next poll.
pub fn focused(app: &mut App, plugin: &str, session: &str, tab: &str) {
    let Some((i, all)) = panels(app, plugin) else { return };
    let Some(k) = owner(&all, tab) else { return };
    let Some(f) = &all[k].on_focus else { return };
    let data = props(app, &all, k, session, tab, true, "side", (0.0, 0.0));
    if let Some(script) = app.plugins.luau.plugins[i].script() {
        cx::with_app(app, || script.spawn_fn(f, data, None).ok());
    }
    app.plugins.luau.asked = true;
}

/// Tab `tab` left `session`'s strip: its panel's `onClose` runs.
pub fn closed(app: &mut App, plugin: &str, session: &str, tab: &str, tx: &mut Tx) {
    let Some((i, all)) = panels(app, plugin) else { return };
    let Some(k) = owner(&all, tab) else { return };
    let Some(f) = all[k].on_close.clone() else { return };
    let data = props(app, &all, k, session, tab, false, "side", (0.0, 0.0));
    super::commands::spawn(app, i, &f, data, tx);
}

/// What Recently closed keeps of `tab`: its panel's `closed`, else the tab's own title.
pub fn closed_info(app: &App, plugin: &str, _session: &str, tab: &PanelTabDesc) -> ClosedTabInfo {
    let Some((_, all)) = panels(app, plugin) else { return ClosedTabInfo::Default };
    match owner(&all, &tab.id).and_then(|k| all[k].spec.closed.clone()) {
        Some(c) => ClosedTabInfo::Allow(ClosedPanelTab { title: c.title, icon_name: c.icon, ..Default::default() }),
        None => ClosedTabInfo::Default,
    }
}

/// Whether one of the plugin's panels asks for the wider session minimum now.
pub fn wide(app: &App, plugin: &str) -> bool {
    let Some((i, all)) = panels(app, plugin) else { return false };
    all.iter().any(|p| match &p.is_wide {
        Some(f) => app.plugins.luau.plugins[i].script().is_some_and(|s| cx::with_app(app, || s.eval_fn::<bool>(f, ())).unwrap_or(false)),
        None => p.spec.wide == Some(true),
    })
}

/// The pane the plugin's first panel names for a new tab.
pub fn home_pane(app: &App, plugin: &str) -> Option<SessionPane> {
    let (_, all) = panels(app, plugin)?;
    Some(pane(all.first()?.spec.pane))
}

/// The narrow-window views the plugins' panels offer (`PanelSpec.mobile`), each keyed by its
/// panel's tab key, `<plugin>:<panel>`.
pub fn narrow_views(app: &App) -> Vec<(String, opencode_plugin_spec::PanelMobileView)> {
    app.plugins
        .luau
        .running()
        .flat_map(|(_, p)| p.half.contribs.borrow().panels.iter().filter_map(|c| Some((crate::panels::panel_key(&p.manifest.id, &c.spec.id), c.spec.mobile.clone()?))).collect::<Vec<_>>())
        .collect()
}

/// The tab key of plugin `plugin`'s panel `panel`, or of its instance `instance`.
pub fn tab_key(plugin: &str, panel: &str, instance: Option<&str>) -> String {
    match instance {
        Some(i) if !i.is_empty() => crate::panels::panel_key(plugin, i),
        _ => crate::panels::panel_key(plugin, panel),
    }
}
