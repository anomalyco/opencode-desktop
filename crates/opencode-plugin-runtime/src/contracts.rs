//! `Contracts`: values one plugin half provides and others use, across VMs. Plain data crosses
//! as [`Data`]; functions at any path are proxied back into the provider's VM and invalidated
//! when the provider reloads or withdraws.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{FromLua, Function, IntoLua, Lua, MultiValue, Table, Value, Variadic};
use rune_luau::Data;

use crate::Kind;
use crate::luau::method;

/// Host callbacks used by [`build`] to record contract reads, bump surface epochs, and trace calls.
#[derive(Clone, Copy)]
pub struct Hooks {
    pub on_read: fn(&str),
    pub on_change: fn(),
    pub trace_on: fn() -> bool,
    pub trace_call: fn(&str, &str, &str, u64),
}

thread_local! {
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

/// The generation of contract `id` (0 when nothing provides it).
pub fn generation(id: &str) -> u64 {
    CONTRACTS.with_borrow(|c| c.get(id).map_or(0, |p| p.generation))
}

/// Forgets what plugin `owner`'s `kind` half provided from any VM but `keep`. Returns `true`
/// when at least one contract was removed.
pub fn withdraw_owner(owner: &str, kind: Kind, keep: Option<u64>) -> bool {
    CONTRACTS.with_borrow_mut(|c| {
        let before = c.len();
        c.retain(|_, p| &*p.owner != owner || p.kind != kind || Some(p.vm) == keep);
        before != c.len()
    })
}

/// Populates the `Contracts` service table (`Provide`, `Use`, `Summary`).
pub fn build(
    lua: &Lua,
    t: &Table,
    owner: Rc<str>,
    kind: Kind,
    get_vm: impl Fn() -> u64 + 'static,
    make_signal: impl Fn(&Lua, &str) -> mlua::Result<Value> + 'static,
    hooks: Hooks,
) -> mlua::Result<()> {
    let provide_owner = owner;
    method(lua, t, "Provide", move |lua, (id, value): (String, Value)| {
        let mut fns = Vec::new();
        let data = split(&value, &mut Vec::new(), &mut fns, 0);
        let gen_next = generation(&id) + 1;
        CONTRACTS.with_borrow_mut(|c| c.insert(id.clone(), Provided { owner: provide_owner.clone(), kind, vm: get_vm(), data, fns, generation: gen_next }));
        (hooks.on_change)();
        let handle = lua.create_table()?;
        let gone = id.clone();
        let owner = provide_owner.clone();
        method(lua, &handle, "Withdraw", move |_, ()| {
            CONTRACTS.with_borrow_mut(|c| {
                if c.get(&gone).is_some_and(|p| p.owner == owner) {
                    c.remove(&gone);
                }
            });
            (hooks.on_change)();
            Ok(())
        })?;
        method(lua, &handle, "Refresh", |_, ()| Ok(()))?;
        Ok(handle)
    })?;
    method(lua, t, "Use", move |lua, id: String| live(lua, id, &make_signal, hooks))?;
    method(lua, t, "Summary", |lua, ()| {
        let out = lua.create_table()?;
        CONTRACTS.with_borrow(|c| c.keys().try_for_each(|id| out.raw_set(id.as_str(), "active")))?;
        Ok(out)
    })
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
        // `mlua::Integer` is `i32` on wasm32 and `i64` on 64-bit targets.
        #[allow(clippy::unnecessary_cast)]
        Value::Integer(i) => Data::Integer(*i as i64),
        Value::Number(n) => Data::Number(*n),
        Value::String(s) => Data::String(s.to_string_lossy()),
        _ => Data::Nil,
    }
}

fn live(lua: &Lua, id: String, make_signal: &impl Fn(&Lua, &str) -> mlua::Result<Value>, hooks: Hooks) -> mlua::Result<Table> {
    let o = lua.create_table()?;
    let i = id.clone();
    method(lua, &o, "State", move |_, ()| {
        (hooks.on_read)(&i);
        Ok(if generation(&i) > 0 { "active" } else { "pending" })
    })?;
    let i = id.clone();
    method(lua, &o, "Generation", move |_, ()| {
        (hooks.on_read)(&i);
        Ok(generation(&i) as f64)
    })?;
    let i = id.clone();
    let cached: RefCell<Option<(u64, mlua::RegistryKey)>> = RefCell::new(None);
    method(lua, &o, "Get", move |lua, ()| {
        (hooks.on_read)(&i);
        let gen_now = generation(&i);
        if let Some((_, key)) = cached.borrow().as_ref().filter(|(g, _)| *g == gen_now) {
            return lua.registry_value::<Value>(key);
        }
        let Some((data, fns)) = CONTRACTS.with_borrow(|c| c.get(&i).map(|p| (p.data.clone(), p.fns.clone()))) else {
            cached.borrow_mut().take();
            return Ok(Value::Nil);
        };
        let value = data.into_lua(lua)?;
        for (path, f) in fns {
            let gen_id = i.clone();
            let fn_name = path.join(".");
            let proxy = lua.create_function(move |lua, args: MultiValue| {
                // A provider that reloaded or went away answers nothing.
                if generation(&gen_id) != gen_now {
                    return Err(mlua::Error::runtime(format!("contract {gen_id} changed; use Get() again")));
                }
                let args: Vec<Data> = args.into_iter().map(|a| Data::from_lua(a, lua)).collect::<mlua::Result<_>>()?;
                let t0 = (hooks.trace_on)().then(rune::time::Instant::now);
                let trace_args = (hooks.trace_on)().then(|| format!("{args:?}"));
                let out = f.call::<Data>(Variadic::from_iter(args))?;
                if let (Some(t0), Some(a)) = (t0, trace_args) {
                    (hooks.trace_call)(&gen_id, &fn_name, &a, t0.elapsed().as_micros() as u64);
                }
                out.into_lua(lua)
            })?;
            set_path(lua, &value, &path, Value::Function(proxy))?;
        }
        let key = lua.create_registry_value(value.clone())?;
        *cached.borrow_mut() = Some((gen_now, key));
        Ok(value)
    })?;
    o.raw_set("Changed", make_signal(lua, &format!("Contracts.{id}.Changed"))?)?;
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
