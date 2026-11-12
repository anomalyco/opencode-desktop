//! `Office`: Word, Excel and PowerPoint files opened and laid out by the host (`crate::office`),
//! each on a thread of its own, for the Microsoft Office plugin's viewers to draw with
//! `ui.officePages` and `ui.officeSheet`.

use std::rc::Rc;

use mlua::{Lua, MultiValue, Table};
use rune_luau::{Completer, Data, Host};

use super::{async_method, map, method};
use crate::App;
use crate::office::registry;
use crate::plugin_host::cx::{self, Dep};
use crate::plugin_host::plugin::Half;
use crate::plugin_host::vm::Vmx;

/// Every Office Open XML file is a zip archive.
const ZIP: &[u8] = &[0x50, 0x4b, 0x03, 0x04];
/// A Compound File: an encrypted Office file, or one saved in the binary .doc, .xls or .ppt formats.
const COMPOUND: &[u8] = &[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    async_method(lua, vmx, t, "OpenAsync", "Office.OpenAsync")?;
    method(lua, t, "Version", |_, doc: Table| {
        let id = document_id(&doc)?;
        cx::read(Dep::Office(id));
        Ok(registry::version(id) as f64)
    })?;
    let plugin = vmx.half.id.clone();
    method(lua, t, "SetSheet", move |_, (doc, index): (Table, i64)| {
        registry::set_sheet(document_id(&doc)?, usize::try_from(index - 1).unwrap_or(0));
        cx::ask(&plugin, cx::Request::Redraw);
        Ok(())
    })?;
    method(lua, t, "Sheet", |_, doc: Table| {
        let id = document_id(&doc)?;
        cx::read(Dep::Office(id));
        Ok(registry::sheet(id) as i64 + 1)
    })?;
    method(lua, t, "Close", |_, doc: Table| {
        registry::close(document_id(&doc)?);
        Ok(())
    })
}

pub fn asyncs(host: &Host<'_>, t: &Table, _half: &Rc<Half>) -> mlua::Result<()> {
    t.raw_set("Office.OpenAsync", host.async_function("Office:OpenAsync", open)?)
}

/// An `OfficeDocument` record's id.
pub fn document_id(doc: &Table) -> mlua::Result<u64> {
    doc.raw_get::<Option<i64>>("id")?.and_then(|id| u64::try_from(id).ok()).ok_or_else(|| mlua::Error::runtime("not an OfficeDocument"))
}

/// `Office:OpenAsync(path, directory, kind)`: reads the file through the window's server (from
/// disk for a window without one), then opens it when it is a zip archive.
fn open(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (path, directory, kind): (String, String, String) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let kind = match kind.as_str() {
        "docx" => office_preview::Kind::Document,
        "xlsx" => office_preview::Kind::Spreadsheet,
        "pptx" => office_preview::Kind::Presentation,
        other => return Err(mlua::Error::runtime(format!("unknown OfficeKind {other:?}"))),
    };
    let app = cx::app().ok_or_else(|| mlua::Error::runtime("Office:OpenAsync needs a window"))?;
    let waker = (*app.waker).clone();
    let opened = move |bytes: Result<Vec<u8>, String>| {
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(e) => return done.complete(answer(0, "unreadable", Some(&e))),
        };
        let size = bytes.len();
        if bytes.starts_with(ZIP) {
            registry::open(kind, bytes, size, waker, done);
        } else {
            done.complete(answer(size, if bytes.starts_with(COMPOUND) { "compound" } else { "other" }, None));
        }
    };
    if !app.live() {
        let file = std::path::Path::new(&directory).join(&path);
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()));
        opened(bytes);
        return Ok(());
    }
    let url = format!("/api/fs/read/{}?location%5Bdirectory%5D={}", crate::review::data::encode_fs_path(&relative(&path, &directory)), crate::switcher::encode(&directory));
    app.job(move |srv| {
        let bytes = srv.get_bytes(&url);
        Box::new(move |_: &mut App, _: &mut rune::Tx| opened(bytes)) as crate::jobs::Apply
    });
    Ok(())
}

fn answer(size: usize, signature: &str, error: Option<&str>) -> Data {
    map([("size", Data::Integer(size as i64)), ("signature", Data::String(signature.to_owned())), ("error", error.map_or(Data::Nil, |e| Data::String(e.to_owned())))])
}

/// `path` under `directory` with forward slashes, as the server's file routes take it.
fn relative(path: &str, directory: &str) -> String {
    let (path, directory) = (path.replace('\\', "/"), directory.replace('\\', "/"));
    path.strip_prefix(directory.trim_end_matches('/')).filter(|r| r.starts_with('/')).map_or(path.clone(), |r| r.trim_start_matches('/').to_owned())
}
