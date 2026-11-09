//! Luau contributions drawn into the app's surfaces: panels and their tab labels, shell slots,
//! titlebar items, composer pills, popovers, dialogs, Settings rows and pages, file viewers and
//! code blocks. Every render goes through [`render`], memoized as the module doc says.

use std::hash::{Hash, Hasher};

use mlua::Function;
use rune::{El, Tx, col, el, row, text};
use rune_luau::Data;
use session_view::palette::Elevate as _;

use super::cx;
use super::plugin::Contrib;
use super::services::map;
use crate::App;
use crate::palette::Pal;
use crate::panels::{PanelLauncherDesc, PanelSessionCtx, PanelTabDesc, PanelViewCtx};

thread_local! {
    /// A render logged an error since [`render`] cleared it.
    static FAILED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Notes that the render running now failed (the scripts' log hook calls it on an error).
pub fn note_error() {
    FAILED.set(true);
}

/// Renders `f(view, props)` of plugin `i` as surface `surface`, entering the VM only when the
/// script ran, the props, the theme or what the render read last time changed. A shell slot
/// whose render fails draws nothing, as the native host's do, and isn't run again until the
/// plugin reloads; the console has the error. Other surfaces show the error card with Reload.
pub fn render(app: &App, p: &Pal, i: usize, surface: &str, f: &Function, props: Data) -> El {
    let plugin = &app.plugins.luau.plugins[i];
    let Some(script) = plugin.script() else { return el() };
    let half = &plugin.half;
    let chrome = surface.starts_with("slot:");
    if chrome && half.failed.borrow().contains(surface) {
        return el();
    }
    FAILED.set(false);
    app.plugins.luau.dark.set(p.dark);
    let reads = half.reads.borrow().get(surface).cloned().unwrap_or_default();
    let mut h = std::hash::DefaultHasher::new();
    (script.version(), half.epoch.get(), super::epoch(), p.theme, p.generation, p.dark, cx::stamp(app, &reads)).hash(&mut h);
    format!("{props:?}").hash(&mut h);
    let version = h.finish();
    let before = script.metrics().renders;
    let props_for_bench = crate::flag("--luau-bench").then(|| props.clone());
    let id = half.id.clone();
    let (built, read) = cx::recording(|| cx::rendering_as(&id, || cx::with_app(app, || script.render_fn::<App>(surface, version, p.dark, f, props))));
    if chrome && FAILED.replace(false) {
        half.failed.borrow_mut().insert(surface.to_owned());
        return el();
    }
    if script.metrics().renders > before {
        bench(app, p, script, surface, f, &props_for_bench);
        half.reads.borrow_mut().insert(surface.to_owned(), read);
        app.plugins.luau.renders.set(app.plugins.luau.renders.get() + 1);
    } else {
        app.plugins.luau.hits.set(app.plugins.luau.hits.get() + 1);
    }
    built
}

/// `--luau-bench`: the first time a surface rebuilds, runs its render 500 more times, each a
/// memo miss, and prints the p50 and p99 time of one rebuild (script and `El` construction).
fn bench(app: &App, p: &Pal, script: &rune_luau::Script, surface: &str, f: &Function, props: &Option<Data>) {
    thread_local! {
        static DONE: std::cell::RefCell<std::collections::HashSet<String>> = std::cell::RefCell::default();
    }
    let Some(props) = props else { return };
    let key = surface.split(':').take(2).collect::<Vec<_>>().join(":");
    if !DONE.with_borrow_mut(|d| d.insert(key)) {
        return;
    }
    let mut times: Vec<f64> = (0..500u64)
        .map(|n| {
            let start = rune::time::Instant::now();
            let built = cx::with_app(app, || script.render_fn::<App>(&format!("bench:{surface}"), n ^ 0x5eed_0000, p.dark, f, props.clone()));
            let us = start.elapsed().as_secs_f64() * 1e6;
            drop(built);
            us
        })
        .collect();
    times.sort_by(f64::total_cmp);
    eprintln!("luau bench: {} {surface}: rebuild p50 {:.1} us p99 {:.1} us max {:.1} us (n 500)", script.name(), times[249], times[494], times[499]);
}

/// Each running plugin's contributions of one kind, with the plugin's index.
fn each<'a, S: 'a, R>(app: &'a App, pick: impl Fn(&super::plugin::Contribs) -> Vec<R> + 'a) -> Vec<(usize, R)> {
    let _ = std::marker::PhantomData::<S>;
    app.plugins.luau.running().flat_map(|(i, p)| pick(&p.half.contribs.borrow()).into_iter().map(move |r| (i, r))).collect()
}

/// What a render needs from a contribution: its key, a function and its spec's fields.
fn take<S: Clone>(list: &[Contrib<S>], f: &str) -> Vec<(&'static str, S, Option<Function>)> {
    list.iter().map(|c| (c.key, c.spec.clone(), c.f(f).cloned())).collect()
}

// ---- Panels ----------------------------------------------------------------------------------

/// The panel tabs Luau plugins offer the session: one per panel, the instance panels' open tabs
/// as `list` names them, and the console when it has opened.
pub fn panel_tabs(app: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let mut out = Vec::new();
    for (i, (key, spec, list)) in each::<(), _>(app, |c| take(&c.panels, "list")) {
        let icon = crate::plugin_host::icons::resolve("", spec.icon.as_deref().unwrap_or("window-analytics"));
        let Some(list) = list else {
            let mut tab = PanelTabDesc::new(key, spec.title.clone()).icon(icon);
            if spec.transient == Some(true) {
                tab = tab.transient();
            }
            if spec.first == Some(true) {
                tab = tab.first();
            }
            if spec.fallback == Some(true) {
                tab = tab.fallback();
            }
            out.push(tab);
            continue;
        };
        let prefix = format!("{key}/");
        let open: Vec<String> = ctx.open_ids.iter().filter_map(|id| id.strip_prefix(&prefix).map(str::to_owned)).collect();
        if open.is_empty() {
            continue;
        }
        let script = app.plugins.luau.plugins[i].script();
        let tabs: Option<Vec<opencode_plugin_spec::PanelTab>> =
            script.and_then(|s| cx::with_app(app, || s.eval_fn::<Data>(&list, (ctx.session_id.to_owned(), open.clone())).ok())).and_then(|d| d.decode().ok());
        for t in tabs.unwrap_or_default() {
            out.push(PanelTabDesc::new(format!("{prefix}{}", t.instance), t.title).icon(icon).transient());
        }
    }
    if ctx.open_ids.iter().any(|id| id == super::console::TAB) {
        out.push(PanelTabDesc::new(super::console::TAB, "Plugin console").icon(crate::plugin_host::icons::resolve("", "console")).transient());
    }
    out
}

/// A launcher card per Luau panel that wants one.
pub fn panel_launchers(app: &App, ctx: &PanelSessionCtx) -> Vec<PanelLauncherDesc> {
    each::<(), _>(app, |c| take(&c.panels, "list"))
        .into_iter()
        .filter(|(_, (_, spec, list))| list.is_none() && spec.launcher != Some(false))
        .map(|(i, (key, spec, _))| {
            let description = spec.description.clone().or_else(|| app.plugins.luau.plugins[i].manifest.description.clone());
            PanelLauncherDesc {
                id: key,
                title: spec.title.clone(),
                description,
                icon: crate::plugin_host::icons::resolve("", spec.icon.as_deref().unwrap_or("window-analytics")),
                shortcut: &[],
                order: spec.order.unwrap_or(0) as i32,
                opened: ctx.open_ids.iter().any(|o| o == key),
            }
        })
        .collect()
}

/// The narrow-window views Luau panels offer (`PanelSpec.mobile`), each with its tab's key in
/// the host's `plugin` panel.
pub fn narrow_views(app: &App) -> Vec<(String, opencode_plugin_spec::PanelMobileView)> {
    each::<(), _>(app, |c| c.panels.iter().filter_map(|c| Some((c.key, c.spec.mobile.clone()?))).collect())
        .into_iter()
        .map(|(_, (key, view))| (crate::panels::panel_key(crate::plugin_host::view::PANEL, key), view))
        .collect()
}

/// Splits `<plugin>.<panel>[/<instance>]` into the plugin index, the panel's render and label.
fn panel_of<'a>(app: &'a App, tab: &'a str) -> Option<(usize, opencode_plugin_spec::PanelSpec, Option<Function>, Option<Function>, &'a str)> {
    let (base, instance) = tab.split_once('/').unwrap_or((tab, ""));
    app.plugins.luau.running().find_map(|(i, p)| {
        let c = p.half.contribs.borrow();
        let c = c.panels.iter().find(|c| c.key == base)?;
        Some((i, c.spec.clone(), c.f("render").cloned(), c.f("label").cloned(), instance))
    })
}

/// Whether tab `tab` is a Luau panel's (or the console).
pub fn owns_tab(app: &App, tab: &str) -> bool {
    tab == super::console::TAB || panel_of(app, tab).is_some()
}

/// A Luau panel's content.
pub fn panel(app: &App, p: &Pal, ctx: &PanelViewCtx) -> Option<El> {
    if ctx.tab.id == super::console::TAB {
        return Some(super::console::view(app, p));
    }
    let (i, spec, f, _, instance) = panel_of(app, &ctx.tab.id)?;
    let f = f?;
    let props = map([
        ("sessionId", Data::String(ctx.session_id.to_owned())),
        ("directory", Data::String(ctx.directory.to_owned())),
        ("instance", if instance.is_empty() { Data::Nil } else { Data::String(instance.to_owned()) }),
        ("active", Data::Bool(ctx.visible)),
        ("placement", Data::String(if ctx.mobile { "mobile" } else { "side" }.into())),
        ("width", Data::Integer(ctx.pane_size.0.round() as i64)),
        ("height", Data::Integer(ctx.pane_size.1.round() as i64)),
    ]);
    let placement = if ctx.mobile { ":mobile" } else { "" };
    let body = render(app, p, i, &format!("panel:{}:{}:{instance}{placement}", spec.id, ctx.session_id), &f, props);
    let plugin = &app.plugins.luau.plugins[i].manifest.id;
    Some(col().key(format!("plugin-{plugin}-{}{}", spec.id, if instance.is_empty() { String::new() } else { format!("-{instance}") })).w_full().h_full().child(body))
}

/// A Luau panel's own tab label (`PanelSpec.label`).
pub fn tab_lead(app: &App, p: &Pal, tab: &PanelTabDesc, selected: bool) -> Option<El> {
    let (i, spec, _, label, instance) = panel_of(app, &tab.id)?;
    let label = label?;
    let session = app.active_session_id().unwrap_or_default();
    let props = map([
        ("sessionId", Data::String(session.to_owned())),
        ("instance", if instance.is_empty() { Data::Nil } else { Data::String(instance.to_owned()) }),
        ("active", Data::Bool(selected)),
        ("preview", Data::Bool(false)),
    ]);
    Some(render(app, p, i, &format!("label:{}:{session}:{instance}", spec.id), &label, props))
}

// ---- Slots, titlebar, composer -----------------------------------------------------------------

/// What Luau plugins put in shell slot `name` for session `session`, by `order`.
pub fn slot(app: &App, p: &Pal, name: &str, session: &str, active: bool, compact: bool) -> Vec<(i64, El)> {
    if name == "session.header" {
        app.plugins.luau.narrow.set(compact);
    }
    let directory = app.session(session).map_or("", |s| s.directory.as_str()).to_owned();
    each::<(), _>(app, |c| c.slots.iter().filter(|s| s.spec.at.as_str() == name).map(|s| (s.spec.clone(), s.f("render").cloned())).collect())
        .into_iter()
        .filter_map(|(i, (spec, f))| {
            let props = map([
                ("sessionId", Data::String(session.to_owned())),
                ("directory", Data::String(directory.clone())),
                ("active", Data::Bool(active)),
                ("compact", Data::Bool(compact)),
                ("shown", Data::Bool(true)),
            ]);
            Some((spec.order.unwrap_or(0), render(app, p, i, &format!("slot:{name}:{}:{session}", spec.id), &f?, props)))
        })
        .collect()
}

/// Titlebar items: each plugin's `show()` answer, drawn as the app's titlebar pill.
pub fn titlebar_items(app: &App, p: &Pal) -> Vec<El> {
    each::<(), _>(app, |c| c.titlebar_items.iter().map(|t| (t.key, t.f("show").cloned(), t.f("run").cloned())).collect())
        .into_iter()
        .filter_map(|(i, (key, show, run))| {
            let script = app.plugins.luau.plugins[i].script()?;
            let view: opencode_plugin_spec::TitlebarItemView = cx::with_app(app, || script.eval_fn::<Data>(show.as_ref()?, ()).ok())?.decode().ok()?;
            let mut pill = row().key(format!("plugin-titlebar-{key}")).role("button").label(view.title.clone().unwrap_or_else(|| view.label.clone())).h(24.0).px(8.0).gap(6.0).center_y().radius(6.0).hover(p.hover);
            if view.pressed == Some(true) {
                pill = pill.fill(p.pressed);
            }
            if let Some(icon) = &view.icon {
                pill = pill.child(rune::icon(crate::plugin_host::icons::resolve("", icon), 14.0).color(p.icon));
            }
            pill = pill.child(text(view.label).font(crate::view::UI).color(p.text));
            let Some(run) = run else { return Some(pill) };
            Some(pill.on_press_tx(move |s: &mut App, tx: &mut Tx| super::commands::spawn(s, i, &run, (), tx)))
        })
        .collect()
}

/// The pills Luau plugins put in the composer lane: their `show()` answer in the lane's capsule;
/// a pill with `renderMenu` opens it as a popover above, one without runs `run`.
pub fn pill_views(app: &App, p: &Pal, session: &str) -> Vec<El> {
    let directory = app.session(session).map_or("", |s| s.directory.as_str()).to_owned();
    each::<(), _>(app, |c| c.composer_pills.iter().map(|c| (c.key, c.spec.id.clone(), c.f("show").cloned(), c.f("renderMenu").is_some(), c.f("run").cloned())).collect())
        .into_iter()
        .filter_map(|(i, (key, id, show, menu, run))| {
            let script = app.plugins.luau.plugins[i].script()?;
            let props = map([("sessionId", Data::String(session.to_owned())), ("directory", Data::String(directory.clone()))]);
            let view: opencode_plugin_spec::ComposerPillView = cx::with_app(app, || script.eval_fn::<Data>(show.as_ref()?, props.clone()).ok())?.decode().ok()?;
            let anchor = format!("plugin-pill-{key}");
            let plugin = app.plugins.luau.plugins[i].half.id.clone();
            let open = app.plugins.luau.overlay.pill_open(&plugin, &id);
            let name = view.title.clone().unwrap_or_else(|| view.label.clone());
            let ink = match view.tone {
                Some(t) => session_view::palette::token(p.dark, &format!("state-fg-{}", t.as_str())).unwrap_or(p.icon_muted),
                None => p.icon_muted,
            };
            let mut pill = crate::subagents::chip(app, p, &anchor, name, open).shrink().min_w(0.0);
            if let Some(icon) = &view.icon {
                pill = pill.child(rune::icon(crate::plugin_host::icons::resolve("", icon), 14.0).color(ink));
            }
            pill = pill.child(text(view.label).font(crate::view::UI).color(p.text).truncate().max_w(260.0).shrink().min_w(0.0));
            Some(pill.on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if menu {
                    if s.plugins.luau.overlay.pill_open(&plugin, &id) {
                        return super::requests::close_popover(s, tx);
                    }
                    s.plugins.luau.overlay = super::Overlay::PillMenu { plugin: plugin.clone(), id: id.clone(), anchor: anchor.clone(), data: props.clone() };
                    crate::popover::motion::opening(&s.knobs, tx);
                } else if let Some(run) = &run {
                    super::commands::spawn(s, i, run, props.clone(), tx);
                }
            }))
        })
        .collect()
}

