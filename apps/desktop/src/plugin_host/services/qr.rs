//! Desktop `Qr` Luau service binding delegating to `opencode_plugin_runtime::qr`.

use std::rc::Rc;

use mlua::{Lua, Table};

use crate::plugin_host::vm::Vmx;

pub fn build(lua: &Lua, _: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    opencode_plugin_runtime::qr::build(lua, t)
}
