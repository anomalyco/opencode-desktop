//! The `Composer` Luau service exposed alongside `Browser` (`plugin_host::native::browser`).

use mlua::{Lua, Table, Value};
use rune::Tx;

use super::super::super::cx::{self, Dep};
use super::super::super::services::{lazy_signals, method, with_app};
use super::super::super::vm::Vmx;
use crate::App;

/// The `Composer` service.
pub fn composer(lua: &Lua, vmx: &std::rc::Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "InsertPill", |_, (id, label): (String, String)| {
        with_app("Composer:InsertPill", |app| {
            app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if !s.composer.text.is_empty() && !s.composer.text.ends_with(char::is_whitespace) {
                    s.composer.insert(" ");
                }
                s.composer.insert_pill(id, &label);
                s.composer.insert(" ");
                s.composer.focused = true;
                s.kernel.slots.get_mut(&crate::panels::SIDE).focus.blur();
                super::super::super::blur_fields(s);
            }));
            Ok(())
        })
    })?;
    method(lua, t, "Active", |_, ()| {
        cx::read(Dep::Composer);
        with_app("Composer:Active", |app| Ok(app.has_composer()))
    })?;
    method(lua, t, "Directory", |_, ()| {
        cx::read(Dep::Composer);
        with_app("Composer:Directory", |app| Ok(app.composer_directory()))
    })?;
    method(lua, t, "Model", |lua, ()| {
        cx::read(Dep::Composer);
        with_app("Composer:Model", |app| {
            use crate::pickers::PickerHost as _;
            let selection = app.selection();
            let found = app
                .pickers()
                .models
                .iter()
                .find(|m| m.provider == selection.model.provider && m.id == selection.model.id);
            match found {
                Some(m) => {
                    let out = lua.create_table_with_capacity(0, 3)?;
                    out.raw_set("providerId", m.provider.as_str())?;
                    out.raw_set("modelId", m.id.as_str())?;
                    out.raw_set("name", m.name.as_str())?;
                    Ok(Value::Table(out))
                }
                None => Ok(Value::Nil),
            }
        })
    })?;
    method(lua, t, "Picker", |_, ()| {
        cx::read(Dep::Composer);
        with_app("Composer:Picker", |app| Ok(app.composer_ext.pickers.open().map(|k| k.key().to_owned())))
    })?;
    lazy_signals(lua, vmx, t, "Composer", &["PickerChanged"])
}
