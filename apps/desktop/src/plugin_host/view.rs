//! Where plugins show up in the side region and Settings: each plugin panel as its own tab of
//! the host's `plugin` panel with its own New tab launcher card (and `+` menu row), the
//! `session.panel` menu items beside them, and Settings › Plugins (each plugin's state, trust,
//! Reload, its switch, the contracts it provides, and the settings rows it registered).

use opencode_plugin::abi::{
    PANEL_FALLBACK, PANEL_FIRST, PANEL_INSTANCES, PANEL_NO_LAUNCHER, RENDER_ACTIVE, RENDER_SELECTED, SURFACE_PANEL, SURFACE_SETTING, SURFACE_TAB, contrib as kinds,
};
use rune::{El, Tx, col, row, text};

use super::icons::{intern, resolve};
use super::lifecycle::PluginState;
use super::render::{self, Props, card};
use super::shared::Live;
use super::{PluginHost, Slot};
use crate::App;
use crate::palette::Pal;
use crate::panels::{PanelLauncherDesc, PanelRegistration, PanelSessionCtx, PanelTabDesc, PanelViewCtx, SessionPane, icon_by_name, panel_key};
use crate::view::UI;

/// The host panel every plugin panel is a tab of: `plugin:<plugin>.<panel>`.
pub const PANEL: &str = "plugin";

rune_register::register!(crate::panels::PANELS: PanelRegistration = [
    PanelRegistration::new(PANEL, list, render).order(40).launchers(launchers, open_entry).poll(super::run::poll).tab_lead(tab_lead).on_close(closed),
]);

fn panels(host: &PluginHost) -> impl Iterator<Item = (usize, &super::instance::PanelRec)> {
    host.slots.iter().enumerate().filter_map(|(i, s)| s.active().map(|inst| (i, inst))).flat_map(|(i, inst)| inst.contributions.panels.iter().map(move |p| (i, p)))
}

fn list(app: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let host = &app.plugins;
    let instances = panels(host).filter(|(_, p)| p.flags & PANEL_INSTANCES != 0).flat_map(|(i, p)| instance_tabs(app, i, p, ctx));
    let running = panels(host).filter(|(_, p)| p.flags & PANEL_INSTANCES == 0).map(|(i, p)| {
        let tab = PanelTabDesc::new(p.tab, p.title.clone()).icon(resolve(&host.slots[i].manifest.id, &p.icon));
        let tab = if p.flags & PANEL_FIRST != 0 { tab.first() } else { tab };
        if p.flags & PANEL_FALLBACK != 0 { tab.fallback() } else { tab }
    });
    let failed = host.slots.iter().filter(|s| s.state == PluginState::Failed).flat_map(|s| &s.tabs);
    running.chain(instances).chain(failed.map(|(tab, title, icon)| PanelTabDesc::new(tab.clone(), title.clone()).icon(icon_by_name(Some(icon))))).collect()
}

/// The tabs of instance panel `p` the session has open (`<plugin>.<panel>/<instance>`), as the
/// plugin lists them: in its order, with its titles; a held one hidden.
fn instance_tabs(app: &App, i: usize, p: &super::instance::PanelRec, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let prefix = format!("{}/", p.tab);
    let open: Vec<&str> = ctx.open_ids.iter().filter_map(|id| id.strip_prefix(&prefix)).collect();
    if open.is_empty() {
        return Vec::new();
    }
    let Some(list) = p.list.as_ref() else { return Vec::new() };
    let input = std::iter::once(ctx.session_id).chain(open.iter().copied()).collect::<Vec<_>>().join("\n");
    let answer = super::run::peek(app, i, &list.cb, &input).unwrap_or_default();
    let icon = resolve(&app.plugins.slots[i].manifest.id, &p.icon);
    answer
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let (id, title, flags) = (f.next()?, f.next().unwrap_or_default(), f.next().unwrap_or_default());
            let tab = PanelTabDesc::new(format!("{prefix}{id}"), title).icon(icon).transient();
            Some(PanelTabDesc { hidden: flags.contains('h'), ..tab })
        })
        .collect()
}

/// Splits a plugin tab id into the panel's record and the instance it shows ("" for none).
fn panel_of<'a>(host: &'a PluginHost, tab: &'a str) -> Option<(usize, &'a super::instance::PanelRec, &'a str)> {
    let (base, instance) = tab.split_once('/').unwrap_or((tab, ""));
    panels(host).find(|(_, p)| p.tab == base).map(|(i, p)| (i, p, instance))
}