// ---- Popovers and dialogs ----------------------------------------------------------------------

/// The open Luau popover and the scrim under it that closes it.
pub fn popover_view(app: &App, p: &Pal) -> Vec<El> {
    let (plugin, id, data) = match &app.plugins.luau.overlay {
        super::Overlay::Closed => return Vec::new(),
        super::Overlay::Popover { plugin, id, data, .. } | super::Overlay::PillMenu { plugin, id, data, .. } => (plugin, id, data),
    };
    let Some((i, _)) = app.plugins.luau.running().find(|(_, pl)| pl.half.id == *plugin) else { return Vec::new() };
    let half = &app.plugins.luau.plugins[i].half;
    let scrim = crate::popover::scrim::<App>("plugin-popover-scrim", super::requests::close_popover);
    if let super::Overlay::PillMenu { anchor, .. } = &app.plugins.luau.overlay {
        let pill = half.contribs.borrow().composer_pills.iter().find(|c| c.spec.id == *id).map(|c| (c.key, c.f("renderMenu").cloned(), c.f("show").cloned()));
        let Some((key, Some(menu), show)) = pill else { return Vec::new() };
        let script = app.plugins.luau.plugins[i].script();
        let view: Option<opencode_plugin_spec::ComposerPillView> = script.and_then(|s| cx::with_app(app, || s.eval_fn::<Data>(show.as_ref()?, data.clone()).ok())).and_then(|d| d.decode().ok());
        let content = render(app, p, i, &format!("pill:{id}"), &menu, data.clone());
        let heading = view.map(|v| v.title.unwrap_or(v.label)).unwrap_or_default();
        let head = text(heading.clone()).font(rune::TextStyle::new(11.0, 530.0).lh(12.0).tracking(0.05)).color(p.faint).wrap().max_lines(3).w_full();
        let card = col()
            .key(format!("plugin-pill-popover-{key}"))
            .role("dialog")
            .label(heading)
            .w(320.0)
            .padding(4.0)
            .radius(10.0)
            .fill(p.layer1)
            .elevate(p.floating)
            .on_press(|_: &mut App| {})
            .child(col().w_full().px(8.0).pt(4.0).pb(6.0).child(head))
            .child(content);
        return vec![scrim, crate::popover::place(&app.knobs, card, anchor, rune::Side::Top, rune::Align::Start, 8.0)];
    }
    let super::Overlay::Popover { anchor, side, align, gap, .. } = &app.plugins.luau.overlay else { return Vec::new() };
    let found = half.contribs.borrow().popovers.iter().find(|c| c.spec.id == *id).map(|c| (c.key, c.f("render").cloned()));
    let Some((key, Some(f))) = found else { return Vec::new() };
    let content = render(app, p, i, &format!("popover:{id}"), &f, map([("data", data.clone())]));
    let content = col().key(format!("plugin-popover-{key}")).child(content);
    vec![scrim, crate::popover::place(&app.knobs, content, anchor, *side, *align, *gap)]
}

