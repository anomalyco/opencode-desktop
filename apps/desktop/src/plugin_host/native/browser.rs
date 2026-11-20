//! `Browser`, `Composer` and `ui.browserSurface`: the window's pages as the app's embedded
//! browser engine (`crate::browser`, `browser_pane`) runs them, which starts and downloads the
//! engine, composites each page's frames, forwards input and carries DevTools calls. The browser
//! plugin keeps the tabs, their titles, the address bar, history, the element picker's notes,
//! the commands and the MCP tools.

#[cfg(not(target_arch = "wasm32"))]
use std::hash::{Hash, Hasher};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use rune::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use browser_pane::EngineStatus;
#[cfg(not(target_arch = "wasm32"))]
use mlua::{Function, Lua, MultiValue, Table, Value};
#[cfg(not(target_arch = "wasm32"))]
use rune::{El, Ime, Tx, col};
#[cfg(not(target_arch = "wasm32"))]
use rune_luau::{Completer, Data, Host, NodeArgs};
#[cfg(not(target_arch = "wasm32"))]
mod chrome;
mod composer;
pub use composer::composer;
#[cfg(not(target_arch = "wasm32"))]
mod history;
#[cfg(not(target_arch = "wasm32"))]
mod scope;
#[cfg(not(target_arch = "wasm32"))]
mod viewport;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{asyncs, build, ime, pages, version, view};

#[cfg(not(target_arch = "wasm32"))]
use super::super::cx::{self, Dep};
#[cfg(not(target_arch = "wasm32"))]
use super::super::services::{json_to_data, lazy_signals, map, method, with_app};
#[cfg(not(target_arch = "wasm32"))]
use super::super::vm::Vmx;
#[cfg(not(target_arch = "wasm32"))]
use super::{handle_field, object, p};
#[cfg(not(target_arch = "wasm32"))]
use crate::App;

/// How long a DevTools call may take before it fails.
#[cfg(not(target_arch = "wasm32"))]
const DEVTOOLS_TIMEOUT: Duration = Duration::from_secs(10);

/// `Browser:Prepare`: how long the engine it starts waits for a page by default, and at most.
#[cfg(not(target_arch = "wasm32"))]
const PREPARE_HOLD_S: f64 = 5.0;
#[cfg(not(target_arch = "wasm32"))]
const PREPARE_HOLD_MAX_S: f64 = 60.0;

