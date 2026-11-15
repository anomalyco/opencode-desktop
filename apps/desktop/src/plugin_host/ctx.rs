//! The `ctx` a plugin half's setup receives: `GetService`, `CreateSignal`, `CreateEditor`,
//! `BindHotState`, the `Add*` contribution methods of its half, and its `Disposing` signal.
//! Each `Add*` decodes its spec with `Data::decode` into the contract's Rust record and keeps the
//! spec's functions beside it.

use std::collections::BTreeMap;
use std::rc::Rc;

use mlua::{FromLua, Function, Lua, Table, Value};
use opencode_plugin_spec::{self as spec, ContributionKind, Record};
use rune_luau::Data;
use serde::de::DeserializeOwned;

use super::vm::Vmx;
use super::plugin::{Contrib, Contribs, Kind};
use crate::plugin_host::icons::intern;

/// Builds the context table for VM `vmx`.
pub fn build(lua: &Lua, vmx: &Rc<Vmx>, hot: &Table) -> mlua::Result<Table> {
    let ctx = lua.create_table()?;
    let half = &vmx.half;
    ctx.raw_set("id", &*half.id)?;
    ctx.raw_set("half", if half.kind == Kind::Window { "window" } else { "app" })?;
    ctx.raw_set("contract", spec::CONTRACT)?;
    let v = vmx.clone();
    ctx.raw_set("GetService", lua.create_function(move |lua, (_, name): (Value, String)| super::services::get(lua, &v, &name))?)?;
    // A plugin's own signal is how it says what it draws changed: each fire redraws its
    // surfaces, as `Contribution:Refresh` does (`super::poll`).
    let signal = vmx.script_signal.clone();
    if half.owns_redraw {
        let owner = Rc::downgrade(half);
        let redraw = lua.create_function(move |_, _: mlua::MultiValue| {
            if let Some(half) = owner.upgrade() { half.epoch.set(half.epoch.get().wrapping_add(1)); }
            Ok(())
        })?;
        let new: Function = lua.load("local new, redraw = ...; return function() local s = new(); s:Connect(redraw); return s end").set_name("=ctx.CreateSignal").call((signal, redraw))?;
        ctx.raw_set("CreateSignal", new)?;
    } else {
        ctx.raw_set("CreateSignal", lua.create_function(move |_, _: Value| signal.call::<Value>(()))?)?;
    }
    let hot = hot.clone();
    ctx.raw_set(
        "BindHotState",
        lua.create_function(move |_, (_, save, load): (Value, Function, Function)| {
            hot.raw_set("save", save)?;
            hot.raw_set("load", load)
        })?,
    )?;
    if half.kind == Kind::Window {
        let v = vmx.clone();
        ctx.raw_set("CreateEditor", lua.create_function(move |lua, (_, initial): (Value, Option<String>)| super::services::editor(lua, &v, initial))?)?;
    }
    ctx.raw_set("Disposing", super::services::signal(lua, vmx, "ctx.Disposing")?)?;
    if half.kind == Kind::Window {
        ctx.raw_set("FocusChanged", super::services::signal(lua, vmx, "ctx.FocusChanged")?)?;
    }
    for kind in ContributionKind::ALL.iter().copied().filter(|k| allowed(*k, half.kind)) {
        let v = vmx.clone();
        let add = lua.create_function(move |lua, (_, spec): (Value, Table)| add(lua, &v, kind, &spec))?;
        ctx.raw_set(kind.method(), add)?;
    }
    Ok(ctx)
}

/// Which half may add a kind (the contract's `halves`).
fn allowed(kind: ContributionKind, half: Kind) -> bool {
    use ContributionKind as K;
    match kind {
        K::MenubarItem => half == Kind::App,
        K::ServerSource | K::McpTool => true,
        _ => half == Kind::Window,
    }
}

