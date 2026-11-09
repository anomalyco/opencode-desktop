//! The contract's host services, as `ctx:GetService(name)` returns them. Each is a table of
//! methods built once per VM; its signals (`Sessions.Changed`) are made when first read, so the
//! host fires only what a plugin listens to. Reads in a render record what they read
//! ([`super::cx::read`]); changes are queued ([`super::cx::ask`]) and carried out after the entry.
//!
//! LU-2 implements: Sessions, Models, Server, Servers, Http, Process, Store, Contracts, Console,
//! Toast, Popover, Dialog, Menu, Layout, Links, Commands, Theme, App, System, Notifications,
//! Workspace (files and downloads), Diagnostics (metrics) and I18n. The rest answer with an error
//! naming the package that brings them (see `PENDING`).

mod app;
pub mod diag;
mod i18n;
mod net;
pub mod process;
mod session;
pub mod store;
mod ui;
mod watch;

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use mlua::{Function, Lua, MultiValue, Table, Value};
use rune_luau::{Data, Host};

use super::cx::{self, Dep};
use super::plugin::{Half, Kind};
use super::vm::Vmx;
use crate::App;

pub use session::session_version;
pub use store::store_version;
pub use ui::panel_state as ui_panel_state;
pub use watch::fire_watched;

/// Services later packages bring, and which.
const PENDING: &[(&str, &str)] = &[
    ("Diff", "LU-6"),
    ("Terminal", "LU-6"),
    ("Browser", "LU-7"),
    ("Office", "LU-5"),
    ("Updater", "LU-5"),
    ("Power", "LU-4"),
    ("Qr", "LU-4"),
    ("Askpass", "LU-4"),
    ("Images", "LU-5"),
];

/// Window-half-only services.
const WINDOW_ONLY: &[&str] = &["Popover", "Dialog", "Menu", "Layout", "Links", "Commands", "Diff"];

/// `ctx:GetService(name)`, built once per VM.
pub fn get(lua: &Lua, vmx: &Rc<Vmx>, name: &str) -> mlua::Result<Value> {
    let cache: Table = match lua.named_registry_value::<Option<Table>>("opencode.services")? {
        Some(t) => t,
        None => {
            let t = lua.create_table()?;
            lua.set_named_registry_value("opencode.services", &t)?;
            t
        }
    };
    if let Some(found) = cache.raw_get::<Option<Table>>(name)? {
        return Ok(Value::Table(found));
    }
    if vmx.half.kind == Kind::App && WINDOW_ONLY.contains(&name) {
        return Err(mlua::Error::runtime(format!("{name} is a window service: the app half has no window")));
    }
    let t = lua.create_table()?;
    match name {
        "Sessions" | "Models" | "Server" | "Servers" | "Theme" => session::build(lua, vmx, name, &t)?,
        "Http" | "Workspace" => net::build(lua, vmx, name, &t)?,
        "Process" => process::build(lua, vmx, &t)?,
        "Store" | "Contracts" | "Console" => store::build(lua, vmx, name, &t)?,
        "Diagnostics" => diag::build(lua, vmx, &t)?,
        "Toast" | "Popover" | "Dialog" | "Menu" | "Layout" | "Links" | "Commands" | "Notifications" => ui::build(lua, vmx, name, &t)?,
        "App" | "System" => app::build(lua, vmx, name, &t)?,
        "I18n" => i18n::build(lua, vmx, &t)?,
        _ => {
            let why = PENDING.iter().find(|(n, _)| *n == name).map_or_else(|| format!("there is no service {name:?}"), |(_, pkg)| format!("{name} isn't in this app yet ({pkg} brings it)"));
            return Err(mlua::Error::runtime(why));
        }
    }
    cache.raw_set(name, &t)?;
    Ok(Value::Table(t))
}

/// The host's async functions, made with the VM (they need its `Host`).
pub fn asyncs(host: &Host<'_>, t: &Table, half: &Rc<Half>) -> mlua::Result<()> {
    net::asyncs(host, t, half)?;
    process::asyncs(host, t, half)?;
    diag::asyncs(host, t, half)?;
    ui::asyncs(host, t, half)
}

