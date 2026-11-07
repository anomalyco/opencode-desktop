//! Where plugins show up in the side region and Settings: each plugin panel as its own tab of
//! the host's `plugin` panel with its own New tab launcher card (and `+` menu row), the
//! `session.panel` menu items beside them, and Settings › Plugins (each plugin's state, trust,
//! Reload, its switch, the contracts it provides, and the settings rows it registered).

use opencode_plugin::abi::{SURFACE_PANEL, SURFACE_SETTING, contrib as kinds};
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
    PanelRegistration::new(PANEL, list, render).order(40).launchers(launchers, open_entry).poll(super::run::poll),
]);

fn panels(host: &PluginHost) -> impl Iterator<Item = (usize, &super::instance::PanelRec)> {
    host.slots.iter().enumerate().filter_map(|(i, s)| s.active().map(|inst| (i, inst))).flat_map(|(i, inst)| inst.contributions.panels.iter().map(move |p| (i, p)))
}

fn list(app: &App, _ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let host = &app.plugins;
    let running = panels(host).map(|(i, p)| PanelTabDesc::new(p.tab, p.title.clone()).icon(resolve(&host.slots[i].manifest.id, &p.icon)));
    let failed = host.slots.iter().filter(|s| s.state == PluginState::Failed).flat_map(|s| &s.tabs);
    running.chain(failed.map(|(tab, title, icon)| PanelTabDesc::new(tab.clone(), title.clone()).icon(icon_by_name(Some(icon))))).collect()
}

/// A launcher card per plugin panel, and one per `session.panel` menu item.
fn launchers(app: &App, ctx: &PanelSessionCtx) -> Vec<PanelLauncherDesc> {
    let host = &app.plugins;
    let panels = panels(host).map(|(i, p)| {
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

fn open_entry(app: &mut App, id: &str, pane: SessionPane, tx: &mut Tx) {
    match id.strip_prefix("menu:") {
        Some(key) => super::contrib::run_menu_item(app, key, "", tx),
        None => app.open_side_tab(&panel_key(PANEL, id), Some(pane), false, false, tx),
    }
}

fn render(app: &App, pal: &Pal, ctx: &PanelViewCtx) -> El {
    let host = &app.plugins;
    let (plugin, panel) = ctx.tab.id.split_once('.').unwrap_or((&ctx.tab.id, ""));
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
        ..Props::new(SURFACE_PANEL, panel)
    };
    let body = match render::surface(app, i, &rec.render, &props, pal) {
        Ok(el) => el.unwrap_or_else(rune::el),
        Err(message) => card(pal, &slot.manifest.name, &format!("Stopped drawing: {message}"), Some(&slot.manifest.id), true),
    };
    col().key(format!("plugin-{plugin}-{panel}")).w_full().h_full().child(body)
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