/// The `Browser` service.
#[cfg(not(target_arch = "wasm32"))]
pub fn build(lua: &Lua, vmx: &std::rc::Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "Tabs", |lua, sid: Option<String>| {
        cx::read(Dep::Browser);
        let scope = scope::TabScope::capture(lua);
        let ids: Vec<u64> = with_app("Browser:Tabs", |app| {
            let scoped = if scope.tool_sid.is_some() || !scope.is_builtin_browser {
                let Some(target) = scope.target_session(app, sid.as_deref()) else {
                    return Ok(Vec::new());
                };
                Some(target)
            } else {
                sid
            };
            Ok(match scoped {
                Some(sid) => app.workspace_panes.browser.session_tabs.get(&sid).map(|ids| ids.iter().copied().filter(|&id| app.workspace_panes.browser.host.tab(id).is_some()).collect()).unwrap_or_default(),
                None => app.workspace_panes.browser.host.tabs().iter().map(|t| t.id).collect(),
            })
        })?;
        let out = lua.create_table()?;
        for (n, id) in ids.into_iter().enumerate() {
            out.raw_set(n + 1, tab(lua, id)?)?;
        }
        Ok(out)
    })?;
    method(lua, t, "Get", |lua, id: String| {
        cx::read(Dep::Browser);
        let scope = scope::TabScope::capture(lua);
        let found = with_app("Browser:Get", |app| {
            Ok(id.parse::<u64>().ok().filter(|&id| {
                app.workspace_panes.browser.host.tab(id).is_some() && scope.check_tab(app, id, false).is_ok()
            }))
        })?;
        match found {
            Some(id) => Ok(Value::Table(tab(lua, id)?)),
            None => Ok(Value::Nil),
        }
    })?;
    method(lua, t, "List", |lua, (_sid, open): (String, Vec<String>)| {
        cx::read(Dep::Browser);
        let scope = scope::TabScope::capture(lua);
        let rows = with_app("Browser:List", |app| {
            Ok(open
                .iter()
                .map(|raw| {
                    let page = raw
                        .parse::<u64>()
                        .ok()
                        .filter(|&id| scope.check_tab(app, id, false).is_ok())
                        .and_then(|id| app.workspace_panes.browser.host.tab(id));
                    map([
                        ("id", Data::String(raw.clone())),
                        ("url", Data::String(page.map(|t| t.url.clone()).filter(|u| u != "about:blank").unwrap_or_default())),
                        ("title", Data::String(page.map(|t| t.title.clone()).unwrap_or_default())),
                        ("open", Data::Bool(page.is_some())),
                    ])
                })
                .collect())
        })?;
        super::diff::lua_data(lua, Data::List(rows))
    })?;
    method(lua, t, "Active", |lua, sid: String| {
        cx::read(Dep::Browser);
        let scope = scope::TabScope::capture(lua);
        let found = with_app("Browser:Active", |app| {
            let Some(sid) = scope.target_session(app, Some(&sid)) else { return Ok(None) };
            Ok(app.default_browser_tab_for_session(&sid).filter(|&id| {
                app.workspace_panes.browser.host.tab(id).is_some() && scope.check_tab(app, id, false).is_ok()
            }))
        })?;
        match found {
            Some(id) => Ok(Value::Table(tab(lua, id)?)),
            None => Ok(Value::Nil),
        }
    })?;
    method(lua, t, "Selected", |lua, sid: Option<String>| {
        cx::read(Dep::Browser);
        let scope = scope::TabScope::capture(lua);
        let found = with_app("Browser:Selected", |app| {
            let Some(sid) = scope.target_session(app, sid.as_deref()) else { return Ok(None) };
            let open = app.kernel.slots.get(&crate::panels::SIDE).sessions.get(&sid).is_some_and(|st| st.visibility.open()) && !app.narrow();
            Ok(app.active_browser_tab_for_session(&sid).filter(|&id| {
                open && app.workspace_panes.browser.host.tab(id).is_some() && scope.check_tab(app, id, false).is_ok()
            }))
        })?;
        match found {
            Some(id) => Ok(Value::Table(tab(lua, id)?)),
            None => Ok(Value::Nil),
        }
    })?;
    let plugin = vmx.half.id.clone();
    method(lua, t, "Focused", move |lua, ()| {
        cx::read(Dep::Browser);
        let scope = scope::TabScope::capture(lua);
        let found = with_app("Browser:Focused", |app| {
            let field = super::super::editors::plugin_focused(app, &plugin);
            Ok(app
                .focused_browser_pane(field)
                .and(app.active_browser_tab_id())
                .filter(|&id| scope.check_tab(app, id, false).is_ok()))
        })?;
        match found {
            Some(id) => Ok(Value::Table(tab(lua, id)?)),
            None => Ok(Value::Nil),
        }
    })?;
    method(lua, t, "Available", |_, ()| {
        cx::read(Dep::Browser);
        with_app("Browser:Available", |app| Ok(!app.browser_unsupported()))
    })?;
    method(lua, t, "Engine", |lua, ()| {
        cx::read(Dep::Browser);
        with_app("Browser:Engine", |app| {
            let (state, message) = engine(app);
            mlua::IntoLua::into_lua(
                map([
                    ("state", Data::String(state.into())),
                    ("message", message.map_or(Data::Nil, Data::String)),
                    ("downloads", Data::Bool(app.workspace_panes.browser.host.engine_downloads())),
                    ("policy", Data::Bool(app.workspace_panes.browser.host.engine_policy())),
                    ("find", Data::Bool(app.workspace_panes.browser.host.engine_find())),
                    ("permissionSync", Data::Bool(app.workspace_panes.browser.host.engine_permission_sync())),
                    ("clearCache", Data::Bool(app.workspace_panes.browser.host.engine_policy())),
                    ("hardReload", Data::Bool(true)),
                ]),
                lua,
            )
        })
    })?;
    method(lua, t, "NormalizeUrl", |lua, (url, sid): (String, Option<String>)| {
        let scope = scope::TabScope::capture(lua);
        let roots = with_app("Browser:NormalizeUrl", |app| {
            if app.workspace_panes.browser.host.has_live_sidecar() && !app.workspace_panes.browser.host.engine_policy() {
                return Ok(Vec::new());
            }
            let target = scope.target_session(app, sid.as_deref());
            Ok(app.browser_file_roots(target.as_deref()))
        })?;
        browser_pane::normalize_url(&url, &roots).map_err(mlua::Error::runtime)
    })?;
    method(lua, t, "Prepare", |_, seconds: Option<f64>| {
        let hold = Duration::from_secs_f64(seconds.unwrap_or(PREPARE_HOLD_S).clamp(1.0, PREPARE_HOLD_MAX_S));
        with_app("Browser:Prepare", |app| {
            app.later(Box::new(move |s: &mut App, _: &mut Tx| {
                if !s.use_mock_browser() && !s.browser_unsupported() {
                    s.workspace_panes.browser.host.prepare(hold);
                }
            }));
            Ok(())
        })
    })?;
    chrome::service_methods(lua, t)?;
    history::service_methods(lua, t)?;
    let raw: Function = vmx.asyncs.raw_get("Browser.OpenAsync")?;
    let open: Function = lua.load("local raw = ... return function(self, spec) local id = raw(spec) return if id then self:Get(id) else nil end").set_name("=Browser:OpenAsync").call(raw)?;
    t.raw_set("OpenAsync", open)?;
    lazy_signals(lua, vmx, t, "Browser", &["Changed", "Picked"])
}

