//! Store (values kept with the app, shared by every window), Contracts (values one plugin
//! half provides and others use, across VMs) and Console (the Studio output and command bar).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{FromLua, Function, IntoLua, Lua, MultiValue, Table, Value, Variadic};
use rune_luau::{Data, LogLevel};

use super::{app_now, lazy_signals, method, signal};
use crate::plugin_host::luau::cx::{self, Dep};
use crate::plugin_host::luau::plugin::Kind;
use crate::plugin_host::luau::{console, vm::Vmx};

thread_local! {
    /// Each store value's version, by `(plugin, key)`: what memos and `Changed` follow.
    static VERSIONS: RefCell<HashMap<(String, String), u64>> = RefCell::default();
    static CONTRACTS: RefCell<HashMap<String, Provided>> = RefCell::default();
}

/// A value a plugin half provided: plain data, and its functions by path, called in the
/// provider's VM.
struct Provided {
    owner: Rc<str>,
    /// The half that provided it: each half withdraws only its own.
    kind: Kind,
    vm: u64,
    data: Data,
    fns: Vec<(Vec<String>, Function)>,
    generation: u64,
}

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, name: &str, t: &Table) -> mlua::Result<()> {
    match name {
        "Store" => store(lua, vmx, t),
        "Contracts" => contracts(lua, vmx, t),
        _ => {
            for (name, level) in [("Log", LogLevel::Info), ("Warn", LogLevel::Warn), ("Error", LogLevel::Error)] {
                let plugin = vmx.half.id.clone();
                method(lua, t, name, move |_, args: Variadic<Value>| {
                    let text: Vec<String> = args.iter().map(|v| v.to_string().unwrap_or_else(|_| v.type_name().to_owned())).collect();
                    console::push(&plugin, level, &text.join("\t"));
                    Ok(())
                })?;
            }
            method(lua, t, "Lines", |lua, ()| {
                cx::read(Dep::Console);
                let app = app_now("Console:Lines")?;
                console::lines(app).into_lua(lua)
            })?;
            method(lua, t, "Clear", |_, ()| {
                console::clear();
                crate::plugin_host::luau::bump();
                Ok(())
            })?;
            let own = vmx.half.id.clone();
            method(lua, t, "Eval", move |lua, (plugin, source): (String, String)| {
                let app = app_now("Console:Eval")?;
                let result = if !super::diag::debug_build() {
                    Err("The console's command bar runs in local and beta builds only".to_owned())
                } else if *own == *plugin {
                    console::eval_here(lua, &source)
                } else {
                    console::eval(app, &plugin, &source)
                };
                let out = lua.create_table()?;
                out.raw_set("ok", result.is_ok())?;
                match result {
                    Ok(value) => out.raw_set("value", value)?,
                    Err(e) => out.raw_set("error", e)?,
                }
                Ok(out)
            })
        }
    }
}

/// The version of store `key` of `plugin`.
pub fn store_version(plugin: &str, key: &str) -> u64 {
    VERSIONS.with_borrow(|v| v.get(&(plugin.to_owned(), key.to_owned())).copied().unwrap_or(0))
}

/// Notes that store `key` of `plugin` changed.
pub fn store_changed(plugin: &str, key: &str) {
    VERSIONS.with_borrow_mut(|v| *v.entry((plugin.to_owned(), key.to_owned())).or_default() += 1);
    crate::plugin_host::luau::bump();
}

fn store(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    for (name, per_session) in [("Value", false), ("SessionValue", true)] {
        let v = vmx.clone();
        method(lua, t, name, move |lua, (key, default): (String, Value)| value(lua, &v, key, default, per_session))?;
    }
    Ok(())
}

