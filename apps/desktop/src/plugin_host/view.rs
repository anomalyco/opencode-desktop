//! Where plugins show up in the existing UI: their panels as tabs of the host's `plugin` panel
//! in the session side region, and Settings › Plugins (each plugin's state, Reload, its switch,
//! and the settings rows it registered).

use std::cell::RefCell;
use std::ffi::c_void;

use opencode_plugin::abi::{AbiPalette, AbiStr, RenderProps, SURFACE_PANEL, SURFACE_SETTING};
use rune::{Color, El, Tx, col, row, text};

use super::arena::Build;
use super::instance::RenderEnv;
use super::lifecycle::PluginState;
use super::vtable::{Kind, RenderCtx};
use super::{PluginHost, Slot, intern};
use crate::App;
use crate::palette::Pal;
use crate::panels::{PanelLauncherDesc, PanelRegistration, PanelSessionCtx, PanelTabDesc, PanelViewCtx, SessionPane, icon_by_name, panel_key};
use crate::view::{UI, UI_MED};

/// The host panel every plugin panel is a tab of: `plugin:<plugin>.<panel>`.
pub const PANEL: &str = "plugin";

rune_register::register!(crate::panels::PANELS: PanelRegistration = [
    PanelRegistration::new(PANEL, list, render).order(40).launcher(launcher, open_first).poll(super::poll),
]);

fn panels(host: &PluginHost) -> impl Iterator<Item = (usize, &super::instance::PanelRec)> {
    host.slots.iter().enumerate().filter_map(|(i, s)| s.active().map(|inst| (i, inst))).flat_map(|(i, inst)| inst.contributions.panels.iter().map(move |p| (i, p)))
}

fn list(app: &App, _ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let running = panels(&app.plugins).map(|(_, p)| PanelTabDesc::new(p.tab.clone(), p.title.clone()).icon(icon_by_name(Some(&p.icon))));
    let failed = app.plugins.slots.iter().filter(|s| s.state == PluginState::Failed).flat_map(|s| &s.tabs);
    running.chain(failed.map(|(tab, title, icon)| PanelTabDesc::new(tab.clone(), title.clone()).icon(icon_by_name(Some(icon))))).collect()
}

fn first_panel(host: &PluginHost) -> Option<(usize, &super::instance::PanelRec)> {
    panels(host).min_by_key(|(_, p)| p.order)
}

fn launcher(app: &App, ctx: &PanelSessionCtx) -> Option<PanelLauncherDesc> {
    let (i, p) = first_panel(&app.plugins)?;
    let description = app.plugins.slots[i].manifest.description.clone();
    Some(PanelLauncherDesc {
        id: PANEL,
        title: p.title.clone(),
        description: (!description.is_empty()).then_some(description),
        icon: icon_by_name(Some(&p.icon)),
        shortcut: &[],
        order: p.order,
        opened: ctx.open_ids.contains(&p.tab),
    })
}

fn open_first(app: &mut App, pane: SessionPane, tx: &mut Tx) {
    let Some(tab) = first_panel(&app.plugins).map(|(_, p)| p.tab.clone()) else { return };
    app.open_side_tab(&panel_key(PANEL, &tab), Some(pane), false, false, tx);
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
    let props = Props { surface: SURFACE_PANEL, id: panel, session: ctx.session_id, directory: ctx.directory, size: ctx.pane_size, visible: ctx.visible, focused: ctx.focused };
    let body = match render_surface(app, i, &rec.render, &props, pal) {
        Ok(el) => el,
        Err(message) => card(pal, &slot.manifest.name, &format!("Stopped drawing: {message}"), Some(&slot.manifest.id), true),
    };
    col().key(format!("plugin-{plugin}-{panel}")).w_full().h_full().child(body)
}

struct Props<'a> {
    surface: u32,
    id: &'a str,
    session: &'a str,
    directory: &'a str,
    size: (f32, f32),
    visible: bool,
    focused: bool,
}

