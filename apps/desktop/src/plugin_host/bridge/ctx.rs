//! The `ctx` a plugin half's setup receives: `GetService`, `CreateSignal`, `CreateEditor`,
//! `BindHotState`, the `Add*` contribution methods of its half, and its `Disposing` signal.
//! Spec decoding and contribution storage delegate to `opencode_plugin_runtime::contribs`.

use std::rc::Rc;

use mlua::{FromLua, Function, Lua, Table, Value};
use opencode_plugin_runtime::contribs::allowed;
use opencode_plugin_spec::{self as spec, ContributionKind};
use rune_luau::Data;

use crate::plugin_host::icons::intern;
use crate::plugin_host::plugin::{Contribs, Kind};
use crate::plugin_host::services;
use crate::plugin_host::vm::Vmx;

/// Builds the context table for VM `vmx`.
pub fn build(lua: &Lua, vmx: &Rc<Vmx>, hot: &Table) -> mlua::Result<Table> {
    let ctx = lua.create_table()?;
    let half = &vmx.half;
    ctx.raw_set("id", &*half.id)?;
    ctx.raw_set("half", if half.kind == Kind::Window { "window" } else { "app" })?;
    ctx.raw_set("contract", spec::CONTRACT)?;
    let v = vmx.clone();
    ctx.raw_set("GetService", lua.create_function(move |lua, (_, name): (Value, String)| services::get(lua, &v, &name))?)?;
    // A plugin's own signal is how it says what it draws changed: each fire redraws its
    // surfaces, as `Contribution:Refresh` does (`crate::plugin_host::poll`).
    let signal = vmx.script_signal.clone();
    if half.owns_redraw {
        let owner = Rc::downgrade(half);
        let redraw = lua.create_function(move |_, _: mlua::MultiValue| {
            if let Some(half) = owner.upgrade() { half.epoch.set(half.epoch.get().wrapping_add(1)); }
            Ok(())
        })?;
        let new: Function = opencode_plugin_runtime::chunk::load(lua, "local new, redraw = ...; return function() local s = new(); s:Connect(redraw); return s end").set_name("=ctx.CreateSignal").call((signal, redraw))?;
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
        ctx.raw_set("CreateEditor", lua.create_function(move |lua, (_, initial): (Value, Option<String>)| services::editor(lua, &v, initial))?)?;
    }
    ctx.raw_set("Disposing", services::signal(lua, vmx, "ctx.Disposing")?)?;
    if half.kind == Kind::Window {
        ctx.raw_set("FocusChanged", services::signal(lua, vmx, "ctx.FocusChanged")?)?;
    }
    for kind in ContributionKind::ALL.iter().copied().filter(|k| allowed(*k, half.kind)) {
        let v = vmx.clone();
        let add = lua.create_function(move |lua, (_, spec): (Value, Table)| add(lua, &v, kind, &spec))?;
        ctx.raw_set(kind.method(), add)?;
    }
    Ok(ctx)
}

fn add(lua: &Lua, vmx: &Rc<Vmx>, kind: ContributionKind, t: &Table) -> mlua::Result<Value> {
    let half = &vmx.half;
    let id: String = t.raw_get::<Option<String>>(if kind == ContributionKind::McpTool { "name" } else { "id" })?.unwrap_or_default();
    if id.is_empty() {
        return Err(mlua::Error::runtime(format!("{}: the spec needs an id", kind.method())));
    }
    let key = intern(&format!("{}.{id}", half.id));
    vmx.with_contribs(|c| c.add(lua, kind, t, key, &|msg| vmx.warn(msg)))?;
    half.epoch.set(half.epoch.get() + 1);
    crate::plugin_host::bump();
    contribution(lua, vmx, kind, id)
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
