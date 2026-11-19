//! Pure Luau / `mlua` conversion, record-stack, snapshot-cache, and resource-marshaling helpers
//! shared across plugin services.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use mlua::{Lua, RegistryKey, Table, Value};
use rune_luau::Data;

use crate::machines::{Item, Items, Ticket};

/// Adds method `name` to service table `t`: `f(lua, args)` without the `self` the colon passes.
/// Arguments convert straight off the Lua stack with no intermediate argument list.
pub fn method<A: mlua::FromLuaMulti, R: mlua::IntoLuaMulti>(
    lua: &Lua,
    t: &Table,
    name: &str,
    f: impl Fn(&Lua, A) -> mlua::Result<R> + 'static,
) -> mlua::Result<()> {
    t.raw_set(name, lua.create_function(move |lua, (_, args): (Receiver, A)| f(lua, args))?)
}

/// The `self` a method call passes, skipped where it sits on the stack.
struct Receiver;

impl mlua::FromLua for Receiver {
    fn from_lua(_: Value, _: &Lua) -> mlua::Result<Self> {
        Ok(Receiver)
    }

    #[allow(unsafe_code)]
    unsafe fn from_stack(_: std::os::raw::c_int, _: &mlua::state::RawLua) -> mlua::Result<Self> {
        Ok(Receiver)
    }
}

/// A new table `fill` writes field by field straight on the Lua stack, for the records a render
/// reads (a session, a model): one protected call, rather than a trip through mlua's handles per
/// field.
#[allow(unsafe_code)]
pub fn record(lua: &Lua, fields: i32, fill: impl FnOnce(&mut Record)) -> mlua::Result<Table> {
    // SAFETY: `fill` only pushes values and sets them into the table on top, which stays there.
    unsafe {
        lua.exec_raw::<Table>((), |state| {
            mlua::ffi::lua_createtable(state, 0, fields);
            fill(&mut Record { state });
        })
    }
}

/// The table [`record`] fills, on top of the stack.
pub struct Record {
    state: *mut mlua::ffi::lua_State,
}

impl Record {
    #[allow(unsafe_code)]
    pub fn str(&mut self, key: &std::ffi::CStr, value: &str) {
        // SAFETY: the table is on top; the value pushed above it is set and popped.
        unsafe {
            mlua::ffi::lua_pushlstring(self.state, value.as_ptr().cast(), value.len());
            mlua::ffi::lua_rawsetfield(self.state, -2, key.as_ptr());
        }
    }

    pub fn opt_str(&mut self, key: &std::ffi::CStr, value: Option<&str>) {
        if let Some(v) = value {
            self.str(key, v);
        }
    }

    #[allow(unsafe_code)]
    pub fn num(&mut self, key: &std::ffi::CStr, value: f64) {
        // SAFETY: as in `str`.
        unsafe {
            mlua::ffi::lua_pushnumber(self.state, value);
            mlua::ffi::lua_rawsetfield(self.state, -2, key.as_ptr());
        }
    }

    #[allow(unsafe_code)]
    pub fn bool(&mut self, key: &std::ffi::CStr, value: bool) {
        // SAFETY: as in `str`.
        unsafe {
            mlua::ffi::lua_pushboolean(self.state, i32::from(value));
            mlua::ffi::lua_rawsetfield(self.state, -2, key.as_ptr());
        }
    }

    /// A nested record under `key`.
    #[allow(unsafe_code)]
    pub fn record(&mut self, key: &std::ffi::CStr, fields: i32, fill: impl FnOnce(&mut Record)) {
        // SAFETY: the nested table is pushed, filled while on top, then set into ours.
        unsafe {
            mlua::ffi::lua_createtable(self.state, 0, fields);
            fill(self);
            mlua::ffi::lua_rawsetfield(self.state, -2, key.as_ptr());
        }
    }
}

/// The records this VM's renders read, as last built, by slot and by what they were built from:
/// a render asks for the same session or server list every time it runs.
#[derive(Default)]
struct Snapshots(HashMap<String, (u64, RegistryKey)>);

