//! Desktop `Power` Luau service binding delegating to `opencode_plugin_runtime::power`.

use std::rc::Rc;

use mlua::{Lua, Table};

pub use opencode_plugin_runtime::power::release;

use super::Held;
use crate::plugin_host::vm::Vmx;

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let acquire_v = vmx.clone();
    let release_half = Rc::downgrade(&vmx.half);
    opencode_plugin_runtime::power::build(
        lua,
        t,
        move |id| acquire_v.half.held.borrow_mut().push((acquire_v.vm.get(), Held::Power(id))),
        move |id| {
            if let Some(h) = release_half.upgrade() {
                h.held.borrow_mut().retain(|(_, held)| *held != Held::Power(id));
            }
        },
    )
}