/// Adds method `name` to service `t`: `f(lua, args)` without the `self` the colon passes.
pub(super) fn method<A: mlua::FromLuaMulti, R: mlua::IntoLuaMulti>(lua: &Lua, t: &Table, name: &str, f: impl Fn(&Lua, A) -> mlua::Result<R> + 'static) -> mlua::Result<()> {
    t.raw_set(name, lua.create_function(move |lua, args: MultiValue| {
        let mut args = args;
        args.pop_front();
        f(lua, A::from_lua_multi(args, lua)?)
    })?)
}

/// Adds the async function `name` (made in [`asyncs`]) as method `method` of `t`.
pub(super) fn async_method(lua: &Lua, vmx: &Rc<Vmx>, t: &Table, method: &str, name: &str) -> mlua::Result<()> {
    let f: Function = vmx.asyncs.raw_get(name)?;
    let wrap: Function = lua.load("local f = ... ; return function(_, ...) return f(...) end").call(f)?;
    t.raw_set(method, wrap)
}

/// A host signal: made now, fired by the host through [`fire`] under `name`.
pub fn signal(_: &Lua, vmx: &Rc<Vmx>, name: &str) -> mlua::Result<Value> {
    let key = (vmx.vm.get(), name.to_owned());
    if let Some(s) = vmx.half.signals.borrow().get(&key) {
        return Ok(s.clone());
    }
    let s: Value = vmx.script_signal.call(())?;
    vmx.half.signals.borrow_mut().insert(key, s.clone());
    Ok(s)
}

/// Makes signal fields of `t` (`Changed`, ...) as they are first read.
pub(super) fn lazy_signals(lua: &Lua, vmx: &Rc<Vmx>, t: &Table, service: &str, fields: &'static [&'static str]) -> mlua::Result<()> {
    let (v, service) = (vmx.clone(), service.to_owned());
    let mt = lua.create_table()?;
    mt.raw_set(
        "__index",
        lua.create_function(move |lua, (t, key): (Table, String)| {
            if !fields.contains(&key.as_str()) {
                return Ok(Value::Nil);
            }
            let s = signal(lua, &v, &format!("{service}.{key}"))?;
            t.raw_set(key, &s)?;
            Ok(s)
        })?,
    )?;
    t.set_metatable(Some(mt))?;
    Ok(())
}

/// Fires host signal `name` of every running half in `halves` with `args`, if it made it.
pub fn fire(app: &App, plugin: &super::Plugin, name: &str, args: impl Fn(&Lua) -> mlua::Result<MultiValue>) {
    let (Some(script), Some(vm)) = (plugin.script(), plugin.script().and_then(|s| s.vm_id())) else { return };
    let half = &plugin.half;
    let Some(signal) = half.signals.borrow().get(&(vm, name.to_owned())).cloned() else { return };
    let Some(fire) = half.fire.borrow().get(&vm).cloned() else { return };
    cx::with_app(app, || {
        let Some(Ok(mut values)) = script.with_lua(|lua| args(lua)) else { return };
        values.push_front(signal);
        script.spawn_fn(&fire, values, None).ok();
    });
}

/// `ctx:CreateEditor(initial)`: the n-th editor this setup made is the window's n-th text field
/// of the plugin, its text and caret kept across reloads.
pub fn editor(lua: &Lua, vmx: &Rc<Vmx>, initial: Option<String>) -> mlua::Result<Table> {
    let index = vmx.half.editors_made.get();
    vmx.half.editors_made.set(index + 1);
    super::editors::ensure(&vmx.half.id, index, initial.as_deref().unwrap_or_default());
    let t = lua.create_table()?;
    t.raw_set("id", index)?;
    t.raw_set("plugin", &*vmx.half.id)?;
    let mt = lua.create_table()?;
    let methods = lua.create_table()?;
    let id = vmx.half.id.clone();
    let p = id.clone();
    method(lua, &methods, "Text", move |_, ()| {
        cx::read(Dep::Editor(p.clone(), index));
        Ok(super::editors::text(&p, index))
    })?;
    let p = id.clone();
    method(lua, &methods, "Version", move |_, ()| {
        cx::read(Dep::Editor(p.clone(), index));
        Ok(super::editors::version(&p, index))
    })?;
    let p = id.clone();
    method(lua, &methods, "SetText", move |_, text: String| {
        super::editors::set_text(&p, index, &text);
        Ok(())
    })?;
    let p = id.clone();
    method(lua, &methods, "Focus", move |_, ()| {
        super::editors::focus(&p, index);
        cx::ask(&p, cx::Request::Redraw);
        Ok(())
    })?;
    methods.raw_set("Changed", signal(lua, vmx, &format!("editor.{index}.Changed"))?)?;
    methods.raw_set("Submitted", signal(lua, vmx, &format!("editor.{index}.Submitted"))?)?;
    mt.raw_set("__index", methods)?;
    mt.raw_set("__type", "Editor")?;
    t.set_metatable(Some(mt))?;
    Ok(t)
}