/// The record in `slot` built from `stamp`: `build` runs only when `stamp` moved, and the script
/// gets its own deep copy of the kept table, which costs far less than building it again (no
/// string is hashed and interned anew), so what it does to its copy stays its own.
pub fn snapshot(lua: &Lua, slot: &str, stamp: u64, build: impl FnOnce() -> mlua::Result<Value>) -> mlua::Result<Value> {
    if lua.app_data_ref::<Snapshots>().is_none() {
        lua.set_app_data(Snapshots::default());
    }
    let kept = lua.app_data_ref::<Snapshots>().and_then(|s| s.0.get(slot).filter(|(at, _)| *at == stamp).map(|(_, k)| lua.registry_value::<Table>(k)));
    if let Some(kept) = kept {
        return deep_copy(lua, &kept?).map(Value::Table);
    }
    let built = build()?;
    let mut kept = lua.app_data_mut::<Snapshots>();
    // A record that isn't a table (a session that doesn't exist) is cheap to make: none is kept.
    let (Value::Table(t), Some(s)) = (&built, kept.as_mut()) else {
        if let Some(s) = kept.as_mut() {
            s.0.remove(slot);
        }
        return Ok(built);
    };
    if s.0.len() >= 256 {
        s.0.clear();
    }
    s.0.insert(slot.to_owned(), (stamp, lua.create_registry_value(t)?));
    drop(kept);
    deep_copy(lua, t).map(Value::Table)
}

/// A copy of `t` and of every table under it (records nest a few levels at most).
#[allow(unsafe_code)]
fn deep_copy(lua: &Lua, t: &Table) -> mlua::Result<Table> {
    // SAFETY: `copy` pushes exactly one table over the argument, which is then removed.
    unsafe {
        lua.exec_raw::<Table>(t, |state| {
            copy(state, -1, 0);
            mlua::ffi::lua_remove(state, -2);
        })
    }
}

