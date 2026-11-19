//! Desktop `I18n` service adapter over `opencode_plugin_runtime::i18n`.

use std::rc::Rc;

use mlua::{Lua, Table};

use super::lazy_signals;
use crate::plugin_host::cx::{self, Dep};
use crate::plugin_host::plugin::Kind;
use crate::plugin_host::vm::Vmx;

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    opencode_plugin_runtime::i18n::build(
        lua,
        t,
        vmx.half.catalogs.clone(),
        vmx.half.kind == Kind::Window,
        || cx::read(Dep::Locale),
    )?;
    lazy_signals(lua, vmx, t, "I18n", &["Changed"])
}

/// The interface language changed: every plugin of the window forgets the strings it
/// remembered, and its surfaces draw again.
pub fn locale_changed(app: &crate::App) {
    for (_, p) in app.plugins.running() {
        if let Some(script) = p.script() {
            opencode_plugin_runtime::i18n::clear_vm(script);
        }
    }
    crate::plugin_host::bump();
}
