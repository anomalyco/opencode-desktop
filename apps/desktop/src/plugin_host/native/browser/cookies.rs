//! Native Luau bindings for Settings > Browser's sign-in controls over `#286`'s profile cookie
//! store (`browser_pane::cookies::host`). Only the bundled browser plugin's own settings page may
//! call them: never a third-party plugin, and never from an MCP tool's coroutine.

use mlua::{Lua, Table};
use rune::Tx;

use super::{App, Dep, cx, method, scope::TabScope, with_app};

fn require_settings_caller(lua: &Lua, what: &str) -> mlua::Result<()> {
    let scope = TabScope::capture(lua);
    if scope.is_builtin_browser && scope.tool_sid.is_none() && scope.in_flight.is_empty() {
        return Ok(());
    }
    Err(mlua::Error::runtime(format!("{what}: browser.cookies.denied")))
}

pub(super) fn service_methods(lua: &Lua, t: &Table) -> mlua::Result<()> {
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
    Ok(())
}