/// An instance panel's tab closed: its plugin forgets the instance.
fn closed(app: &mut App, session: &str, tab: &str, tx: &mut Tx) {
    let Some((i, cb, instance)) = panel_of(&app.plugins, tab).and_then(|(i, p, inst)| Some((i, p.close.as_ref()?.cb, inst.to_owned()))) else { return };
    if instance.is_empty() {
        return;
    }
    super::run::action(app, tx, i, super::instance::Scope::Window, cb, opencode_plugin::abi::EVENT_TAB_CLOSED, &instance, session);
}

/// Opens instance `instance` of slot `i`'s panel `panel` in the routed session, selected.
pub fn open_instance(app: &mut App, i: usize, panel: &str, instance: &str, tx: &mut Tx) {
    if app.active_session_id().is_none() || instance.is_empty() {
        return;
    }
    let plugin = &app.plugins.slots[i].manifest.id;
    let key = panel_key(PANEL, &format!("{plugin}.{panel}/{instance}"));
    app.open_side_tab(&key, None, false, false, tx);
}

/// A launcher card per plugin panel, and one per `session.panel` menu item.
fn launchers(app: &App, ctx: &PanelSessionCtx) -> Vec<PanelLauncherDesc> {
    let host = &app.plugins;
    let panels = panels(host).filter(|(_, p)| p.flags & (PANEL_NO_LAUNCHER | PANEL_INSTANCES) == 0).map(|(i, p)| {
        let slot = &host.slots[i];
        let description = if p.description.is_empty() { slot.manifest.description.clone() } else { p.description.clone() };
        PanelLauncherDesc {
            id: p.tab,
            title: p.title.clone(),
            description: (!description.is_empty()).then_some(description),
            icon: resolve(&slot.manifest.id, &p.icon),
            shortcut: &[],
            order: p.order,
            opened: ctx.open_ids.iter().any(|o| o == p.tab),
        }
    });
    let items = super::contrib::active(host, kinds::MENU_ITEM).filter(|(_, c)| c.target == "session.panel").map(|(i, c)| PanelLauncherDesc {
        id: intern(&format!("menu:{}", c.key)),
        title: c.title.clone(),
        description: (!c.description.is_empty()).then(|| c.description.clone()),
        icon: resolve(&host.slots[i].manifest.id, &c.icon),
        shortcut: &[],
        order: c.order,
        opened: false,
    });
    panels.chain(items).collect()
}

/// A plugin panel's own tab label (`PanelSpec` `label`), drawn for the routed session.
fn tab_lead(app: &App, pal: &Pal, tab: &PanelTabDesc, selected: bool) -> Option<El> {
    let host = &app.plugins;
    let (i, rec, instance) = panel_of(host, &tab.id)?;
    let label = rec.label.as_ref()?;
    let session = app.active_session_id().unwrap_or_default();
    let directory = app.session(session).map_or("", |s| s.directory.as_str());
    let flags = RENDER_ACTIVE | if selected { RENDER_SELECTED } else { 0 };
    let props = Props { session, directory, flags, path: instance, ..Props::new(SURFACE_TAB, &rec.id) };
    render::surface(app, i, label, &props, pal).ok().flatten()
}

fn open_entry(app: &mut App, id: &str, pane: SessionPane, tx: &mut Tx) {
    match id.strip_prefix("menu:") {
        Some(key) => super::contrib::run_menu_item(app, key, "", tx),
        None => app.open_side_tab(&panel_key(PANEL, id), Some(pane), false, false, tx),
    }
}

fn render(app: &App, pal: &Pal, ctx: &PanelViewCtx) -> El {
    let host = &app.plugins;
    let (base, instance) = ctx.tab.id.split_once('/').unwrap_or((&ctx.tab.id, ""));
    let (plugin, panel) = base.split_once('.').unwrap_or((base, ""));
    let Some(i) = host.slots.iter().position(|s| s.manifest.id == plugin) else {
        return card(pal, plugin, "This plugin isn't installed any more.", None, false);
    };
    let slot = &host.slots[i];
    let Some(rec) = slot.active().and_then(|inst| inst.contributions.panels.iter().find(|p| p.id == panel)) else {
        let why = slot.error.as_deref().unwrap_or(slot.state.label());
        return card(pal, &slot.manifest.name, why, Some(&slot.manifest.id), slot.state == PluginState::Failed);
    };
    let props = Props {
        session: ctx.session_id,
        directory: ctx.directory,
        size: ctx.pane_size,
        visible: ctx.visible,
        focused: ctx.focused,
        path: instance,
        ..Props::new(SURFACE_PANEL, panel)
    };
    let body = match render::surface(app, i, &rec.render, &props, pal) {
        Ok(el) => el.unwrap_or_else(rune::el),
        Err(message) => card(pal, &slot.manifest.name, &format!("Stopped drawing: {message}"), Some(&slot.manifest.id), true),
    };
    col().key(if instance.is_empty() { format!("plugin-{plugin}-{panel}") } else { format!("plugin-{plugin}-{panel}-{instance}") }).w_full().h_full().child(body)
}