fn value(lua: &Lua, vmx: &Rc<Vmx>, name: String, default: Value, per_session: bool) -> mlua::Result<Table> {
    let plugin = vmx.half.id.clone();
    let default = Data::from_lua(default, lua)?;
    let o = lua.create_table()?;
    let read = {
        let (plugin, name, default) = (plugin.clone(), name.clone(), default.clone());
        // Each key's value as last parsed, by its version: a render reads it every time it runs.
        let parsed: Rc<std::cell::RefCell<std::collections::HashMap<String, (u64, Data)>>> = Rc::default();
        move |lua: &Lua, session: &str| -> mlua::Result<Value> {
            let key = crate::plugin_host::shared::key(&name, session);
            cx::read(Dep::Store(plugin.clone(), key.clone()));
            let version = store_version(&plugin, &key);
            if let Some((v, data)) = parsed.borrow().get(&key)
                && *v == version
            {
                return data.clone().into_lua(lua);
            }
            let text = app_now("Store")?.plugins.shared.stores.text(&plugin, &name, session);
            let data = text.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).map_or_else(|| default.clone(), |j| super::json_to_data(&j));
            parsed.borrow_mut().insert(key, (version, data.clone()));
            data.into_lua(lua)
        }
    };
    let write = {
        let (plugin, name) = (plugin.clone(), name.clone());
        move |lua: &Lua, session: &str, value: Value| -> mlua::Result<()> {
            let json = Data::from_lua(value, lua)?.to_json().to_string();
            app_now("Store")?.plugins.shared.stores.set(&plugin, &name, session, &json);
            store_changed(&plugin, &crate::plugin_host::shared::key(&name, session));
            Ok(())
        }
    };
    if per_session {
        let r = read.clone();
        method(lua, &o, "Get", move |lua, session: String| r(lua, &session))?;
        let w = write.clone();
        method(lua, &o, "Set", move |lua, (session, value): (String, Value)| w(lua, &session, value))?;
    } else {
        method(lua, &o, "Get", move |lua, ()| read(lua, ""))?;
        method(lua, &o, "Set", move |lua, value: Value| write(lua, "", value))?;
    }
    let (p, n) = (plugin.clone(), name.clone());
    method(lua, &o, "Version", move |_, ()| {
        cx::read(Dep::Store(p.clone(), n.clone()));
        Ok(store_version(&p, &n) as f64)
    })?;
    o.raw_set("Changed", signal(lua, vmx, &format!("Store.{name}.Changed"))?)?;
    Ok(o)
}

/// The generation of contract `id` (0 when nothing provides it).
pub fn contract_generation(id: &str) -> u64 {
    CONTRACTS.with_borrow(|c| c.get(id).map_or(0, |p| p.generation))
}

/// Forgets what plugin `owner`'s `kind` half provided from any VM but `keep`.
pub fn withdraw_owner(owner: &str, kind: Kind, keep: Option<u64>) {
    let changed = CONTRACTS.with_borrow_mut(|c| {
        let before = c.len();
        c.retain(|_, p| &*p.owner != owner || p.kind != kind || Some(p.vm) == keep);
        before != c.len()
    });
    if changed {
        crate::plugin_host::luau::bump();
    }
}

fn contracts(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v = vmx.clone();
    method(lua, t, "Provide", move |lua, (id, value): (String, Value)| {
        let mut fns = Vec::new();
        let data = split(&value, &mut Vec::new(), &mut fns, 0);
        let generation = contract_generation(&id) + 1;
        CONTRACTS.with_borrow_mut(|c| c.insert(id.clone(), Provided { owner: v.half.id.clone(), kind: v.half.kind, vm: v.vm.get(), data, fns, generation }));
        crate::plugin_host::luau::bump();
        let handle = lua.create_table()?;
        let gone = id.clone();
        let owner = v.half.id.clone();
        method(lua, &handle, "Withdraw", move |_, ()| {
            CONTRACTS.with_borrow_mut(|c| {
                if c.get(&gone).is_some_and(|p| p.owner == owner) {
                    c.remove(&gone);
                }
            });
            crate::plugin_host::luau::bump();
            Ok(())
        })?;
        method(lua, &handle, "Refresh", |_, ()| Ok(()))?;
        Ok(handle)
    })?;
    let v = vmx.clone();
    method(lua, t, "Use", move |lua, id: String| live(lua, &v, id))?;
    method(lua, t, "Summary", |lua, ()| {
        let out = lua.create_table()?;
        CONTRACTS.with_borrow(|c| c.keys().try_for_each(|id| out.raw_set(id.as_str(), "active")))?;
        Ok(out)
    })?;
    Ok(())
}

