//! Native Luau bindings for Settings > Browser's sign-in controls over `#286`'s profile cookie
//! store (`browser_pane::cookies::host`). Only the bundled browser plugin's own settings page may
//! call them: never a third-party plugin, and never from an MCP tool's coroutine.

use mlua::{Function, Lua, MultiValue, Table};
use rune::Tx;
use rune_luau::{Completer, Host};

use super::{App, Dep, Vmx, cx, method, scope::TabScope, with_app};

fn require_settings_caller(lua: &Lua, what: &str) -> mlua::Result<()> {
    let scope = TabScope::capture(lua);
    if scope.is_builtin_browser && scope.tool_sid.is_none() && scope.in_flight.is_empty() {
        return Ok(());
    }
    Err(mlua::Error::runtime(format!("{what}: browser.cookies.denied")))
}

pub(super) fn service_methods(lua: &Lua, vmx: &std::rc::Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    // `settings.tsx:208-209`: the switch shows the profile's state, and is disabled while it is
    // unknown (an engine without the `#286` cookie store). While the engine is idle, the persisted
    // preference stays live and `BrowserHost` sends `SetShareSignIns` on `Ready { cookies: true }`.
    method(lua, t, "ShareSignIns", |lua, ()| {
        require_settings_caller(lua, "Browser:ShareSignIns")?;
        cx::read(Dep::Browser);
        with_app("Browser:ShareSignIns", |app| {
            let host = &app.workspace_panes.browser.host;
            Ok((!host.engine_caps_known() || host.engine_cookies()).then(|| host.share_sign_ins()))
        })
    })?;
    // `profile/index.ts:657-661`: turning it off stops every mirror and removes the shared cookies
    // from the remote partitions; the engine does both (`#286` `SetShareSignIns`).
    method(lua, t, "SetShareSignIns", |lua, enabled: bool| {
        require_settings_caller(lua, "Browser:SetShareSignIns")?;
        with_app("Browser:SetShareSignIns", |app| {
            app.later(Box::new(move |s: &mut App, _: &mut Tx| s.workspace_panes.browser.host.set_share_sign_ins(enabled)));
            Ok(())
        })
    })?;
    // `settings.tsx:266` `settings.cookies.count`: the profile's cookies while the engine runs, else nil
    // (`cookie_import` reads them again once the count is a few seconds old or the store changed).
    method(lua, t, "CookieCount", |lua, ()| {
        require_settings_caller(lua, "Browser:CookieCount")?;
        cx::read(Dep::Browser);
        with_app("Browser:CookieCount", |app| Ok(app.browser_cookie_count()))
    })?;
    // `Browser:ClearProfileDataAsync()`: every cookie and the open sites' stored data (`cookie_import`).
    let raw: Function = vmx.asyncs.raw_get("Browser.ClearProfileDataAsync")?;
    let call: Function = lua
        .load("local raw = ... return function(self) return raw() end")
        .set_name("=Browser:ClearProfileDataAsync")
        .call(raw)?;
    t.raw_set("ClearProfileDataAsync", call)?;
    // `Browser:ImportCookieFileAsync(title)`: the app shows the picker itself (`cookie_import`).
    let raw: Function = vmx.asyncs.raw_get("Browser.ImportCookieFileAsync")?;
    let call: Function = lua
        .load("local raw = ... return function(self, title) return raw(title) end")
        .set_name("=Browser:ImportCookieFileAsync")
        .call(raw)?;
    t.raw_set("ImportCookieFileAsync", call)?;
    // `Browser:ImportSignInsAsync(source, profile, sites?)`: another browser's sign-ins (`cookie_import`).
    let raw: Function = vmx.asyncs.raw_get("Browser.ImportSignInsAsync")?;
    let call: Function = lua
        .load("local raw = ... return function(self, source, profile, sites) return raw(source, profile, sites) end")
        .set_name("=Browser:ImportSignInsAsync")
        .call(raw)?;
    t.raw_set("ImportSignInsAsync", call)?;
    Ok(())
}

pub(super) fn asyncs(host: &Host<'_>, t: &Table) -> mlua::Result<()> {
    t.raw_set("Browser.ImportCookieFileAsync", host.async_function("Browser:ImportCookieFileAsync", import_cookie_file)?)?;
    t.raw_set("Browser.ImportSignInsAsync", host.async_function("Browser:ImportSignInsAsync", import_sign_ins)?)?;
    t.raw_set("Browser.ClearProfileDataAsync", host.async_function("Browser:ClearProfileDataAsync", clear_profile_data)?)
}

fn clear_profile_data(lua: &Lua, _: MultiValue, done: Completer) -> mlua::Result<()> {
    require_settings_caller(lua, "Browser:ClearProfileDataAsync")?;
    with_app("Browser:ClearProfileDataAsync", |app| {
        app.later(Box::new(move |s: &mut App, _: &mut Tx| s.start_profile_clear(done)));
        Ok(())
    })
}

fn import_cookie_file(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (title,): (String,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    require_settings_caller(lua, "Browser:ImportCookieFileAsync")?;
    with_app("Browser:ImportCookieFileAsync", |app| {
        app.later(Box::new(move |s: &mut App, tx: &mut Tx| s.start_cookie_file_import(title, done, tx)));
        Ok(())
    })
}

fn import_sign_ins(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (source, profile, sites): (String, String, Option<Vec<String>>) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    require_settings_caller(lua, "Browser:ImportSignInsAsync")?;
    with_app("Browser:ImportSignInsAsync", |app| {
        app.later(Box::new(move |s: &mut App, _: &mut Tx| s.start_browser_import(source, profile, sites, done)));
        Ok(())
    })
}