/// The open Luau dialog over a scrim, as the app's own dialogs sit.
pub fn dialog_view(app: &App, p: &Pal) -> Vec<El> {
    let Some(open) = &app.plugins.luau.dialog else { return Vec::new() };
    let Some((i, _)) = app.plugins.luau.running().find(|(_, pl)| pl.half.id == open.plugin) else { return Vec::new() };
    let found = app.plugins.luau.plugins[i].half.contribs.borrow().dialogs.iter().find(|c| c.spec.id == open.id).map(|c| (c.key, c.spec.size, c.f("render").cloned()));
    let Some((key, size, Some(f))) = found else { return Vec::new() };
    let width = match size {
        Some(opencode_plugin_spec::DialogSize::Small) => 400.0,
        Some(opencode_plugin_spec::DialogSize::Large) => 720.0,
        _ => 520.0,
    };
    let content = render(app, p, i, &format!("dialog:{}", open.id), &f, map([("data", open.data.clone())]));
    let card = col().key(format!("plugin-dialog-{key}")).w(width).radius(12.0).fill(p.bg_base).elevate(p.overlay).on_press(|_: &mut App| {}).child(content);
    crate::widgets::modal(&format!("plugin-dialog-modal-{key}"), p, card, None)
}

// ---- Settings ----------------------------------------------------------------------------------