/// Settings › Plugins: every discovered plugin with its state, then each running plugin's own
/// rows. Nothing when no plugin is installed.
pub fn settings_section(s: &App, p: &Pal) -> Option<El> {
    let host = &s.plugins;
    if host.slots.is_empty() {
        return None;
    }
    let titles: Vec<String> = host.slots.iter().map(|slot| format!("{} {}", slot.manifest.name, slot.manifest.version).trim().to_owned()).collect();
    let rows = host.slots.iter().enumerate().zip(&titles).map(|((i, slot), title)| {
        let id = slot.manifest.id.clone();
        let key = format!("plugin-{id}");
        (key, title.as_str(), status(host, i, slot), controls(p, slot, id))
    });
    let list = crate::settings::settings_card(p, "plugins-list", rows);
    let own = host.slots.iter().enumerate().filter_map(|(i, slot)| plugin_rows(s, p, i, slot));
    Some(col().key("plugins-settings").w_full().gap(32.0).child(crate::settings::section(p, "Plugins", list)).children(own))
}

fn status(host: &PluginHost, i: usize, slot: &Slot) -> String {
    let mut parts = vec![format!("{} · {}", slot.tier.label(), slot.state.label())];
    if !slot.manifest.description.is_empty() {
        parts.push(slot.manifest.description.clone());
    }
    let provided = host.shared.contracts.summary().filter(|(_, _, _, owner)| *owner == Some(i)).map(|(id, state, generation, _)| match state {
        Live::Active => format!("Provides {id} (generation {generation})"),
        _ => format!("Provides {id} ({state:?})"),
    });
    parts.extend(provided);
    if slot.blocked && slot.state == PluginState::PendingTrust {
        parts.push("Windows blocked this file because it came from another computer.".to_owned());
    }
    let errors = [slot.error.clone(), slot.render_error.borrow().clone().map(|e| format!("Stopped drawing: {e}")), slot.reload_error.clone().map(|e| format!("Reload failed: {e}"))];
    parts.extend(errors.into_iter().flatten());
    if !slot.dir.as_os_str().is_empty() {
        parts.push(slot.dir.display().to_string());
    }
    parts.join("\n")
}

fn controls(p: &Pal, slot: &Slot, id: String) -> El {
    let switchable = matches!(slot.state, PluginState::Active | PluginState::Disabled | PluginState::Failed | PluginState::PendingTrust);
    let reloadable = matches!(slot.state, PluginState::Active | PluginState::Failed);
    let on = slot.state == PluginState::Active;
    let toggle = switchable.then(|| {
        let id = id.clone();
        crate::settings::switch(p, &format!("plugin-switch-{id}"), on).on_press_tx(move |app: &mut App, tx: &mut Tx| super::swap::toggle_enabled(app, &id, tx))
    });
    let reload = reloadable.then(|| {
        let id = id.clone();
        render::button(p, &format!("plugin-reload-{id}"), "Reload", move |app: &mut App, tx: &mut Tx| super::swap::reload(app, &id, tx))
    });
    row().gap(12.0).center_y().children(reload).children(toggle)
}

/// The settings rows plugin `slot` registered, under its name.
fn plugin_rows(s: &App, p: &Pal, i: usize, slot: &Slot) -> Option<El> {
    let inst = slot.active()?;
    if inst.contributions.settings.is_empty() {
        return None;
    }
    let rows = inst.contributions.settings.iter().map(|rec| {
        let control = match render::surface(s, i, &rec.render, &Props::new(SURFACE_SETTING, &rec.id), p) {
            Ok(el) => el.unwrap_or_else(rune::el),
            Err(e) => text(format!("Error: {e}")).font(UI).color(p.danger),
        };
        (format!("plugin-{}-{}", slot.manifest.id, rec.id), rec.title.as_str(), rec.description.as_str(), control)
    });
    let card = crate::settings::settings_card(p, &format!("plugin-{}-settings", slot.manifest.id), rows);
    Some(crate::settings::section(p, intern(&slot.manifest.name), card))
}
