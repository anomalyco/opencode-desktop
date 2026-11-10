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
    // A render inside another plugin's (a file viewer under the Files panel's `ui.codeView`)
    // hands what it reads, and its own plugin's state, to the render around it, which otherwise
    // keeps the view it built until its own reads move.
    half.reads.borrow().get(surface).into_iter().flatten().for_each(|dep| cx::read(dep.clone()));
    cx::read(cx::Dep::Plugin(id));
    built
}

/// `--luau-bench`: the first three times a surface rebuilds (the first often runs before its
/// data arrives), runs its render 1,000 more times and prints the p50 and p99 time of one
/// rebuild (script and `El` construction): `rebuild`, every memo missing; `warm`, as a following
/// frame sees it, the surface missing and the script's own `view:Memo`s holding what didn't change.
fn bench(app: &App, p: &Pal, script: &rune_luau::Script, surface: &str, f: &Function, props: &Option<Data>) {
    thread_local! {
        static DONE: std::cell::RefCell<std::collections::HashMap<String, u32>> = std::cell::RefCell::default();
    }
    let Some(props) = props else { return };
    let key = surface.split(':').take(2).collect::<Vec<_>>().join(":");
    let nth = DONE.with_borrow_mut(|d| {
        let n = d.entry(key).or_default();
        *n += 1;
        *n
    });
    if nth > 3 {
        return;
    }
    let id = format!("bench:{surface}");
    for (name, seed, warm) in [("rebuild", 0x5eed_0000u64, false), ("warm", 0x3eed_0000, true)] {
        let mut times: Vec<f64> = (0..1000u64)
            .map(|n| {
                let start = rune::time::Instant::now();
                let run = || cx::with_app(app, || script.render_fn::<App>(&id, n ^ seed, p.dark, f, props.clone()));
                let built = if warm { rune::memo::as_next_build(run) } else { run() };
                let us = start.elapsed().as_secs_f64() * 1e6;
                drop(built);
                us
            })
            .collect();
        times.sort_by(f64::total_cmp);
        eprintln!("luau bench: {} {surface}: {name}#{nth} p50 {:.1} us p99 {:.1} us max {:.1} us (n 1000)", script.name(), times[499], times[989], times[999]);
    }
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

// ---- Slots, titlebar, composer -----------------------------------------------------------------

/// What Luau plugins put in shell slot `name` for session `session`, by `order`.
pub fn slot(app: &App, p: &Pal, name: &str, session: &str, active: bool, compact: bool) -> Vec<(i64, El)> {
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

/// Desktop's default titlebar item icon: a download arrow (`titlebar-items.tsx`).
static DOWNLOAD: rune::Icon = rune::Icon { name: "titlebar-download", view: 14.0, body: r#"<path d="M7 11V3M3.5 7.63128L7 11L10.5 7.63128" stroke="currentColor" fill="none"/>"# };

/// Titlebar items: each plugin's `show()` answer as Desktop's `TitlebarItemButton`: a round
/// `bg-deep` pill holding its icon in `icon-accent`, which grows on hover to show its label
/// (11/16 at 530, `text-accent`) as the label fades and slides in over 150 ms. In the titlebar
/// it is 20 across with the label before the icon, 12 before the titlebar's end; in the
/// sidebar's footer (`vertical`) 28 across with the label after it.
pub fn titlebar_items(app: &App, p: &Pal, vertical: bool) -> Vec<El> {
    let ease = rune::Curve::tween(0.15, rune::Easing::cubic(0.0, 0.0, 0.58, 1.0));
    each::<(), _>(app, |c| c.titlebar_items.iter().map(|t| (t.key, t.f("show").cloned(), t.f("run").cloned())).collect())
        .into_iter()
        .filter_map(|(i, (key, show, run))| {
            let script = app.plugins.luau.plugins[i].script()?;
            let view: opencode_plugin_spec::TitlebarItemView = cx::with_app(app, || script.eval_fn::<Data>(show.as_ref()?, ()).ok())?.decode().ok()?;
            let busy = view.busy == Some(true);
            let open = app.plugins.luau.titlebar_hover == Some(key);
            let side = if vertical { 28.0 } else { 20.0 };
            let glyph = if busy {
                rune_kit::Loader::new().size(12.0).build(crate::palette::kit(p))
            } else {
                let icon = view.icon.as_deref().map_or(&DOWNLOAD, |name| crate::plugin_host::icons::resolve("", name));
                rune::icon(icon, 14.0).color(session_view::palette::token(p.dark, "icon-icon-accent").unwrap_or(p.accent))
            };
            let glyph = row().size(side, side).center().child(glyph);
            let label = text(view.label.clone())
                .font(rune::TextStyle::new(11.0, 530.0).lh(16.0))
                .color(session_view::palette::token(p.dark, "text-text-accent").unwrap_or(p.accent))
                .opacity(if open { 1.0 } else { 0.0 })
                .opacity_motion(ease)
                .offset(if open { 0.0 } else if vertical { -8.0 } else { 8.0 }, 0.0)
                .offset_motion(ease);
            let label = if vertical { label.ml(1.0).mr(16.0) } else { label.ml(8.0).mr(1.0) };
            let inner = if vertical { row().child(glyph).child(label) } else { row().justify_end().child(label).child(glyph) };
            let mut pill = inner
                .key(format!("plugin-titlebar-{key}"))
                .role("button")
                .label(view.title.clone().unwrap_or_else(|| view.label.clone()))
                .h(side)
                .min_w(side)
                .center_y()
                .radius(side / 2.0)
                .fill(p.bg_deep)
                .clip_rounded(side / 2.0)
                .no_window_drag()
                .on_hover_change(move |s: &mut App, inside: bool, tx: &mut Tx| {
                    let now = inside.then_some(key);
                    if s.plugins.luau.titlebar_hover != now && (inside || s.plugins.luau.titlebar_hover == Some(key)) {
                        s.plugins.luau.titlebar_hover = now;
                        tx.instant();
                    }
                });
            if !open {
                pill = pill.w(side);
            }
            if !vertical {
                pill = pill.mr(12.0);
            }
            if busy {
                return Some(pill.layer_opacity(0.6).disabled(true));
            }
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
            // `w-80 p-1` inside a 0.5 transparent border (`[data-composer-pill-panel]`).
            .padding(4.5)
            .radius(10.0)
            .fill(p.layer1)
            .elevate(p.floating)
            .on_press(|_: &mut App| {})
            .child(col().w_full().px(8.0).pt(4.0).pb(6.0).child(head))
            .child(content);
        return vec![scrim, crate::popover::place(&app.knobs, card, anchor, rune::Side::Top, rune::Align::Start, 8.0)];
    }
    let super::Overlay::Popover { anchor, side, align, gap, flyout, .. } = &app.plugins.luau.overlay else { return Vec::new() };
    let found = half.contribs.borrow().popovers.iter().find(|c| c.spec.id == *id).map(|c| (c.key, c.f("render").cloned()));
    let Some((key, Some(f))) = found else { return Vec::new() };
    let content = render(app, p, i, &format!("popover:{id}"), &f, map([("data", data.clone())]));
    let content = col().key(format!("plugin-popover-{key}")).child(content);
    let mut out = vec![scrim, crate::popover::place(&app.knobs, content, anchor, *side, *align, *gap)];
    out.extend(flyout_view(app, p, i, flyout));
    out
}

/// The list a popover row opened, hung beside its row (drawn after the popover that holds the
/// row): its top 2 above the row's, so its first row lines up with it across the list's 2 inset
/// (`session-service-flyout`, `translateY(row - 2)`), 2 out from the row, which sits 2 inside
/// the card, so it meets the card's edge.
fn flyout_view(app: &App, p: &Pal, i: usize, flyout: &super::Flyout) -> Option<El> {
    let list = flyout.list()?;
    let half = &app.plugins.luau.plugins[i].half;
    let (key, f) = half.contribs.borrow().popovers.iter().find(|c| c.spec.id == list.id).map(|c| (c.key, c.f("render").cloned()))?;
    let content = render(app, p, i, &format!("flyout:{}", list.id), &f?, map([("data", list.data.clone())]));
    // An anchored element keeps its anchor's top whatever its margin, so the list moves up inside it.
    let body = col()
        .mt(-2.0)
        // Where the list is, for the rows' safe triangle; drawing doesn't depend on it.
        .on_layout_tx(|s: &mut App, rect, tx: &mut Tx| {
            s.plugins.luau.aim.list.set(Some(rect));
            tx.live_only();
        })
        // The pointer reached the list: the aim ends (`aim.cancelled`).
        .on_hover_change(|s: &mut App, inside: bool, tx: &mut Tx| {
            let aiming = matches!(&s.plugins.luau.overlay, super::Overlay::Popover { flyout: super::Flyout::Aiming { .. }, .. });
            if !(inside && aiming) {
                return tx.live_only();
            }
            super::requests::step_flyout(s, None, super::FlyoutEvent::Settled, tx);
        })
        .child(content);
    Some(col().key(format!("plugin-flyout-{key}")).anchored(list.row.clone(), list.side, rune::Align::Start, 2.0).on_press(|_: &mut App| {}).child(body))
}

/// The open Luau dialog over a scrim, as the app's own dialogs sit: in Desktop's dialog frame
/// (`dialog.css`, `<Dialog fit>`: as tall as its content). Esc or a press outside closes it and
/// runs its `onDismiss`.
pub fn dialog_view(app: &App, p: &Pal) -> Vec<El> {
    let Some(open) = &app.plugins.luau.dialog else { return Vec::new() };
    let Some((i, _)) = app.plugins.luau.running().find(|(_, pl)| pl.half.id == open.plugin) else { return Vec::new() };
    let found = app.plugins.luau.plugins[i].half.contribs.borrow().dialogs.iter().find(|c| c.spec.id == open.id).map(|c| (c.key, c.spec.clone(), c.f("render").cloned()));
    let Some((key, spec, Some(f))) = found else { return Vec::new() };
    let width = match spec.size {
        Some(opencode_plugin_spec::DialogSize::Small) => 400.0,
        Some(opencode_plugin_spec::DialogSize::Large) => 640.0,
        _ => 480.0,
    };
    let content = render(app, p, i, &format!("dialog:{}", open.id), &f, map([("data", open.data.clone())]));
    let card = col().w(width).radius(6.0).fill(p.layer1).elevate(p.overlay).clip_rounded(6.0).child(content);
    let card = col().key(format!("plugin-dialog-{key}")).role("dialog").label(spec.title.clone().unwrap_or_default()).modal(true, |s: &mut App| super::requests::dismiss_dialog(s, &mut Tx::default())).child(card);
    crate::widgets::modal(&format!("plugin-dialog-modal-{key}"), p, card, None)
}

/// A Luau server source's entry `rec` (key `key`), as its window half's `row` draws it in
/// Settings › Servers, with its latest health check.
pub fn server_row(app: &App, p: &Pal, key: &str, rec: &crate::plugin_host::instance::ServerRec, health: Option<String>) -> Option<El> {
    let plugin = key.split_once(':')?.0;
    let (i, f) = app.plugins.luau.running().find_map(|(i, pl)| {
        (pl.manifest.id == plugin).then(|| pl.half.contribs.borrow().server_sources.iter().find(|c| c.spec.id == rec.source).and_then(|c| c.f("row").cloned())).flatten().map(|f| (i, f))
    })?;
    let health = health.map_or(Data::Nil, |v| map([("healthy", Data::Bool(true)), ("version", Data::String(v))]));
    let props = map([("id", Data::String(rec.id.clone())), ("key", Data::String(key.to_owned())), ("health", health)]);
    Some(render(app, p, i, &format!("server:{key}"), &f, props))
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

/// What page `spec` of plugin `i` draws at the end of its header, when it draws anything.
pub fn settings_page_actions(app: &App, p: &Pal, i: usize, spec: &opencode_plugin_spec::SettingsPageSpec) -> Option<El> {
    let f = app.plugins.luau.plugins[i].half.contribs.borrow().settings_pages.iter().find(|c| c.spec.id == spec.id).and_then(|c| c.f("renderActions").cloned())?;
    Some(render(app, p, i, &format!("page-actions:{}", spec.id), &f, Data::Nil))
}

pub fn settings_page_body(app: &App, p: &Pal, i: usize, spec: &opencode_plugin_spec::SettingsPageSpec, f: &Function, target: Option<&str>) -> El {
    render(app, p, i, &format!("page:{}", spec.id), f, map([("target", target.map_or(Data::Nil, |t| Data::String(t.to_owned())))]))
}

// ---- Files and code blocks ---------------------------------------------------------------------

thread_local! {
    /// The host's own view of the code block a plugin renders now, for `ui.genUi` to place.
    static NATIVE: std::cell::RefCell<Option<El>> = const { std::cell::RefCell::new(None) };
}

/// Takes the host's view of the block being rendered (`ui.genUi`); None outside a code block.
pub fn take_native() -> Option<El> {
    NATIVE.take()
}

/// The running plugin and code block that render ```` ```language ```` blocks.
fn code_block_of(app: &App, language: &str) -> Option<(usize, opencode_plugin_spec::CodeBlockSpec, Option<Function>)> {
    each::<(), _>(app, |c| c.code_blocks.iter().map(|c| (c.spec.clone(), c.f("render").cloned())).collect())
        .into_iter()
        .find(|(_, (spec, _))| spec.languages.iter().any(|l| l.eq_ignore_ascii_case(language)))
        .map(|(i, (spec, f))| (i, spec, f))
}

pub fn code_block_lang(app: &App, language: &str) -> bool {
    code_block_of(app, language).is_some()
}

/// A plugin's view of one ```` ```language ```` block (`CodeBlock.render`), `native` the host's
/// own view of it, which `ui.genUi` places; `version` moves whenever `native` does.
#[allow(clippy::too_many_arguments)]
pub fn code_block(app: &App, p: &Pal, language: &str, key: &str, code: &str, streaming: bool, version: u64, native: El) -> Option<El> {
    let Some((i, spec, Some(f))) = code_block_of(app, language) else { return Some(native) };
    let props = map([
        ("code", Data::String(code.to_owned())),
        ("language", Data::String(language.to_ascii_lowercase())),
        ("sessionId", app.active_session_id().map_or(Data::Nil, |s| Data::String(s.to_owned()))),
        ("streaming", Data::Bool(streaming)),
        // Not the contract's: what the host's own view draws from, so a memo of it moves too.
        ("revision", Data::Integer(version as i64)),
    ]);
    NATIVE.set(Some(native));
    let built = render(app, p, i, &format!("code:{}:{key}", spec.id), &f, props);
    NATIVE.take();
    Some(built)
}

/// A Luau plugin's view of file `path`, when a file viewer takes its extension.
pub fn file_viewer(app: &App, p: &Pal, session: &str, directory: &str, path: &str) -> Option<El> {
    let ext = std::path::Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
    let (i, (key, spec, f)) = each::<(), _>(app, |c| take(&c.file_viewers, "render")).into_iter().find(|(_, (_, s, _))| s.extensions.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)))?;
    let abs = if std::path::Path::new(path).is_absolute() { path.to_owned() } else { format!("{}/{path}", directory.trim_end_matches(['/', '\\'])) };
    let props = map([("path", Data::String(abs.clone())), ("sessionId", Data::String(session.to_owned())), ("directory", Data::String(directory.to_owned()))]);
    let body = render(app, p, i, &format!("file:{}:{abs}", spec.id), &f?, props);
    Some(col().key(format!("plugin-file-{key}")).w_full().grow(1.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(body))
}