/// What `dep` reads now, as a number that changes with it.
pub fn dep_version(app: &App, dep: &Dep) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    match dep {
        Dep::Session(id) => session_version(app, id).hash(&mut h),
        Dep::Routed => app.active_session_id().hash(&mut h),
        Dep::Sessions => app.sessions.iter().map(|s| session_version(app, &s.id)).for_each(|v| v.hash(&mut h)),
        Dep::Model(provider, model) => session::model_version(app, provider, model).hash(&mut h),
        Dep::Resource(plugin, path) => app.plugins.luau.half(plugin).and_then(|half| half.resources.borrow().get(path).map(|r| r.ticket.generation())).hash(&mut h),
        Dep::Store(plugin, key) => store_version(plugin, key).hash(&mut h),
        Dep::Editor(plugin, index) => super::editors::version_in(app, plugin, *index).hash(&mut h),
        Dep::Popover(plugin, id) => app.plugins.luau.overlay.popover_open(plugin, id).hash(&mut h),
        Dep::Flyout(plugin) => app.plugins.luau.overlay.flyout_row(plugin).hash(&mut h),
        Dep::Dialog(plugin, id) => app.plugins.luau.dialog.as_ref().is_some_and(|o| *o.plugin == **plugin && o.id == *id).hash(&mut h),
        Dep::Panel(plugin, panel) => ui::panel_state(app, plugin, panel, app.active_session_id().unwrap_or_default()).hash(&mut h),
        Dep::Narrow => ui::narrow(app).hash(&mut h),
        Dep::Locale => opencode_i18n::locale().id().hash(&mut h),
        Dep::Server => (&app.server_key, app.can_write()).hash(&mut h),
        Dep::Contract(id) => store::contract_generation(id).hash(&mut h),
        Dep::Dev(name) => format!("{:?}", diag::get(app, name)).hash(&mut h),
        Dep::Console => super::console::version().hash(&mut h),
    }
    h.finish()
}

/// A JSON value as a Luau value.
pub fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        J::String(s) => Value::String(lua.create_string(s)?),
        J::Array(items) => {
            let t = lua.create_table_with_capacity(items.len(), 0)?;
            for (i, item) in items.iter().enumerate() {
                t.raw_set(i + 1, json_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        J::Object(fields) => {
            let t = lua.create_table_with_capacity(0, fields.len())?;
            for (k, item) in fields {
                t.raw_set(k.as_str(), json_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
    })
}

/// A JSON value as [`Data`], for a completer.
pub fn json_to_data(v: &serde_json::Value) -> Data {
    use serde_json::Value as J;
    match v {
        J::Null => Data::Nil,
        J::Bool(b) => Data::Bool(*b),
        J::Number(n) => n.as_i64().map_or_else(|| Data::Number(n.as_f64().unwrap_or(0.0)), Data::Integer),
        J::String(s) => Data::String(s.clone()),
        J::Array(items) => Data::List(items.iter().map(json_to_data).collect()),
        J::Object(fields) => Data::Map(fields.iter().map(|(k, v)| (k.clone(), json_to_data(v))).collect()),
    }
}

/// A map of `Data` fields.
pub fn map(fields: impl IntoIterator<Item = (&'static str, Data)>) -> Data {
    Data::Map(fields.into_iter().filter(|(_, v)| *v != Data::Nil).map(|(k, v)| (k.to_owned(), v)).collect())
}

/// The app of the entry running now, or the error a binding raises without one.
pub(super) fn app_now<'a>(what: &str) -> mlua::Result<&'a App> {
    cx::app().ok_or_else(|| mlua::Error::runtime(format!("{what} can only be read while the plugin runs")))
}
