//! Native Luau bindings for browser chrome Phase 1c features (`Find`, `StopFinding`, `FindResult`,
//! `ReloadIgnoreCache`, `ClearCacheAsync`, `Favicon`) and conversation app-link review draining.

use mlua::{Function, Lua, MultiValue, Table};
use rune::Tx;
use rune_luau::{Completer, Data, Host};
use rune::time::Instant;

use super::{App, DEVTOOLS_TIMEOUT, Dep, Vmx, cx, id_of, later, map, not_running, read_tab, scope, with_app};

pub(super) fn service_methods(lua: &Lua, t: &Table) -> mlua::Result<()> {
    t.raw_set(
        "TakeAppLinkReview",
        lua.create_function(|lua, ()| {
            cx::read(Dep::Browser);
            let next = with_app("Browser:TakeAppLinkReview", |app| {
                Ok(app.workspace_panes.browser.pending_app_links.borrow_mut().pop_front())
            })?;
            match next {
                Some((tab_id, url)) => mlua::IntoLua::into_lua(
                    map([("tabId", Data::String(tab_id.to_string())), ("url", Data::String(url))]),
                    lua,
                ),
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )
}

pub(super) fn asyncs(host: &Host<'_>, t: &Table) -> mlua::Result<()> {
    t.raw_set(
        "BrowserTab.ClearCacheAsync",
        host.async_function("BrowserTab:ClearCacheAsync", clear_cache)?,
    )
}

pub(super) fn tab_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.raw_set(
        "Favicon",
        lua.create_function(|lua, this: Table| {
            read_tab(lua, &this, "BrowserTab:Favicon", |app, id| {
                Ok(app.workspace_panes.browser.host.tab(id).and_then(|t| t.favicon.clone()))
            })
        })?,
    )?;
    m.raw_set(
        "FindResult",
        lua.create_function(|lua, this: Table| {
            let found = read_tab(lua, &this, "BrowserTab:FindResult", |app, id| {
                Ok(app
                    .workspace_panes
                    .browser
                    .host
                    .tab(id)
                    .and_then(|t| t.find.matches()))
            })?;
            match found {
                Some((active, total)) => mlua::IntoLua::into_lua(
                    map([("active", Data::Number(active as f64)), ("total", Data::Number(total as f64))]),
                    lua,
                ),
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )?;
    m.raw_set(
        "Find",
        lua.create_function(|lua, (this, query, forward, find_next): (Table, String, Option<bool>, Option<bool>)| {
            let forward = forward.unwrap_or(true);
            let find_next = find_next.unwrap_or(false);
            later(lua, &this, "BrowserTab:Find", move |s, id, _| {
                s.workspace_panes.browser.host.find(id, &query, forward, false, find_next);
            })
        })?,
    )?;
    m.raw_set(
        "StopFinding",
        lua.create_function(|lua, (this, clear_selection): (Table, Option<bool>)| {
            let clear = clear_selection.unwrap_or(true);
            later(lua, &this, "BrowserTab:StopFinding", move |s, id, _| {
                s.workspace_panes.browser.host.stop_find(id, clear);
            })
        })?,
    )?;
    m.raw_set(
        "ReloadIgnoreCache",
        lua.create_function(|lua, this: Table| {
            later(lua, &this, "BrowserTab:ReloadIgnoreCache", move |s, id, _| {
                s.workspace_panes.browser.host.clear_refusal(id);
                s.workspace_panes.browser.host.reload_ignore_cache(id);
            })
        })?,
    )?;
    let raw: Function = match lua.app_data_ref::<std::rc::Rc<Vmx>>() {
        Some(vmx) => vmx.asyncs.raw_get("BrowserTab.ClearCacheAsync")?,
        None => return Err(mlua::Error::runtime("the plugin host isn't set up")),
    };
    let call: Function = lua
        .load("local raw = ... return function(self) local r = raw(self.Id) if r and r.error then error(r.error, 0) end end")
        .set_name("=BrowserTab:ClearCacheAsync")
        .call(raw)?;
    m.raw_set("ClearCacheAsync", call)?;
    let _ = id_of;
    Ok(())
}

fn clear_cache(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (id,): (String,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let scope = scope::TabScope::capture(lua);
    with_app("BrowserTab:ClearCacheAsync", |app| {
        app.later(Box::new(move |s: &mut App, _: &mut Tx| {
            let failed = |message: String| map([("error", Data::String(message))]);
            let Some(id) = id.parse::<u64>().ok().filter(|&id| s.workspace_panes.browser.host.tab(id).is_some()) else {
                return done.complete(failed(format!("No browser tab with tabID {id}")));
            };
            if let Err(err) = scope.check_tab(s, id, true) {
                return done.complete(failed(err));
            }
            if !s.workspace_panes.browser.host.has_live_sidecar() {
                return done.complete(failed(not_running(&s.workspace_panes.browser.host.status())));
            }
            match s.workspace_panes.browser.host.clear_site_cache(id) {
                Ok(request) => {
                    s.workspace_panes.browser.devtools.insert(
                        request,
                        crate::surfaces::browser::PendingDevtools {
                            done,
                            deadline: Instant::now() + DEVTOOLS_TIMEOUT,
                            tab_id: id,
                            is_dialog: false,
                        },
                    );
                }
                Err(err) => done.complete(failed(err)),
            }
        }));
        Ok(())
    })
}