/// Settings › Plugins rows a plugin added (`AddSetting`), as `(key, title, description, control)`.
pub fn setting_rows(app: &App, p: &Pal, i: usize) -> Vec<(String, String, String, El)> {
    let plugin = &app.plugins.luau.plugins[i];
    let rows: Vec<_> = plugin.half.contribs.borrow().settings.iter().map(|c| (c.spec.clone(), c.f("render").cloned())).collect();
    rows.into_iter()
        .map(|(spec, f)| {
            let control = f.map_or_else(el, |f| render(app, p, i, &format!("setting:{}", spec.id), &f, Data::Nil));
            (format!("plugin-{}-{}", plugin.manifest.id, spec.id), spec.title, spec.description.unwrap_or_default(), control)
        })
        .collect()
}

/// The settings pages Luau plugins add: on host page `page` as sections, or as pages of their
/// own (`page` None), by key.
pub fn settings_pages(app: &App, page: Option<&str>) -> Vec<(usize, &'static str, opencode_plugin_spec::SettingsPageSpec, Option<Function>)> {
    each::<(), _>(app, |c| c.settings_pages.iter().map(|s| (s.key, s.spec.clone(), s.f("render").cloned())).collect())
        .into_iter()
        .filter(|(_, (_, spec, _))| spec.page.map(|p| p.as_str()) == page)
        .map(|(i, (key, spec, f))| (i, key, spec, f))
        .collect()
}