/// The host's async functions: `Browser:OpenAsync` and `BrowserTab`'s async methods.
#[cfg(not(target_arch = "wasm32"))]
pub fn asyncs(host: &Host<'_>, t: &Table) -> mlua::Result<()> {
    t.raw_set("Browser.OpenAsync", host.async_function("Browser:OpenAsync", open)?)?;
    t.raw_set("BrowserTab.DevtoolsAsync", host.async_function("BrowserTab:DevtoolsAsync", devtools)?)?;
    t.raw_set("BrowserTab.SiteCookiesAsync", host.async_function("BrowserTab:SiteCookiesAsync", site_cookies)?)?;
    t.raw_set("BrowserTab.ClearSiteAsync", host.async_function("BrowserTab:ClearSiteAsync", clear_site)?)?;
    t.raw_set("BrowserTab.ReadFileAsync", host.async_function("BrowserTab:ReadFileAsync", read_file)?)?;
    chrome::asyncs(host, t)
}

/// A downloaded file as the plugin sees it: Desktop's `files.list` fields (no path).
#[cfg(not(target_arch = "wasm32"))]
fn file_json(f: &browser_pane::DownloadFile) -> serde_json::Value {
    let state = match f.state {
        browser_pane::DownloadState::Pending => "pending",
        browser_pane::DownloadState::Completed => "completed",
        browser_pane::DownloadState::Failed => "failed",
    };
    serde_json::json!({ "id": f.id, "name": f.name, "mime": f.mime, "bytes": f.bytes, "state": state, "error": f.error })
}

/// Reads a completed download of the page, off the UI thread: its text when it's UTF-8, else
/// its bytes in base64, as Desktop's `files.get` hands a file over; at most 5 MiB, its limit.
#[cfg(not(target_arch = "wasm32"))]
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

/// Opens the page on the next poll, in a new tab of its session's side region, and answers with
/// its id (nil without an engine for this platform).
#[cfg(not(target_arch = "wasm32"))]
fn open(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (spec,): (Value,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let spec: opencode_plugin_spec::BrowserOpenSpec = mlua::FromLua::from_lua(spec, lua).and_then(|d: Data| d.decode().map_err(mlua::Error::runtime))?;
    let scope = scope::TabScope::capture(lua);
    with_app("Browser:OpenAsync", |app| {
        app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            if s.browser_unsupported() {
                return done.complete(Data::Nil);
            }
            let sid = scope.target_session(s, spec.session_id.as_deref());
            if sid.is_none() && (scope.tool_sid.is_some() || !scope.is_builtin_browser) {
                return done.complete(Data::Nil);
            }
            tx.instant();
            let url = if spec.url.trim().is_empty() { "about:blank" } else { spec.url.as_str() };
            let id = s.open_new_browser_tab_for_session(sid.as_deref(), url, None, spec.background != Some(true));
            if scope.tool_sid.is_some() {
                s.workspace_panes.browser.host.set_agent_driven(id, true);
            }
            done.complete(Data::String(id.to_string()));
        }));
        Ok(())
    })
}

