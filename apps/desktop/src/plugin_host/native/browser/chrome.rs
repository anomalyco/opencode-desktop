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
    )?;
    t.raw_set(
        "BrowserTab.ReadFileAsync",
        host.async_function("BrowserTab:ReadFileAsync", read_file)?,
    )
}

/// A downloaded file as the plugin sees it: Desktop's `files.list` fields (no path).
pub(super) fn file_json(f: &browser_pane::DownloadFile) -> serde_json::Value {
    let state = match f.state {
        browser_pane::DownloadState::Pending => "pending",
        browser_pane::DownloadState::Completed => "completed",
        browser_pane::DownloadState::Failed => "failed",
    };
    serde_json::json!({ "id": f.id, "name": f.name, "mime": f.mime, "bytes": f.bytes, "state": state, "error": f.error })
}

/// Reads a completed download of the page, off the UI thread: its text when it's UTF-8, else
/// its bytes in base64, as Desktop's `files.get` hands a file over; at most 5 MiB, its limit.
fn read_file(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    const MAX: u64 = 5 * 1024 * 1024;
    let (id, file_id): (String, String) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let scope = scope::TabScope::capture(lua);
    with_app("BrowserTab:ReadFileAsync", |app| {
        app.later(Box::new(move |s: &mut App, _: &mut Tx| {
            let failed = |message: String| map([("error", Data::String(message))]);
            let Some(tab_id) = id.parse::<u64>().ok().filter(|&id| s.workspace_panes.browser.host.tab(id).is_some()) else {
                return done.complete(failed(format!("No browser tab with tabID {id}")));
            };
            if let Err(err) = scope.check_tab(s, tab_id, true) {
                return done.complete(failed(err));
            }
            let tab = s.workspace_panes.browser.host.tab(tab_id);
            let Some(file) = tab.and_then(|t| t.files.iter().find(|f| f.id == file_id)).cloned() else {
                return done.complete(failed(format!("No downloaded file {file_id} in browser tab {id}: list them with browser_files")));
            };
            match file.state {
                browser_pane::DownloadState::Pending => return done.complete(failed(format!("{} is still downloading: check browser_files again", file.name))),
                browser_pane::DownloadState::Failed => return done.complete(failed(format!("{} didn't download ({})", file.name, file.error.as_deref().unwrap_or("failed")))),
                browser_pane::DownloadState::Completed => {}
            }
            // Opened only where the engine keeps downloads, never at a path the engine merely named.
            let Some(path) = s.workspace_panes.browser.host.download_path(tab_id, &file) else {
                return done.complete(failed(format!("Couldn't read {}: it isn't in the browser's downloads folder", file.name)));
            };
            rune_luau::spawn_blocking(move || {
                use base64::Engine as _;
                use std::io::Read as _;
                let mut bytes = Vec::new();
                let read = std::fs::File::open(&path).and_then(|f| f.take(MAX + 1).read_to_end(&mut bytes));
                if let Err(e) = read {
                    return done.complete(failed(format!("Couldn't read {}: {e}", file.name)));
                }
                if bytes.len() as u64 > MAX {
                    return done.complete(failed(format!("{} is over the 5 MiB limit", file.name)));
                }
                let size = Data::Integer(bytes.len() as i64);
                let body = match String::from_utf8(bytes) {
                    Ok(text) => ("text", Data::String(text)),
                    Err(e) => ("base64", Data::String(base64::engine::general_purpose::STANDARD.encode(e.into_bytes()))),
                };
                done.complete(map([("name", Data::String(file.name)), ("mime", Data::String(file.mime)), ("bytes", size), body]));
            });
        }));
        Ok(())
    })
}

pub(super) fn tab_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.raw_set(
        "IsPrivate",
        lua.create_function(|lua, this: Table| {
            read_tab(lua, &this, "BrowserTab:IsPrivate", |app, id| {
                Ok(app
                    .workspace_panes
                    .browser
                    .host
                    .tab(id)
                    .is_some_and(|t| t.context.is_isolated() || t.preview.is_some()))
            })
        })?,
    )?;
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