pub fn settings_page_body(app: &App, p: &Pal, i: usize, spec: &opencode_plugin_spec::SettingsPageSpec, f: &Function, target: Option<&str>) -> El {
    render(app, p, i, &format!("page:{}", spec.id), f, map([("target", target.map_or(Data::Nil, |t| Data::String(t.to_owned())))]))
}

// ---- Files and code blocks ---------------------------------------------------------------------

/// A Luau plugin's view of file `path`, when a file viewer takes its extension.
pub fn file_viewer(app: &App, p: &Pal, session: &str, directory: &str, path: &str) -> Option<El> {
    let ext = std::path::Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
    let (i, (key, spec, f)) = each::<(), _>(app, |c| take(&c.file_viewers, "render")).into_iter().find(|(_, (_, s, _))| s.extensions.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)))?;
    let abs = if std::path::Path::new(path).is_absolute() { path.to_owned() } else { format!("{}/{path}", directory.trim_end_matches(['/', '\\'])) };
    let props = map([("path", Data::String(abs.clone())), ("sessionId", Data::String(session.to_owned())), ("directory", Data::String(directory.to_owned()))]);
    let body = render(app, p, i, &format!("file:{}:{abs}", spec.id), &f?, props);
    Some(col().key(format!("plugin-file-{key}")).w_full().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(body))
}
