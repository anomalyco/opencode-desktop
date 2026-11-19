//! Desktop `Askpass` service adapter over `opencode_plugin_runtime::askpass`.

use std::rc::Rc;

use mlua::{Lua, Table};
use rune_luau::Data;

pub use opencode_plugin_runtime::askpass::close;

use super::{Held, signal};
use crate::plugin_host::cx;
use crate::plugin_host::plugin::Half;
use crate::plugin_host::vm::Vmx;

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v_vm = vmx.clone();
    let v_sig = vmx.clone();
    let v_held = vmx.clone();
    opencode_plugin_runtime::askpass::build(
        lua,
        t,
        opencode_plugin_runtime::askpass::Bindings {
            owner: vmx.half.token.clone(),
            get_vm: Box::new(move || v_vm.vm.get()),
            script_signal: vmx.script_signal.clone(),
            get_waker: || cx::with_app(|app| app.plugins.waker.clone()).flatten(),
            make_signal: Box::new(move |lua, name| signal(lua, &v_sig, name)),
            on_listen: Box::new(move |vm, id| v_held.half.held.borrow_mut().push((vm, Held::Askpass(id)))),
        },
    )
}

/// The prompts that came and went for the listeners of `half`'s VM `vm` since the last poll.
pub fn drain(half: &Rc<Half>, vm: u64) -> Vec<(String, Data)> {
    opencode_plugin_runtime::askpass::drain(&half.token, vm)
}
