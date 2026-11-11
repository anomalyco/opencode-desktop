//! `Updater`: the app's own updates (`crate::updater`), as the Updater plugin draws them: where
//! they stand, a check, and the install.

use std::rc::Rc;

use mlua::{Lua, MultiValue, Table};
use rune_luau::{Completer, Host};

use super::{app_now, async_method, lazy_signals, method};
use crate::plugin_host::cx::{self, Dep};
use crate::plugin_host::plugin::Half;
use crate::plugin_host::vm::Vmx;

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "State", |lua, ()| {
        cx::read(Dep::Updater);
        mlua::IntoLua::into_lua(crate::updater::state_data(app_now("Updater:State")?), lua)
    })?;
    async_method(lua, vmx, t, "CheckAsync", "Updater.CheckAsync")?;
    async_method(lua, vmx, t, "InstallAsync", "Updater.InstallAsync")?;
    lazy_signals(lua, vmx, t, "Updater", &["StateChanged"])
}

pub fn asyncs(host: &Host<'_>, t: &Table, _half: &Rc<Half>) -> mlua::Result<()> {
    t.raw_set("Updater.CheckAsync", host.async_function("Updater:CheckAsync", check)?)?;
    t.raw_set("Updater.InstallAsync", host.async_function("Updater:InstallAsync", install)?)
}

fn check(_: &Lua, _: MultiValue, done: Completer) -> mlua::Result<()> {
    crate::updater::check_async(app_now("Updater:CheckAsync")?, done);
    Ok(())
}

fn install(_: &Lua, _: MultiValue, done: Completer) -> mlua::Result<()> {
    crate::updater::install_async(app_now("Updater:InstallAsync")?, done);
    Ok(())
}