fn add(lua: &Lua, vmx: &Rc<Vmx>, kind: ContributionKind, t: &Table) -> mlua::Result<Value> {
    use ContributionKind as K;
    let half = &vmx.half;
    let id: String = t.raw_get::<Option<String>>(if kind == K::McpTool { "name" } else { "id" })?.unwrap_or_default();
    if id.is_empty() {
        return Err(mlua::Error::runtime(format!("{}: the spec needs an id", kind.method())));
    }
    let key = intern(&format!("{}.{id}", half.id));
    vmx.with_contribs(|c| -> mlua::Result<()> {
        match kind {
            K::Command => push(lua, &mut c.commands, t, key, vmx),
            K::Panel => push(lua, &mut c.panels, t, key, vmx),
            K::Setting => push(lua, &mut c.settings, t, key, vmx),
            K::SettingsPage => push(lua, &mut c.settings_pages, t, key, vmx),
            K::MenuItem => push(lua, &mut c.menu_items, t, key, vmx),
            K::MenubarItem => push(lua, &mut c.menubar_items, t, key, vmx),
            K::LinkHandler => push(lua, &mut c.link_handlers, t, key, vmx),
            K::TitlebarItem => push(lua, &mut c.titlebar_items, t, key, vmx),
            K::ComposerPill => push(lua, &mut c.composer_pills, t, key, vmx),
            K::CodeBlock => push(lua, &mut c.code_blocks, t, key, vmx),
            K::CodeBlockAction => push(lua, &mut c.code_block_actions, t, key, vmx),
            K::Slot => push(lua, &mut c.slots, t, key, vmx),
            K::Theme => push(lua, &mut c.themes, t, key, vmx),
            K::FileViewer => push(lua, &mut c.file_viewers, t, key, vmx),
            K::ServerSource => push(lua, &mut c.server_sources, t, key, vmx),
            K::Dialog => push(lua, &mut c.dialogs, t, key, vmx),
            K::Popover => push(lua, &mut c.popovers, t, key, vmx),
            K::McpTool => push(lua, &mut c.mcp_tools, t, key, vmx),
        }
    })?;
    half.epoch.set(half.epoch.get() + 1);
    crate::plugin_host::bump();
    contribution(lua, vmx, kind, id)
}

/// Decodes `t` as `S` (warning once per unknown key), keeps its functions, and replaces any
/// earlier contribution with the same id.
fn push<S: DeserializeOwned + Record>(lua: &Lua, list: &mut Vec<Contrib<S>>, t: &Table, key: &'static str, vmx: &Vmx) -> mlua::Result<()> {
    let data = Data::from_lua(Value::Table(t.clone()), lua)?;
    if let Data::Map(fields) = &data {
        for name in fields.keys().filter(|k| !S::knows(k)) {
            vmx.warn(&format!("{}: unknown key {name:?}", S::NAME));
        }
    }
    let spec: S = data.decode().map_err(|e| mlua::Error::runtime(format!("{}: {e}", S::NAME)))?;
    let mut fns = BTreeMap::new();
    // A key that takes a value or a function (`enabled`) keeps only a function here; its value
    // decoded into the spec.
    for name in S::CALLBACKS {
        if let Value::Function(f) = t.raw_get::<Value>(*name)? {
            fns.insert(*name, f);
        }
    }
    list.retain(|c| c.key != key);
    list.push(Contrib { spec, fns, key });
    Ok(())
}

/// The `Contribution` an `Add*` returns: `Withdraw` takes it out, `Refresh` redraws it.
fn contribution(lua: &Lua, vmx: &Rc<Vmx>, kind: ContributionKind, id: String) -> mlua::Result<Value> {
    let t = lua.create_table()?;
    let (v, gone) = (vmx.clone(), id.clone());
    t.raw_set(
        "Withdraw",
        lua.create_function(move |_, _: Value| {
            v.with_contribs(|c: &mut Contribs| c.withdraw(kind, &gone));
            v.half.epoch.set(v.half.epoch.get() + 1);
            crate::plugin_host::bump();
            Ok(())
        })?,
    )?;
    let v = vmx.clone();
    t.raw_set(
        "Refresh",
        lua.create_function(move |_, _: Value| {
            v.half.epoch.set(v.half.epoch.get() + 1);
            crate::plugin_host::bump();
            Ok(())
        })?,
    )?;
    if kind == ContributionKind::ServerSource {
        let order = vmx.with_contribs(|c| c.server_sources.iter().find(|s| s.spec.id == id).and_then(|s| s.spec.order)).unwrap_or(0);
        super::server_sources::add(&vmx.half.id, &id, order);
        let (v, source) = (vmx.clone(), id.clone());
        t.raw_set(
            "SetEntries",
            lua.create_function(move |lua, (_, entries, ready): (Value, Value, Option<bool>)| {
                let entries: Vec<spec::ServerEntry> = spec::lenient::list(Data::from_lua(entries, lua)?.to_json()).map_err(|e| mlua::Error::runtime(format!("ServerSource:SetEntries: {e}")))?;
                super::server_sources::set_entries(&v.half.id, &source, entries, ready.unwrap_or(true));
                super::cx::ask(&v.half.id, super::cx::Request::ServersChanged);
                Ok(())
            })?,
        )?;
        let (v, source) = (vmx.clone(), id);
        t.raw_set(
            "Withdraw",
            lua.create_function(move |_, _: Value| {
                v.with_contribs(|c: &mut Contribs| c.withdraw(kind, &source));
                super::server_sources::withdraw(&v.half.id, &source);
                super::cx::ask(&v.half.id, super::cx::Request::ServersChanged);
                Ok(())
            })?,
        )?;
    }
    Ok(Value::Table(t))
}
