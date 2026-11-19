//! Desktop `Process` service adapter over `opencode_plugin_runtime::process::service`.

use std::rc::Rc;

use mlua::{Lua, Table, Value};
use rune_luau::{Data, Host};

use crate::plugin_host::cx;
use crate::plugin_host::plugin::Half;
use crate::plugin_host::vm::Vmx;

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v_vm = vmx.clone();
    opencode_plugin_runtime::process::service::build(
        lua,
        t,
        vmx.half.procs.clone(),
        move || v_vm.vm.get(),
        vmx.script_signal.clone(),
        vmx.asyncs.clone(),
        || cx::with_app(|app| app.plugins.waker.clone()).flatten(),
    )
}

pub fn asyncs(host: &Host<'_>, t: &Table, half: &Rc<Half>) -> mlua::Result<()> {
    opencode_plugin_runtime::process::service::asyncs(host, t, &half.procs, |lua| {
        lua.app_data_ref::<Rc<Vmx>>().map(|v| v.vm.get())
    })
}

pub fn drain(half: &Half, vm: u64) -> Vec<(Value, Data)> {
    half.procs.drain(vm)
}

pub fn reap(half: &Half, keep_vm: Option<u64>) {
    half.procs.reap(keep_vm);
}
