//! App (the running app's facts) and System (the clipboard, the browser, the file manager and
//! the clock).

use std::rc::Rc;

use mlua::{Lua, Table};

use super::{app_now, lazy_signals, method};
use crate::plugin_host::luau::cx::{self, Request};
use crate::plugin_host::luau::vm::Vmx;

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, name: &str, t: &Table) -> mlua::Result<()> {
    let p = vmx.half.id.clone();
    if name == "App" {
        t.raw_set("Version", crate::updater::build_version())?;
        t.raw_set(
            "Channel",
            match crate::updater::build_channel() {
                "prod" | "latest" => "prod",
                "beta" => "beta",
                _ => "local",
            },
        )?;
        t.raw_set("Platform", if cfg!(target_os = "windows") { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" })?;
        method(lua, t, "Locale", |_, ()| {
            cx::read(cx::Dep::Locale);
            Ok(opencode_i18n::locale().id())
        })?;
        method(lua, t, "Arg", |_, name: String| Ok(crate::arg(&format!("--{name}"))))?;
        method(lua, t, "LogsDir", |_, ()| Ok(crate::dirs::data().map(|d| d.join("logs").display().to_string())))?;
        method(lua, t, "DownloadsDir", |_, ()| Ok(crate::dirs::downloads().map(|d| d.display().to_string())))?;
        return lazy_signals(lua, vmx, t, "App", &["LocaleChanged"]);
    }
    let q = p.clone();
    method(lua, t, "CopyText", move |_, text: String| {
        cx::ask(&q, Request::CopyText(text));
        Ok(())
    })?;
    let q = p.clone();
    method(lua, t, "OpenExternal", move |_, url: String| {
        cx::ask(&q, Request::OpenExternal(url));
        Ok(())
    })?;
    method(lua, t, "Reveal", move |_, path: String| {
        cx::ask(&p, Request::Reveal(path));
        Ok(())
    })?;
    method(lua, t, "Now", |_, ()| Ok(app_now("System:Now")?.now_ms as f64))?;
    method(lua, t, "UtcOffset", |_, ms: f64| Ok(f64::from(rune::LocalTime::at(ms as i64).offset)))
}
