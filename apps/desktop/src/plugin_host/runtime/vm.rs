//! What every plugin VM gets: the `@opencode/plugin` and `@opencode/ui` modules, the app's
//! palette, elevations and type, its host nodes, and the hooks that start a half's setup and
//! put the app within the bindings' reach around every entry from the UI.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use mlua::{Function, Lua, Table};
use rune::{Color, TextStyle, Tx};
use rune_luau::{Host, LogLevel, ScriptOptions};

use super::plugin::{Contribs, Half, Kind};
use crate::App;
use crate::plugin_host::{bridge, console, cx, requests, services, surfaces, ui};

/// One VM's view of its half, held by every binding it has.
pub struct Vmx {
    pub half: Rc<Half>,
    /// The VM's id, once its entry module ran (`ScriptOptions::start`).
    pub vm: Cell<u64>,
    /// `require("@rune/signal").new`.
    pub script_signal: Function,
    /// The host's async functions, by name.
    pub asyncs: Table,
    warned: RefCell<HashSet<String>>,
}

impl Vmx {
    /// Runs `f` on the contributions this VM is adding to: its setup's, or the running ones.
    pub fn with_contribs<R>(&self, f: impl FnOnce(&mut Contribs) -> R) -> R {
        let mut made = self.half.made.borrow_mut();
        match made.as_mut() {
            Some((vm, c)) if *vm == self.vm.get() => f(c),
            _ => f(&mut self.half.contribs.borrow_mut()),
        }
    }

    /// Writes `message` to the console once per VM.
    pub fn warn(&self, message: &str) {
        if self.warned.borrow_mut().insert(message.to_owned()) {
            console::push(&self.half.id, LogLevel::Warn, message);
        }
    }
}

/// The options every plugin half's script runs with.
pub fn options(half: Rc<Half>) -> ScriptOptions {
    let kind = half.kind;
    let log_half = half.clone();
    let mut options = ScriptOptions::new()
        .palette(Rc::new(palette))
        .shadows(Rc::new(shadows))
        .text(TextStyle::new(13.0, 440.0), [440.0, 530.0, 600.0, 700.0])
        .setup(move |host| setup(host, &half))
        .start(move |lua, module, vm| start(lua, module, vm, kind))
        .around(around)
        .around_read(around_read)
        .on_log(move |level, line| {
            if level == LogLevel::Error {
                surfaces::note_error();
            }
            console::push(&log_half.id, level, line)
        });
    for node in ui::nodes::all() {
        options = options.node(node);
    }
    options
}

fn setup(host: &Host<'_>, half: &Rc<Half>) -> mlua::Result<()> {
    let lua = host.lua();
    let hot = opencode_plugin_runtime::contribs::register_modules(host)?;
    let asyncs = lua.create_table()?;
    services::asyncs(host, &asyncs, half)?;
    let vmx = Rc::new(Vmx { half: half.clone(), vm: Cell::new(0), script_signal: host.script_signal()?, asyncs, warned: RefCell::default() });
    lua.set_named_registry_value("opencode.hot", hot)?;
    lua.set_app_data(vmx);
    Ok(())
}

/// Runs the half's setup with its `ctx`, collecting its contributions for VM `vm`.
fn start(lua: &Lua, module: &Table, vm: u64, kind: Kind) -> mlua::Result<()> {
    let vmx = lua.app_data_ref::<Rc<Vmx>>().map(|v| v.clone()).ok_or_else(|| mlua::Error::runtime("the plugin host isn't set up"))?;
    vmx.vm.set(vm);
    opencode_plugin_runtime::contribs::verify_module_half(module, kind)?;
    let fire: Function = lua.load("return function(signal, ...) signal:Fire(...) end").call(())?;
    vmx.half.fire.borrow_mut().insert(vm, fire);
    *vmx.half.made.borrow_mut() = Some((vm, Contribs::default()));
    vmx.half.editors_made.set(0);
    let hot: Table = lua.named_registry_value("opencode.hot")?;
    let ctx = bridge::ctx::build(lua, &vmx, &hot)?;
    let setup: Function = module.raw_get("__setup")?;
    setup.call::<()>(ctx)
}