/// Sends the allow-listed DevTools command on the next poll; `poll` answers it with `{ result }`,
/// or with `{ error }` when the policy or engine refused it, isn't running, or took too long.
#[cfg(not(target_arch = "wasm32"))]
fn devtools(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (id, cdp, params, target): (String, String, Value, Option<String>) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let scope = scope::TabScope::capture(lua);
    let params = match <Data as mlua::FromLua>::from_lua(params, lua)?.to_json() {
        serde_json::Value::Object(o) => serde_json::Value::Object(o),
        _ => serde_json::json!({}),
    };
    with_app("BrowserTab:DevtoolsAsync", |app| {
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
            if scope.tool_sid.is_some() {
                s.workspace_panes.browser.host.set_agent_driven(id, true);
            }
            let is_dialog = cdp == "Page.handleJavaScriptDialog";
            if !is_dialog && s.workspace_panes.browser.host.tab(id).is_some_and(|t| t.prompts.dialog.open().is_some()) {
                return done.complete(failed(format!("[browser.dialog_open] A JavaScript dialog is open in tab {id}. Answer or dismiss the dialog before continuing.")));
            }
            match s.workspace_panes.browser.host.send_cdp(id, cdp, params, target) {
                Ok(request) => {
                    s.workspace_panes.browser.devtools.insert(request, crate::surfaces::browser::PendingDevtools { done, deadline: Instant::now() + DEVTOOLS_TIMEOUT, tab_id: id, is_dialog });
                }
                Err(err) => done.complete(failed(err)),
            }
        }));
        Ok(())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn site_cookies(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (id,): (String,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let scope = scope::TabScope::capture(lua);
    with_app("BrowserTab:SiteCookiesAsync", |app| {
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
            match s.workspace_panes.browser.host.site_cookies(id) {
                Ok(request) => {
                    s.workspace_panes.browser.devtools.insert(request, crate::surfaces::browser::PendingDevtools { done, deadline: Instant::now() + DEVTOOLS_TIMEOUT, tab_id: id, is_dialog: false });
                }
                Err(err) => done.complete(failed(err)),
            }
        }));
        Ok(())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_site(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (id,): (String,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let scope = scope::TabScope::capture(lua);
    with_app("BrowserTab:ClearSiteAsync", |app| {
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
            match s.workspace_panes.browser.host.clear_site(id) {
                Ok(request) => {
                    s.workspace_panes.browser.devtools.insert(request, crate::surfaces::browser::PendingDevtools { done, deadline: Instant::now() + DEVTOOLS_TIMEOUT, tab_id: id, is_dialog: false });
                }
                Err(err) => done.complete(failed(err)),
            }
        }));
        Ok(())
    })
}

/// Why the engine can't take a DevTools call now.
#[cfg(not(target_arch = "wasm32"))]
fn not_running(status: &EngineStatus) -> String {
    match status {
        EngineStatus::Unsupported => "The browser isn't available on this platform yet in OpenCode (Rune)".to_owned(),
        EngineStatus::NotInstalled | EngineStatus::Downloading { .. } | EngineStatus::Verifying | EngineStatus::Paused(_) => "Browser engine isn't installed yet in OpenCode (Rune)".to_owned(),
        EngineStatus::Failed(err) => format!("Browser engine failed to start: {err}"),
        _ => "Browser engine is still starting; try again in a moment".to_owned(),
    }
}

/// The contract's `BrowserEngine` for the engine's status.
#[cfg(not(target_arch = "wasm32"))]
fn engine(app: &App) -> (&'static str, Option<String>) {
    match app.workspace_panes.browser.host.status() {
        _ if app.use_mock_browser() => ("ready", None),
        EngineStatus::Unsupported => ("unsupported", None),
        EngineStatus::NotInstalled => ("missing", None),
        EngineStatus::Downloading { .. } => ("downloading", None),
        EngineStatus::Verifying => ("verifying", None),
        EngineStatus::Paused(_) => ("paused", None),
        EngineStatus::Failed(f) => ("failed", Some(crate::browser::view::fault_text(&f.fault))),
        EngineStatus::Unchecked | EngineStatus::Installed | EngineStatus::Connecting => ("starting", None),
        EngineStatus::Ready => ("ready", None),
    }
}