pub fn abi_palette(p: &Pal) -> AbiPalette {
    let c = |c: Color| [c.r, c.g, c.b, c.a];
    AbiPalette {
        dark: u32::from(p.dark),
        theme: p.theme as u32,
        bg_base: c(p.bg_base),
        layer1: c(p.layer1),
        layer2: c(p.layer2),
        layer3: c(p.layer3),
        text: c(p.text),
        muted: c(p.muted),
        faint: c(p.faint),
        accent: c(p.accent),
        bg_accent: c(p.bg_accent),
        on_fill: c(p.on_fill),
        icon: c(p.icon),
        icon_muted: c(p.icon_muted),
        border: c(p.border),
        border_muted: c(p.border_muted),
        hover: c(p.hover),
        pressed: c(p.pressed),
        success: c(p.success),
        warning: c(p.warning),
        danger: c(p.danger),
        switch_thumb: c(p.switch_thumb),
    }
}

/// Runs a plugin render into the arena and takes the tree out. A panic rolls the arena back,
/// records `render_error`, salts the plugin's memos so nothing half-built is reused, and
/// returns the message; the plugin stays `Active`.
fn render_surface(app: &App, i: usize, render: &RenderEnv, props: &Props, pal: &Pal) -> Result<El, String> {
    let host = &app.plugins;
    let slot = &host.slots[i];
    let (Some(inst), Some(func)) = (slot.active(), render.cb.func) else { return Err("not running".to_owned()) };
    let mark = host.arena.borrow().mark();
    let pal_abi = abi_palette(pal);
    let salt = mix(&[u64::from(inst.generation), slot.salt.get(), u64::from(pal_abi.theme), u64::from(pal_abi.dark)]);
    let ctx = RenderCtx {
        kind: Kind::Render,
        pal: pal_abi,
        build: Build { editors: &host.editors, faint: pal.faint, slot: i, generation: inst.generation, lib: &inst.lib },
        arena: &host.arena,
        prefix: &slot.prefix,
        salt,
        panicked: RefCell::new(None),
    };
    let abi = RenderProps {
        surface: props.surface,
        id: AbiStr::new(props.id),
        session_id: AbiStr::new(props.session),
        directory: AbiStr::new(props.directory),
        width: props.size.0,
        height: props.size.1,
        visible: u8::from(props.visible),
        focused: u8::from(props.focused),
        dark: u8::from(pal.dark),
        _pad: 0,
    };
    // SAFETY: the plugin's render on its own state, with a render context alive for the call.
    let out = unsafe { func(inst.state, (&raw const ctx).cast_mut().cast::<c_void>(), render.cb.data, &abi) };
    let panicked = ctx.panicked.borrow_mut().take();
    let result = match (out.into_result(), panicked) {
        (Ok((_, h)), None) => Ok(host.arena.borrow_mut().take(h)),
        (Err((_, message)), _) | (Ok(_), Some(message)) => Err(message),
    };
    host.arena.borrow_mut().rollback(mark);
    match &result {
        Ok(_) => {
            if slot.render_error.borrow().is_some() {
                *slot.render_error.borrow_mut() = None;
            }
        }
        Err(message) => {
            eprintln!("plugin {}: render panicked: {message}", slot.manifest.id);
            slot.salt.set(slot.salt.get() + 1);
            *slot.render_error.borrow_mut() = Some(message.clone());
        }
    }
    result
}

fn mix(parts: &[u64]) -> u64 {
    parts.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, &v| (h.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95))
}

/// A panel's stand-in while its plugin isn't drawing: why, and Reload when that can help.
fn card(p: &Pal, name: &str, why: &str, reload_id: Option<&str>, failed: bool) -> El {
    col()
        .key("plugin-card")
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(12.0)
        .child(text(name.to_owned()).font(UI_MED).color(p.text))
        .child(text(why.to_owned()).key("plugin-card-why").font(UI).color(if failed { p.danger } else { p.muted }).wrap().selectable(true))
        .children(reload_id.map(|id| row().child(reload_button(p, id))))
}