/// Pushes a copy of the table at `idx`, copying nested tables up to 8 levels down.
#[allow(unsafe_code)]
unsafe fn copy(state: *mut mlua::ffi::lua_State, idx: std::os::raw::c_int, depth: usize) {
    use mlua::ffi;
    // SAFETY: each pass of the walk pushes a key and a value and leaves the stack as it found it.
    unsafe {
        let idx = ffi::lua_absindex(state, idx);
        ffi::luaL_checkstack(state, 4, std::ptr::null());
        ffi::lua_clonetable(state, idx);
        if depth >= 8 {
            return;
        }
        let clone = ffi::lua_gettop(state);
        let mut iter = 0;
        loop {
            iter = ffi::lua_rawiter(state, clone, iter);
            if iter < 0 {
                break;
            }
            if ffi::lua_type(state, -1) == ffi::LUA_TTABLE {
                copy(state, -1, depth + 1);
                ffi::lua_remove(state, -2);
                ffi::lua_rawset(state, clone);
            } else {
                ffi::lua_settop(state, -3);
            }
        }
    }
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

/// A map of `Data` fields, omitting `Data::Nil` entries.
pub fn map(fields: impl IntoIterator<Item = (&'static str, Data)>) -> Data {
    Data::Map(fields.into_iter().filter(|(_, v)| *v != Data::Nil).map(|(k, v)| (k.to_owned(), v)).collect())
}

/// A cached server read (`Server:Resource`): its ticket, and the answer's text and JSON as Luau
/// values, made once per answer.
#[derive(Default)]
pub struct Resource {
    pub ticket: Ticket,
    pub value: Option<(i64, Rc<RegistryKey>, Rc<RegistryKey>)>,
}

/// Converts a [`Resource`]'s current state into a Luau `ServerResource` table, caching the decoded
/// text and JSON/items in the VM registry once per generation.
pub fn resource_value(lua: &Lua, r: &mut Resource) -> mlua::Result<Value> {
    let (status, error) = match &r.ticket {
        Ticket::Empty | Ticket::Loading { .. } => ("loading", None),
        Ticket::Ready { .. } => ("ready", None),
        Ticket::Failed { error, .. } => ("error", Some(error.as_str())),
    };
    let generation = r.ticket.body().map_or(0, |b| b.generation);
    let out = record(lua, 5, |o| {
        o.str(c"status", status);
        o.opt_str(c"error", error);
        o.num(c"generation", generation as f64);
    })?;
    let Some(body) = r.ticket.body() else {
        out.raw_set("text", "")?;
        return Ok(Value::Table(out));
    };
    // The answer's text and JSON (or its items) become Luau values once per answer, not once per
    // read.
    let (text, decoded) = match &r.value {
        Some((g, text, decoded)) if *g == body.generation => (lua.registry_value::<Value>(text)?, lua.registry_value::<Value>(decoded)?),
        _ => {
            let text = Value::String(lua.create_string(&*body.text)?);
            // A split answer keeps its items and the rest of it apart: `{ items, json }`.
            let decoded = match &body.items {
                Some(items) => {
                    let both = lua.create_table_with_capacity(2, 0)?;
                    both.raw_push(items_value(lua, &items.list)?)?;
                    both.raw_push(json_to_lua(lua, &items.rest)?)?;
                    Value::Table(both)
                }
                None => match serde_json::from_str::<serde_json::Value>(&body.text) {
                    Ok(v) => json_to_lua(lua, &v)?,
                    Err(_) => Value::Nil,
                },
            };
            r.value = Some((body.generation, Rc::new(lua.create_registry_value(text.clone())?), Rc::new(lua.create_registry_value(decoded.clone())?)));
            (text, decoded)
        }
    };
    out.raw_set("text", text)?;
    match (&body.items, decoded) {
        (Some(_), Value::Table(both)) => {
            out.raw_set("items", both.raw_get::<Value>(1)?)?;
            out.raw_set("json", both.raw_get::<Value>(2)?)?;
        }
        (_, decoded) => out.raw_set("json", decoded)?,
    }
    Ok(Value::Table(out))
}

/// An answer's listed items (`{ "data": [...] }` or `[...]`), each with its `id`, `type`,
/// `time.created` and JSON, and the rest of the answer; nil for an answer that lists none.
pub fn split_items(text: &str) -> Option<Items> {
    let mut v: serde_json::Value = serde_json::from_str(text).ok()?;
    let list = match &mut v {
        serde_json::Value::Array(list) => std::mem::take(list),
        serde_json::Value::Object(o) => match o.shift_remove("data") {
            Some(serde_json::Value::Array(list)) => list,
            _ => return None,
        },
        _ => return None,
    };
    let list = list
        .iter()
        .map(|m| Item {
            id: m["id"].as_str().unwrap_or("").to_owned(),
            kind: m["type"].as_str().unwrap_or("").to_owned(),
            created: m["time"]["created"].as_f64(),
            text: m.to_string(),
        })
        .collect();
    Some(Items { list, rest: v })
}

fn items_value(lua: &Lua, items: &[Item]) -> mlua::Result<Value> {
    let list = lua.create_table_with_capacity(items.len(), 0)?;
    for item in items {
        list.raw_push(record(lua, 4, |e| {
            e.str(c"id", &item.id);
            e.str(c"kind", &item.kind);
            if let Some(at) = item.created {
                e.num(c"created", at);
            }
            e.str(c"text", &item.text);
        })?)?;
    }
    Ok(Value::Table(list))
}

/// A memo version off a Luau value.
pub fn version_hash(v: &Value) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    match v {
        Value::Boolean(b) => (1u8, b).hash(&mut h),
        Value::Integer(i) => (2u8, (*i as f64).to_bits()).hash(&mut h),
        Value::Number(n) => (2u8, n.to_bits()).hash(&mut h),
        Value::String(s) => (3u8, s.as_bytes().to_vec()).hash(&mut h),
        _ => 0u8.hash(&mut h),
    }
    h.finish()
}

/// `encodeURIComponent`.
pub fn encode_uri(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.extend(['%', HEX[usize::from(b >> 4)] as char, HEX[usize::from(b & 15)] as char]);
        }
    }
    out
}

/// A random version 4 UUID (`crypto.randomUUID()`), from the system's secure source.
pub fn random_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // No secure source: the standard library's per-process random hasher keys instead.
        use std::hash::BuildHasher;
        for (i, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_usize(i);
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}