/// A `BrowserTab` for page `id`.
#[cfg(not(target_arch = "wasm32"))]
fn tab(lua: &Lua, id: u64) -> mlua::Result<Table> {
    let methods = match lua.named_registry_value::<Option<Table>>("opencode.browser.tab")? {
        Some(m) => m,
        None => {
            let m = tab_methods(lua)?;
            lua.set_named_registry_value("opencode.browser.tab", &m)?;
            m
        }
    };
    object(lua, "BrowserTab", &[("Id", Value::String(lua.create_string(id.to_string())?))], methods)
}

#[cfg(not(target_arch = "wasm32"))]
fn id_of(this: &Table) -> mlua::Result<u64> {
    this.raw_get::<String>("Id")?.parse::<u64>().map_err(|_| mlua::Error::runtime("not a BrowserTab"))
}

/// Queues `f` for page `this` on the next poll, if it's still open and owned by the caller's session.
#[cfg(not(target_arch = "wasm32"))]
fn later(lua: &Lua, this: &Table, what: &str, f: impl FnOnce(&mut App, u64, &mut Tx) + Send + 'static) -> mlua::Result<()> {
    let id = id_of(this)?;
    let scope = scope::TabScope::capture(lua);
    with_app(what, |app| {
        scope.check_tab(app, id, true).map_err(mlua::Error::runtime)?;
        app.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            if s.workspace_panes.browser.host.tab(id).is_some() && scope.check_tab(s, id, true).is_ok() {
                tx.instant();
                f(s, id, tx);
            }
        }));
        Ok(())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn read_tab<R>(lua: &Lua, this: &Table, what: &str, f: impl FnOnce(&App, u64) -> mlua::Result<R>) -> mlua::Result<R> {
    cx::read(Dep::Browser);
    let id = id_of(this)?;
    let scope = scope::TabScope::capture(lua);
    with_app(what, |app| {
        scope.check_tab(app, id, false).map_err(mlua::Error::runtime)?;
        f(app, id)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn tab_methods(lua: &Lua) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    m.raw_set("IsOpen", lua.create_function(|lua, this: Table| {
        cx::read(Dep::Browser);
        let id = id_of(&this)?;
        let scope = scope::TabScope::capture(lua);
        with_app("BrowserTab:IsOpen", |app| {
            Ok(app.workspace_panes.browser.host.tab(id).is_some() && scope.check_tab(app, id, false).is_ok())
        })
    })?)?;
    m.raw_set(
        "Url",
        lua.create_function(|lua, this: Table| read_tab(lua, &this, "BrowserTab:Url", |app, id| Ok(app.workspace_panes.browser.host.tab(id).map(|t| t.url.clone()).unwrap_or_default())))?,
    )?;
    m.raw_set(
        "Title",
        lua.create_function(|lua, this: Table| read_tab(lua, &this, "BrowserTab:Title", |app, id| Ok(app.workspace_panes.browser.host.tab(id).map(|t| t.title.clone()).unwrap_or_default())))?,
    )?;
    for (name, field) in [("Loading", 0), ("CanGoBack", 1), ("CanGoForward", 2), ("Inspecting", 3)] {
        m.raw_set(
            name,
            lua.create_function(move |lua, this: Table| {
                read_tab(lua, &this, "BrowserTab", |app, id| Ok(app.workspace_panes.browser.host.tab(id).is_some_and(|t| [t.loading, t.can_go_back, t.can_go_forward, t.inspecting][field])))
            })?,
        )?;
    }
    m.raw_set(
        "LoadError",
        lua.create_function(|lua, this: Table| read_tab(lua, &this, "BrowserTab:LoadError", |app, id| Ok(app.workspace_panes.browser.host.tab(id).and_then(|t| t.load_error.clone()))))?,
    )?;
    m.raw_set(
        "Refusal",
        lua.create_function(|lua, this: Table| read_tab(lua, &this, "BrowserTab:Refusal", |app, id| Ok(app.workspace_panes.browser.host.tab(id).and_then(|t| t.refusal.clone()))))?,
    )?;
    m.raw_set(
        "Rejected",
        lua.create_function(|lua, this: Table| read_tab(lua, &this, "BrowserTab:Rejected", |app, id| Ok(app.workspace_panes.browser.host.tab(id).map_or(0i64, |t| t.rejected as i64))))?,
    )?;
    m.raw_set(
        "Files",
        lua.create_function(|lua, this: Table| {
            let files = read_tab(lua, &this, "BrowserTab:Files", |app, id| {
                Ok(app.workspace_panes.browser.host.tab(id).map(|t| t.files.iter().map(|f| json_to_data(&file_json(f))).collect()).unwrap_or_default())
            })?;
            mlua::IntoLua::into_lua(Data::List(files), lua)
        })?,
    )?;
    m.raw_set("Zoom", lua.create_function(|lua, this: Table| read_tab(lua, &this, "BrowserTab:Zoom", |app, id| Ok(app.workspace_panes.browser.host.tab(id).map_or(1.0, |t| t.zoom))))?)?;
    viewport::methods(lua, &m)?;
    chrome::tab_methods(lua, &m)?;
    m.raw_set(
        "Navigate",
        lua.create_function(|lua, (this, url): (Table, String)| {
            let agent = scope::TabScope::capture(lua).tool_sid.is_some();
            later(lua, &this, "BrowserTab:Navigate", move |s, id, _| {
                if agent { s.workspace_panes.browser.host.set_agent_driven(id, true); }
                s.navigate_browser_tab(id, &url);
            })
        })?,
    )?;
    for name in ["Back", "Forward", "Reload", "Stop"] {
        m.raw_set(
            name,
            lua.create_function(move |lua, this: Table| {
                later(lua, &this, "BrowserTab", move |s, id, _| {
                    if name != "Stop" { s.workspace_panes.browser.host.clear_refusal(id); }
                    let host = &mut s.workspace_panes.browser.host;
                    match name {
                        "Back" => host.back(id),
                        "Forward" => host.forward(id),
                        "Reload" => host.reload(id),
                        _ => host.stop(id),
                    }
                })
            })?,
        )?;
    }
    m.raw_set(
        "SetZoom",
        lua.create_function(|lua, (this, direction): (Table, String)| {
            let step = match direction.as_str() {
                "in" => 1,
                "out" => -1,
                "reset" => 0,
                other => return Err(mlua::Error::runtime(format!("BrowserTab:SetZoom: unknown direction {other:?}"))),
            };
            later(lua, &this, "BrowserTab:SetZoom", move |s, id, _| s.workspace_panes.browser.host.step_zoom(id, step))
        })?,
    )?;
    m.raw_set(
        "Inspect",
        lua.create_function(|lua, (this, enabled): (Table, Option<bool>)| {
            later(lua, &this, "BrowserTab:Inspect", move |s, id, _| {
                let on = s.workspace_panes.browser.host.tab(id).is_some_and(|t| t.inspecting);
                if enabled.is_none_or(|want| want != on) {
                    s.workspace_panes.browser.host.toggle_inspect(id);
                }
            })
        })?,
    )?;
    m.raw_set(
        "Highlight",
        lua.create_function(|lua, (this, node): (Table, Option<f64>)| {
            let id = id_of(&this)?;
            let scope = scope::TabScope::capture(lua);
            with_app("BrowserTab:Highlight", |app| {
                scope.check_tab(app, id, true).map_err(mlua::Error::runtime)?;
                app.workspace_panes.browser.host.highlight_node(id, node.map(|n| n as i64));
                Ok(())
            })
        })?,
    )?;
    m.raw_set(
        "Show",
        lua.create_function(|lua, (this, options): (Table, Value)| {
            let o: opencode_plugin_spec::BrowserShowOptions = if options.is_nil() { Default::default() } else { mlua::FromLua::from_lua(options, lua).and_then(|d: Data| d.decode().map_err(mlua::Error::runtime))? };
            let scope = scope::TabScope::capture(lua);
            later(lua, &this, "BrowserTab:Show", move |s, id, _| {
                let Some(sid) = scope.target_session(s, o.session_id.as_deref()) else { return };
                s.register_existing_browser_tab_for_session(&sid, id, o.background != Some(true));
                s.workspace_panes.browser.host.focus_tab(id);
            })
        })?,
    )?;
    m.raw_set(
        "Close",
        lua.create_function(|lua, this: Table| {
            later(lua, &this, "BrowserTab:Close", |s, id, tx| match s.browser_tab_session(id).map(str::to_owned).or_else(|| s.active_session_id().map(str::to_owned)) {
                Some(sid) => s.close_browser_tab_for_session(&sid, id, tx),
                None => s.forget_browser_tab(id),
            })
        })?,
    )?;
    let raw: Function = match lua.app_data_ref::<std::rc::Rc<Vmx>>() {
        Some(vmx) => vmx.asyncs.raw_get("BrowserTab.DevtoolsAsync")?,
        None => return Err(mlua::Error::runtime("the plugin host isn't set up")),
    };
    let call: Function = lua
        .load("local raw = ... return function(self, method, params, target) local r = raw(self.Id, method, params, target) if r and r.error then error(r.error, 0) end return if r then r.result else nil end")
        .set_name("=BrowserTab:DevtoolsAsync")
        .call(raw)?;
    m.raw_set("DevtoolsAsync", call)?;
    let raw: Function = match lua.app_data_ref::<std::rc::Rc<Vmx>>() {
        Some(vmx) => vmx.asyncs.raw_get("BrowserTab.SiteCookiesAsync")?,
        None => return Err(mlua::Error::runtime("the plugin host isn't set up")),
    };
    let call: Function = lua
        .load("local raw = ... return function(self) local r = raw(self.Id) if r and r.error then error(r.error, 0) end local res = if r then r.result else nil return if type(res) == 'table' and type(res.cookies) == 'number' then res.cookies else 0 end")
        .set_name("=BrowserTab:SiteCookiesAsync")
        .call(raw)?;
    m.raw_set("SiteCookiesAsync", call)?;
    let raw: Function = match lua.app_data_ref::<std::rc::Rc<Vmx>>() {
        Some(vmx) => vmx.asyncs.raw_get("BrowserTab.ClearSiteAsync")?,
        None => return Err(mlua::Error::runtime("the plugin host isn't set up")),
    };
    let call: Function = lua
        .load("local raw = ... return function(self) local r = raw(self.Id) if r and r.error then error(r.error, 0) end end")
        .set_name("=BrowserTab:ClearSiteAsync")
        .call(raw)?;
    m.raw_set("ClearSiteAsync", call)?;
    let raw: Function = match lua.app_data_ref::<std::rc::Rc<Vmx>>() {
        Some(vmx) => vmx.asyncs.raw_get("BrowserTab.ReadFileAsync")?,
        None => return Err(mlua::Error::runtime("the plugin host isn't set up")),
    };
    let call: Function = lua
        .load("local raw = ... return function(self, fileId) local r = raw(self.Id, fileId) if r and r.error then error(r.error, 0) end return r end")
        .set_name("=BrowserTab:ReadFileAsync")
        .call(raw)?;
    m.raw_set("ReadFileAsync", call)?;
    Ok(m)
}

/// What `Dep::Browser` reads now, as a number that changes with it.
#[cfg(not(target_arch = "wasm32"))]
pub fn version(app: &App) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    format!("{:?}", app.workspace_panes.browser.host.status()).hash(&mut h);
    app.workspace_panes.browser.host.engine_downloads().hash(&mut h);
    app.workspace_panes.browser.host.engine_policy().hash(&mut h);
    app.workspace_panes.browser.host.engine_permission_sync().hash(&mut h);
    for t in app.workspace_panes.browser.host.tabs() {
        if let Some(d) = &t.device { (&d.preset, d.width, d.height, d.scale.map(f32::to_bits)).hash(&mut h); }
        (t.id, &t.url, &t.title, t.loading, t.can_go_back, t.can_go_forward, t.zoom.to_bits(), t.inspecting, &t.load_error, &t.favicon, &t.refusal, t.rejected).hash(&mut h);
        t.prompts.hash_state(&mut h);
        t.find.hash_state(&mut h);
        // A download's step or end: `BrowserTab:Files` reads them.
        t.files.iter().for_each(|f| (&f.id, f.bytes, f.state as u8, &f.error).hash(&mut h));
    }
    app.workspace_panes.browser.host.focused_tab_id().hash(&mut h);
    app.workspace_panes.browser.pending_app_links.borrow().len().hash(&mut h);
    let mut sessions: Vec<_> = app.workspace_panes.browser.session_tabs.iter().collect();
    sessions.sort();
    sessions.hash(&mut h);
    // Only the composer's focus: a region reading the composer would build again per keystroke.
    // The side region passes it in (`panels::view::side_region`).
    let side = app.kernel.slots.get(&crate::panels::SIDE);
    (rune::Tracked::untracked(&app.composer).focused, side.focus.is_focused(), format!("{:?}", side.active_pane_focus), app.active_browser_tab_id()).hash(&mut h);
    app.active_session_id().and_then(|sid| side.sessions.get(sid)).map(|st| st.visibility.open()).hash(&mut h);
    h.finish()
}

/// Each open page's id and a number that moves with its address, title, loading and refusal, for
/// `Browser.Changed`.
#[cfg(not(target_arch = "wasm32"))]
pub fn pages(app: &App) -> Vec<(String, u64)> {
    app.workspace_panes.browser
        .host
        .tabs()
        .iter()
        .map(|t| {
            let mut h = std::hash::DefaultHasher::new();
            (&t.url, &t.title, t.loading, &t.refusal, t.rejected, &t.favicon).hash(&mut h);
            t.find.hash_state(&mut h);
            let pending_links = app
                .workspace_panes
                .browser
                .pending_app_links
                .borrow()
                .iter()
                .filter(|(id, _)| *id == t.id)
                .count();
            pending_links.hash(&mut h);
            (t.id.to_string(), h.finish())
        })
        .collect()
}

/// `ui.browserSurface`: page `tab` filling the rest of the pane, or the engine's state.
#[cfg(not(target_arch = "wasm32"))]
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    cx::with_app(|app| {
        cx::read(Dep::Browser);
        let id = handle_field(a, "tab", "Id").and_then(|s| s.parse::<u64>().ok()).ok_or("ui.browserSurface needs a page from the Browser service")?;
        let pal = p(a);
        let surface = crate::browser::view::surface(app, &pal, id, app.plugins.panel_scale.get()).unwrap_or_else(|| col().key("browser-no-page").grow(1.0).w_full());
        let surface = viewport::frame(a, app, id, surface);
        // The node's key lands on this frame, so the page and the engine's states keep their own.
        Ok(col().grow(1.0).min_h(0.0).w_full().child(surface))
    })
    .ok_or_else(|| "ui.browserSurface draws only in a render".to_owned())?
}