fn reload_button(p: &Pal, id: &str) -> El {
    let owned = id.to_owned();
    row()
        .key(format!("plugin-reload-{id}"))
        .role("button")
        .label("Reload")
        .h(28.0)
        .px(10.0)
        .center()
        .radius(6.0)
        .border(0.5, p.border)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |app: &mut App, tx: &mut Tx| super::reload(app, &owned, tx))
        .child(text("Reload").font(UI_MED).color(p.text))
}

/// Settings › Plugins: every discovered plugin with its state, then each running plugin's own
/// rows. Nothing when no plugin is installed.
pub fn settings_section(s: &App, p: &Pal) -> Option<El> {
    let host = &s.plugins;
    if host.slots.is_empty() {
        return None;
    }
    let titles: Vec<String> = host.slots.iter().map(|slot| format!("{} {}", slot.manifest.name, slot.manifest.version).trim().to_owned()).collect();
    let rows = host.slots.iter().zip(&titles).map(|(slot, title)| {
        let id = slot.manifest.id.clone();
        let key = format!("plugin-{id}");
        (key, title.as_str(), status(slot), controls(p, slot, id))
    });
    let list = crate::settings::settings_card(p, "plugins-list", rows);
    let own = host.slots.iter().enumerate().filter_map(|(i, slot)| plugin_rows(s, p, i, slot));
    Some(col().key("plugins-settings").w_full().gap(32.0).child(crate::settings::section(p, "Plugins", list)).children(own))
}

fn status(slot: &Slot) -> String {
    let mut parts = vec![format!("{} · {}", slot.tier.label(), slot.state.label())];
    if !slot.manifest.description.is_empty() {
        parts.push(slot.manifest.description.clone());
    }
    let errors = [slot.error.clone(), slot.render_error.borrow().clone().map(|e| format!("Stopped drawing: {e}")), slot.reload_error.clone().map(|e| format!("Reload failed: {e}"))];
    parts.extend(errors.into_iter().flatten());
    if !slot.dir.as_os_str().is_empty() {
        parts.push(slot.dir.display().to_string());
    }
    parts.join("\n")
}

fn controls(p: &Pal, slot: &Slot, id: String) -> El {
    let switchable = matches!(slot.state, PluginState::Active | PluginState::Disabled | PluginState::Failed);
    let reloadable = matches!(slot.state, PluginState::Active | PluginState::Failed);
    let on = slot.state == PluginState::Active;
    let toggle = switchable.then(|| {
        let id = id.clone();
        crate::settings::switch(p, &format!("plugin-switch-{id}"), on).on_press_tx(move |app: &mut App, tx: &mut Tx| super::toggle_enabled(app, &id, tx))
    });
    row().gap(12.0).center_y().children(reloadable.then(|| reload_button(p, &id))).children(toggle)
}

/// The settings rows plugin `slot` registered, under its name.
fn plugin_rows(s: &App, p: &Pal, i: usize, slot: &Slot) -> Option<El> {
    let inst = slot.active()?;
    if inst.contributions.settings.is_empty() {
        return None;
    }
    let rows = inst.contributions.settings.iter().map(|rec| {
        let props = Props { surface: SURFACE_SETTING, id: &rec.id, session: "", directory: "", size: (0.0, 0.0), visible: true, focused: false };
        let control = render_surface(s, i, &rec.render, &props, p).unwrap_or_else(|e| text(format!("Error: {e}")).font(UI).color(p.danger));
        (format!("plugin-{}-{}", slot.manifest.id, rec.id), rec.title.as_str(), rec.description.as_str(), control)
    });
    let card = crate::settings::settings_card(p, &format!("plugin-{}-settings", slot.manifest.id), rows);
    Some(crate::settings::section(p, intern(&slot.manifest.name), card))
}
