//! App (the running app's facts) and System (the clipboard, the browser, the file manager and
//! the clock).

use std::rc::Rc;

use mlua::{Lua, Table};

use super::{lazy_signals, method, with_app};
use crate::plugin_host::cx::{self, Request};
use crate::plugin_host::vm::Vmx;

pub(crate) fn channel() -> opencode_plugin_spec::Channel {
    use opencode_plugin_spec::Channel;
    match crate::updater::build_channel() {
        "stable" | "prod" | "latest" => Channel::Prod,
        "beta" => Channel::Beta,
        "dev" => Channel::Dev,
        _ => Channel::Local,
    }
}

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, name: &str, t: &Table) -> mlua::Result<()> {
    let p = vmx.half.id.clone();
    if name == "App" {
        t.raw_set("Version", crate::updater::build_version())?;
        t.raw_set("Channel", channel().as_str())?;
        t.raw_set("Commit", crate::updater::build_commit())?;
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
    method(lua, t, "Now", |_, ()| with_app("System:Now", |app| Ok(*app.now_ms as f64)))?;
    method(lua, t, "UtcOffset", |_, ms: f64| Ok(f64::from(rune::LocalTime::at(ms as i64).offset)))?;
    method(lua, t, "FreePort", |_, ()| Ok(std::net::TcpListener::bind("127.0.0.1:0").ok().and_then(|l| l.local_addr().ok()).map(|a| a.port())))?;
    method(lua, t, "RandomId", |_, ()| Ok(random_id()))
}

/// A random version 4 UUID (`crypto.randomUUID()`), from the system's secure source.
pub fn random_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // No secure source: the standard library's per-process random hasher keys instead.
        use std::hash::{BuildHasher, Hasher};
        for (i, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_usize(i);
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}