/// Splits a provided value into plain data and its functions by path.
fn split(v: &Value, path: &mut Vec<String>, fns: &mut Vec<(Vec<String>, Function)>, depth: usize) -> Data {
    match v {
        Value::Function(f) => {
            fns.push((path.clone(), f.clone()));
            Data::Nil
        }
        // An array stays a list (its items may hold functions too).
        Value::Table(t) if depth < 16 && t.raw_len() > 0 => {
            let items = (1..=t.raw_len())
                .map(|i| {
                    path.push(i.to_string());
                    let d = split(&t.raw_get::<Value>(i).unwrap_or(Value::Nil), path, fns, depth + 1);
                    path.pop();
                    d
                })
                .collect();
            Data::List(items)
        }
        Value::Table(t) if depth < 16 => {
            let mut fields = std::collections::BTreeMap::new();
            for (k, item) in t.pairs::<String, Value>().flatten() {
                path.push(k.clone());
                let d = split(&item, path, fns, depth + 1);
                path.pop();
                if d != Data::Nil {
                    fields.insert(k, d);
                }
            }
            Data::Map(fields)
        }
        Value::Boolean(b) => Data::Bool(*b),
        Value::Integer(i) => Data::Integer(*i as i64),
        Value::Number(n) => Data::Number(*n),
        Value::String(s) => Data::String(s.to_string_lossy()),
        _ => Data::Nil,
    }
}

fn live(lua: &Lua, vmx: &Rc<Vmx>, id: String) -> mlua::Result<Table> {
    let o = lua.create_table()?;
    let i = id.clone();
    method(lua, &o, "State", move |_, ()| {
        cx::read(Dep::Contract(i.clone()));
        Ok(if contract_generation(&i) > 0 { "active" } else { "pending" })
    })?;
    let i = id.clone();
    method(lua, &o, "Generation", move |_, ()| {
        cx::read(Dep::Contract(i.clone()));
        Ok(contract_generation(&i) as f64)
    })?;
    let i = id.clone();
    method(lua, &o, "Get", move |lua, ()| {
        cx::read(Dep::Contract(i.clone()));
        let Some((data, fns)) = CONTRACTS.with_borrow(|c| c.get(&i).map(|p| (p.data.clone(), p.fns.clone()))) else { return Ok(Value::Nil) };
        let value = data.into_lua(lua)?;
        for (path, f) in fns {
            let (gen_id, generation) = (i.clone(), contract_generation(&i));
            let proxy = lua.create_function(move |lua, args: MultiValue| {
                // A provider that reloaded or went away answers nothing.
                if contract_generation(&gen_id) != generation {
                    return Err(mlua::Error::runtime(format!("contract {gen_id} changed; use Get() again")));
                }
                let args: Vec<Data> = args.into_iter().map(|a| Data::from_lua(a, lua)).collect::<mlua::Result<_>>()?;
                f.call::<Data>(Variadic::from_iter(args))?.into_lua(lua)
            })?;
            set_path(lua, &value, &path, Value::Function(proxy))?;
        }
        Ok(value)
    })?;
    o.raw_set("Changed", signal(lua, vmx, &format!("Contracts.{id}.Changed"))?)?;
    let _ = lazy_signals;
    Ok(o)
}

fn set_path(lua: &Lua, root: &Value, path: &[String], v: Value) -> mlua::Result<()> {
    let Value::Table(mut t) = root.clone() else { return Ok(()) };
    for (n, key) in path.iter().enumerate() {
        // A list's items go by their index.
        let key: Value = match key.parse::<i64>() {
            Ok(i) if t.raw_len() > 0 => Value::Integer(i as _),
            _ => Value::String(lua.create_string(key)?),
        };
        if n + 1 == path.len() {
            return t.raw_set(key, v);
        }
        t = match t.raw_get::<Value>(key.clone())? {
            Value::Table(next) => next,
            _ => {
                let next = lua.create_table()?;
                t.raw_set(key, &next)?;
                next
            }
        };
    }
    Ok(())
}