/// The engine's messages, DevTools answers and picks.
#[cfg(not(target_arch = "wasm32"))]
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let mut changed = app.poll_browser();
    let now = Instant::now();
    let requests: Vec<u64> = app.workspace_panes.browser.devtools.keys().copied().collect();
    for request in requests {
        let answer = match app.workspace_panes.browser.host.take_cdp_response(request) {
            Some(Ok(v)) => map([("result", json_to_data(&v))]),
            Some(Err(e)) => map([("error", Data::String(e["message"].as_str().map(str::to_owned).unwrap_or_else(|| e.to_string())))]),
            None if app.workspace_panes.browser.devtools.get(&request).is_some_and(|p| p.deadline <= now) => map([("error", Data::String("Browser command timed out".into()))]),
            None => continue,
        };
        if let Some(pending) = app.workspace_panes.browser.devtools.remove(&request) {
            pending.done.complete(answer);
            changed = true;
        }
    }
    changed |= crate::surfaces::browser::prompts::poll(app, tx, now);
    changed |= crate::surfaces::browser::context_menu::poll(app, tx);
    if let Some(next) = app.workspace_panes.browser.devtools.values().map(|p| p.deadline).min() {
        tx.wake_after(next.saturating_duration_since(now).max(Duration::from_millis(25)));
    }
    let picks: Vec<_> = if app.workspace_panes.browser.host.has_picked() { app.workspace_panes.browser.host.drain_picked_elements().collect() } else { Vec::new() };
    for pick in picks {
        let payload = map([("tabId", Data::String(pick.tab_id.to_string())), ("ref", Data::String(pick.eref)), ("selector", Data::String(pick.selector))]);
        for i in 0..app.plugins.list.len() {
            changed |= super::super::fire_named(app, i, "Browser.Picked", payload.clone());
        }
    }
    changed
}

/// IME for the page that has the keyboard.
#[cfg(not(target_arch = "wasm32"))]
pub fn ime(app: &mut App, ime: &Ime) -> bool {
    let Some(id) = app.active_browser_tab_id() else { return false };
    if !app.side_open() || app.composer.focused || super::super::field_focused(app) {
        return false;
    }
    if app.workspace_panes.browser.host.tab(id).is_some_and(|t| t.prompts.dialog.open().is_some() || t.prompts.auth.active().is_some()) {
        return false;
    }
    app.workspace_panes.browser.host.ime(ime)
}
