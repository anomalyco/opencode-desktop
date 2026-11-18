//! Pure Luau / `mlua` conversion and method-binding helpers shared across plugin services.

use mlua::{Lua, Table, Value};
use rune_luau::Data;

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