/// Every press, hover or visible-rows entry: the app within reach, then what the plugin asked
/// for, carried out in the same transaction.
fn around(state: &mut dyn Any, mut tx: Option<&mut Tx>, run: &mut dyn FnMut(Option<&mut Tx>)) {
    let Some(app) = state.downcast_mut::<App>() else { return run(tx) };
    cx::enter(app, || run(tx.as_deref_mut()));
    match tx {
        Some(tx) => requests::carry_out(app, tx),
        None => app.plugins.asked.set(true),
    }
}

fn around_read(state: &dyn Any, run: &mut dyn FnMut()) {
    match state.downcast_ref::<App>() {
        Some(app) => cx::enter(app, run),
        None => run(),
    }
}

/// The contract's semantic tokens, as this app paints them (and `text`, `selected`, the names
/// rune-luau itself asks for: the default text and a selected row's fill). The text scale is
/// Desktop's: `text-strong` .. `text-weaker` its `text-*` tokens (what plugin views such as
/// the Context tab draw with), `text-muted` and `text-faint` its chrome's `text-text-*`.
pub fn palette(name: &str, dark: bool) -> Option<Color> {
    let p = crate::palette::pal(dark);
    let desktop = |token: &str, fallback: Color| session_view::palette::token(dark, token).unwrap_or(fallback);
    Some(match name {
        "bg-base" => p.bg_base,
        "bg-weak" => p.bg_deep,
        "bg-strong" => p.layer2,
        "surface-1" => p.layer1,
        "surface-2" => p.layer2,
        "surface-3" => p.layer3,
        "surface-4" => p.q.layer4,
        "surface-hover" => p.hover,
        "surface-press" => p.pressed,
        "surface-selected" | "selected" => p.row_active,
        "text-strong" | "text" => desktop("text-strong", p.text),
        "text-base" => desktop("text-base", p.muted),
        "text-weak" => desktop("text-weak", p.faint),
        "text-weaker" => desktop("text-weaker", p.icon_faint),
        "text-chrome" => p.text,
        "text-muted" => p.muted,
        "text-faint" => p.faint,
        "text-on-accent" | "icon-on-accent" => p.on_fill,
        "text-interactive" => p.accent,
        "icon-strong" => p.text,
        "icon-base" => p.icon,
        "icon-weak" => p.icon_muted,
        "icon-weaker" => p.icon_faint,
        "border-strong" => p.q.border_strong,
        "border-base" => p.border,
        "border-weak" => p.border_muted,
        "border-focus" => p.v2.border_focus,
        "accent" => p.bg_accent,
        "accent-hover" => p.v2.icon_accent_hover,
        "accent-subtle" => p.selection,
        "status-success" => p.success,
        "status-warning" => p.warning,
        "status-danger" => p.danger,
        "status-info" => p.info,
        "avatar-orange" => p.avatars[0].0,
        "avatar-yellow" => p.avatars[1].0,
        "avatar-cyan" => p.avatars[2].0,
        "avatar-green" => p.avatars[3].0,
        "avatar-red" => p.avatars[4].0,
        "avatar-pink" => p.avatars[5].0,
        "avatar-blue" => p.avatars[6].0,
        "avatar-purple" => p.avatars[7].0,
        "avatar-text" => p.avatar_fg,
        // `site-icon`'s inset ring: `alpha-dark-12` in both schemes.
        "avatar-ring" => desktop("alpha-dark-12", p.border),
        _ => return None,
    })
}

fn shadows(name: &str, dark: bool) -> Option<Vec<rune::Shadow>> {
    let p = crate::palette::pal(dark);
    Some(match name {
        "raised" => p.raised.to_vec(),
        "floating" => p.floating.to_vec(),
        "overlay" => p.overlay.to_vec(),
        _ => return None,
    })
}
