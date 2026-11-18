//! `Browser`, `Composer` and `ui.browserSurface`: the window's pages as the app's embedded
//! browser engine (`crate::browser`, `browser_pane`) runs them, which starts and downloads the
//! engine, composites each page's frames, forwards input and carries DevTools calls. The browser
//! plugin keeps the tabs, their titles, the address bar, history, the element picker's notes,
//! the commands and the MCP tools.

use std::hash::{Hash, Hasher};
use std::time::Duration;
use rune::time::Instant;

use browser_pane::EngineStatus;
use mlua::{Function, Lua, MultiValue, Table, Value};
use rune::{El, Ime, Tx, col};
use rune_luau::{Completer, Data, Host, NodeArgs};
mod viewport;

use super::super::cx::{self, Dep};
use super::super::services::{app_now, json_to_data, lazy_signals, map, method};
use super::super::vm::Vmx;
use super::{handle_field, object, p};
use crate::App;

/// How long a DevTools call may take before it fails.
const DEVTOOLS_TIMEOUT: Duration = Duration::from_secs(10);

/// `Browser:Prepare`: how long the engine it starts waits for a page by default, and at most.
const PREPARE_HOLD_S: f64 = 5.0;
const PREPARE_HOLD_MAX_S: f64 = 60.0;

/// The `Browser` service.
pub fn build(lua: &Lua, vmx: &std::rc::Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    method(lua, t, "Tabs", |lua, sid: Option<String>| {
        cx::read(Dep::Browser);
        let app = app_now("Browser:Tabs")?;
        let ids: Vec<u64> = match sid {
            Some(sid) => app.browser.session_tabs.get(&sid).map(|ids| ids.iter().copied().filter(|&id| app.browser.host.tab(id).is_some()).collect()).unwrap_or_default(),
            None => app.browser.host.tabs().iter().map(|t| t.id).collect(),
        };
        let out = lua.create_table()?;
        for (n, id) in ids.into_iter().enumerate() {
            out.raw_set(n + 1, tab(lua, id)?)?;
        }
        Ok(out)
    })?;
    method(lua, t, "Get", |lua, id: String| {
        cx::read(Dep::Browser);
        let app = app_now("Browser:Get")?;
        match id.parse::<u64>().ok().filter(|&id| app.browser.host.tab(id).is_some()) {
            Some(id) => Ok(Value::Table(tab(lua, id)?)),
            None => Ok(Value::Nil),
        }
    })?;
    method(lua, t, "List", |lua, (_sid, open): (String, Vec<String>)| {
        cx::read(Dep::Browser);
        let app = app_now("Browser:List")?;
        let rows = open
            .iter()
            .map(|raw| {
                let page = raw.parse::<u64>().ok().and_then(|id| app.browser.host.tab(id));
                map([
                    ("id", Data::String(raw.clone())),
                    ("url", Data::String(page.map(|t| t.url.clone()).filter(|u| u != "about:blank").unwrap_or_default())),
                    ("title", Data::String(page.map(|t| t.title.clone()).unwrap_or_default())),
                    ("open", Data::Bool(page.is_some())),
                ])
            })
            .collect();
        super::diff::lua_data(lua, Data::List(rows))
    })?;
    method(lua, t, "Active", |lua, sid: String| {
        cx::read(Dep::Browser);
        let app = app_now("Browser:Active")?;
        match app.default_browser_tab_for_session(&sid).filter(|&id| app.browser.host.tab(id).is_some()) {
            Some(id) => Ok(Value::Table(tab(lua, id)?)),
            None => Ok(Value::Nil),
        }
    })?;
    method(lua, t, "Selected", |lua, sid: Option<String>| {
        cx::read(Dep::Browser);
        let app = app_now("Browser:Selected")?;
        let Some(sid) = sid.or_else(|| app.active_session_id().map(str::to_owned)) else { return Ok(Value::Nil) };
        let open = app.side.sessions.get(&sid).is_some_and(|st| st.visibility.open()) && !app.narrow();
        match app.active_browser_tab_for_session(&sid).filter(|&id| open && app.browser.host.tab(id).is_some()) {
            Some(id) => Ok(Value::Table(tab(lua, id)?)),
            None => Ok(Value::Nil),
        }
    })?;
    let plugin = vmx.half.id.clone();
    method(lua, t, "Focused", move |lua, ()| {
        cx::read(Dep::Browser);
        let app = app_now("Browser:Focused")?;
        let field = super::super::editors::plugin_focused(app, &plugin);
        match app.focused_browser_pane(field).and(app.active_browser_tab_id()) {
            Some(id) => Ok(Value::Table(tab(lua, id)?)),
            None => Ok(Value::Nil),
        }
    })?;
    method(lua, t, "Available", |_, ()| {
        cx::read(Dep::Browser);
        Ok(!app_now("Browser:Available")?.browser_unsupported())
    })?;
    method(lua, t, "Engine", |lua, ()| {
        cx::read(Dep::Browser);
        let app = app_now("Browser:Engine")?;
        let (state, message) = engine(app);
        mlua::IntoLua::into_lua(
            map([
                ("state", Data::String(state.into())),
                ("message", message.map_or(Data::Nil, Data::String)),
                ("downloads", Data::Bool(app.browser.host.engine_downloads())),
                ("policy", Data::Bool(app.browser.host.engine_policy())),
            ]),
            lua,
        )
    })?;
    method(lua, t, "NormalizeUrl", |_, (url, sid): (String, Option<String>)| {
        let app = app_now("Browser:NormalizeUrl")?;
        let roots = if app.browser.host.has_live_sidecar() && !app.browser.host.engine_policy() {
            Vec::new()
        } else {
            app.browser_file_roots(sid.as_deref())
        };
        browser_pane::normalize_url(&url, &roots).map_err(mlua::Error::runtime)
    })?;
    method(lua, t, "Prepare", |_, seconds: Option<f64>| {
        let hold = Duration::from_secs_f64(seconds.unwrap_or(PREPARE_HOLD_S).clamp(1.0, PREPARE_HOLD_MAX_S));
        app_now("Browser:Prepare")?.later(Box::new(move |s: &mut App, _: &mut Tx| {
            if !s.use_mock_browser() && !s.browser_unsupported() {
                s.browser.host.prepare(hold);
            }
        }));
        Ok(())
    })?;
    let raw: Function = vmx.asyncs.raw_get("Browser.OpenAsync")?;
    let open: Function = lua.load("local raw = ... return function(self, spec) local id = raw(spec) return if id then self:Get(id) else nil end").set_name("=Browser:OpenAsync").call(raw)?;
    t.raw_set("OpenAsync", open)?;
    lazy_signals(lua, vmx, t, "Browser", &["Changed", "Picked"])
}

/// The `Composer` service.
pub fn composer(lua: &Lua, t: &Table) -> mlua::Result<()> {
    method(lua, t, "InsertPill", |_, (id, label): (String, String)| {
        app_now("Composer:InsertPill")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if !s.composer.text.is_empty() && !s.composer.text.ends_with(char::is_whitespace) {
                s.composer.insert(" ");
            }
            s.composer.insert_pill(id, &label);
            s.composer.insert(" ");
            s.composer.focused = true;
            s.side.focus.blur();
            super::super::blur_fields(s);
        }));
        Ok(())
    })
}

/// The host's async functions: `Browser:OpenAsync` and `BrowserTab`'s async methods.
pub fn asyncs(host: &Host<'_>, t: &Table) -> mlua::Result<()> {
    t.raw_set("Browser.OpenAsync", host.async_function("Browser:OpenAsync", open)?)?;
    t.raw_set("BrowserTab.DevtoolsAsync", host.async_function("BrowserTab:DevtoolsAsync", devtools)?)?;
    t.raw_set("BrowserTab.SiteCookiesAsync", host.async_function("BrowserTab:SiteCookiesAsync", site_cookies)?)?;
    t.raw_set("BrowserTab.ClearSiteAsync", host.async_function("BrowserTab:ClearSiteAsync", clear_site)?)?;
    t.raw_set("BrowserTab.ReadFileAsync", host.async_function("BrowserTab:ReadFileAsync", read_file)?)
}

/// A downloaded file as the plugin sees it: Desktop's `files.list` fields (no path).
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
fn read_file(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    const MAX: u64 = 5 * 1024 * 1024;
    let (id, file_id): (String, String) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    app_now("BrowserTab:ReadFileAsync")?.later(Box::new(move |s: &mut App, _: &mut Tx| {
        let failed = |message: String| map([("error", Data::String(message))]);
        let tab = id.parse::<u64>().ok().and_then(|id| s.browser.host.tab(id));
        let tab_id = id.parse::<u64>().unwrap_or_default();
        let Some(file) = tab.and_then(|t| t.files.iter().find(|f| f.id == file_id)).cloned() else {
            return done.complete(failed(format!("No downloaded file {file_id} in browser tab {id}: list them with browser_files")));
        };
        match file.state {
            browser_pane::DownloadState::Pending => return done.complete(failed(format!("{} is still downloading: check browser_files again", file.name))),
            browser_pane::DownloadState::Failed => return done.complete(failed(format!("{} didn't download ({})", file.name, file.error.as_deref().unwrap_or("failed")))),
            browser_pane::DownloadState::Completed => {}
        }
        // Opened only where the engine keeps downloads, never at a path the engine merely named.
        let Some(path) = s.browser.host.download_path(tab_id, &file) else {
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
}

/// Opens the page on the next poll, in a new tab of its session's side region, and answers with
/// its id (nil without an engine for this platform).
fn open(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (spec,): (Value,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let spec: opencode_plugin_spec::BrowserOpenSpec = mlua::FromLua::from_lua(spec, lua).and_then(|d: Data| d.decode().map_err(mlua::Error::runtime))?;
    app_now("Browser:OpenAsync")?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
        if s.browser_unsupported() {
            return done.complete(Data::Nil);
        }
        tx.instant();
        let sid = spec.session_id.or_else(|| s.active_session_id().map(str::to_owned));
        let url = if spec.url.trim().is_empty() { "about:blank" } else { spec.url.as_str() };
        let id = s.open_new_browser_tab_for_session(sid.as_deref(), url, None, spec.background != Some(true));
        done.complete(Data::String(id.to_string()));
    }));
    Ok(())
}

/// Sends the allow-listed DevTools command on the next poll; `poll` answers it with `{ result }`,
/// or with `{ error }` when the policy or engine refused it, isn't running, or took too long.
fn devtools(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (id, cdp, params, target): (String, String, Value, Option<String>) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let params = match <Data as mlua::FromLua>::from_lua(params, lua)?.to_json() {
        serde_json::Value::Object(o) => serde_json::Value::Object(o),
        _ => serde_json::json!({}),
    };
    app_now("BrowserTab:DevtoolsAsync")?.later(Box::new(move |s: &mut App, _: &mut Tx| {
        let failed = |message: String| map([("error", Data::String(message))]);
        let Some(id) = id.parse::<u64>().ok().filter(|&id| s.browser.host.tab(id).is_some()) else {
            return done.complete(failed(format!("No browser tab with tabID {id}")));
        };
        if !s.browser.host.has_live_sidecar() {
            return done.complete(failed(not_running(&s.browser.host.status())));
        }
        match s.browser.host.send_cdp(id, cdp, params, target) {
            Ok(request) => {
                s.browser.devtools.insert(request, (done, Instant::now() + DEVTOOLS_TIMEOUT));
            }
            Err(err) => done.complete(failed(err)),
        }
    }));
    Ok(())
}

fn site_cookies(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (id,): (String,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    app_now("BrowserTab:SiteCookiesAsync")?.later(Box::new(move |s: &mut App, _: &mut Tx| {
        let failed = |message: String| map([("error", Data::String(message))]);
        let Some(id) = id.parse::<u64>().ok().filter(|&id| s.browser.host.tab(id).is_some()) else {
            return done.complete(failed(format!("No browser tab with tabID {id}")));
        };
        if !s.browser.host.has_live_sidecar() {
            return done.complete(failed(not_running(&s.browser.host.status())));
        }
        match s.browser.host.site_cookies(id) {
            Ok(request) => {
                s.browser.devtools.insert(request, (done, Instant::now() + DEVTOOLS_TIMEOUT));
            }
            Err(err) => done.complete(failed(err)),
        }
    }));
    Ok(())
}

fn clear_site(lua: &Lua, a: MultiValue, done: Completer) -> mlua::Result<()> {
    let (id,): (String,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let is_builtin_browser = lua
        .app_data_ref::<std::rc::Rc<Vmx>>()
        .is_some_and(|vmx| &*vmx.half.id == "browser");
    let (tool_sid, in_flight) = super::super::commands::current_tool_scope(lua);
    app_now("BrowserTab:ClearSiteAsync")?.later(Box::new(move |s: &mut App, _: &mut Tx| {
        let failed = |message: String| map([("error", Data::String(message))]);
        let Some(id) = id.parse::<u64>().ok().filter(|&id| s.browser.host.tab(id).is_some()) else {
            return done.complete(failed(format!("No browser tab with tabID {id}")));
        };
        let Some(owner) = s.browser_tab_session(id) else {
            return done.complete(failed(format!("Browser tab {id} does not belong to an open session.")));
        };
        let allowed = match &tool_sid {
            Some(caller) => caller.as_deref() == Some(owner),
            None => {
                s.active_session_id() == Some(owner)
                    && (is_builtin_browser || in_flight.iter().all(|sid| sid == owner))
            }
        };
        if !allowed {
            return done.complete(failed(format!("Browser tab {id} belongs to another session.")));
        }
        if !s.browser.host.has_live_sidecar() {
            return done.complete(failed(not_running(&s.browser.host.status())));
        }
        match s.browser.host.clear_site(id) {
            Ok(request) => {
                s.browser.devtools.insert(request, (done, Instant::now() + DEVTOOLS_TIMEOUT));
            }
            Err(err) => done.complete(failed(err)),
        }
    }));
    Ok(())
}

/// Why the engine can't take a DevTools call now.
fn not_running(status: &EngineStatus) -> String {
    match status {
        EngineStatus::Unsupported => "The browser isn't available on this platform yet in OpenCode (Rune)".to_owned(),
        EngineStatus::NotInstalled | EngineStatus::Downloading { .. } | EngineStatus::Verifying | EngineStatus::Paused(_) => "Browser engine isn't installed yet in OpenCode (Rune)".to_owned(),
        EngineStatus::Failed(err) => format!("Browser engine failed to start: {err}"),
        _ => "Browser engine is still starting; try again in a moment".to_owned(),
    }
}

/// The contract's `BrowserEngine` for the engine's status.
fn engine(app: &App) -> (&'static str, Option<String>) {
    match app.browser.host.status() {
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

fn id_of(this: &Table) -> mlua::Result<u64> {
    this.raw_get::<String>("Id")?.parse::<u64>().map_err(|_| mlua::Error::runtime("not a BrowserTab"))
}

/// Queues `f` for page `this` on the next poll, if it's still open then.
fn later(this: &Table, what: &str, f: impl FnOnce(&mut App, u64, &mut Tx) + Send + 'static) -> mlua::Result<()> {
    let id = id_of(this)?;
    app_now(what)?.later(Box::new(move |s: &mut App, tx: &mut Tx| {
        if s.browser.host.tab(id).is_some() {
            tx.instant();
            f(s, id, tx);
        }
    }));
    Ok(())
}

fn tab_methods(lua: &Lua) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    let read = |this: &Table, what: &str| -> mlua::Result<(&'static App, u64)> {
        cx::read(Dep::Browser);
        Ok((app_now(what)?, id_of(this)?))
    };
    m.raw_set("IsOpen", lua.create_function(move |_, this: Table| read(&this, "BrowserTab:IsOpen").map(|(app, id)| app.browser.host.tab(id).is_some()))?)?;
    m.raw_set(
        "Url",
        lua.create_function(move |_, this: Table| read(&this, "BrowserTab:Url").map(|(app, id)| app.browser.host.tab(id).map(|t| t.url.clone()).unwrap_or_default()))?,
    )?;
    m.raw_set(
        "Title",
        lua.create_function(move |_, this: Table| read(&this, "BrowserTab:Title").map(|(app, id)| app.browser.host.tab(id).map(|t| t.title.clone()).unwrap_or_default()))?,
    )?;
    for (name, field) in [("Loading", 0), ("CanGoBack", 1), ("CanGoForward", 2), ("Inspecting", 3)] {
        m.raw_set(
            name,
            lua.create_function(move |_, this: Table| {
                let (app, id) = read(&this, "BrowserTab")?;
                Ok(app.browser.host.tab(id).is_some_and(|t| [t.loading, t.can_go_back, t.can_go_forward, t.inspecting][field]))
            })?,
        )?;
    }
    m.raw_set(
        "LoadError",
        lua.create_function(move |_, this: Table| read(&this, "BrowserTab:LoadError").map(|(app, id)| app.browser.host.tab(id).and_then(|t| t.load_error.clone())))?,
    )?;
    m.raw_set(
        "Refusal",
        lua.create_function(move |_, this: Table| read(&this, "BrowserTab:Refusal").map(|(app, id)| app.browser.host.tab(id).and_then(|t| t.refusal.clone())))?,
    )?;
    m.raw_set(
        "Rejected",
        lua.create_function(move |_, this: Table| read(&this, "BrowserTab:Rejected").map(|(app, id)| app.browser.host.tab(id).map_or(0i64, |t| t.rejected as i64)))?,
    )?;
    m.raw_set(
        "Files",
        lua.create_function(move |lua, this: Table| {
            let (app, id) = read(&this, "BrowserTab:Files")?;
            let files = app.browser.host.tab(id).map(|t| t.files.iter().map(|f| json_to_data(&file_json(f))).collect()).unwrap_or_default();
            mlua::IntoLua::into_lua(Data::List(files), lua)
        })?,
    )?;
    m.raw_set("Zoom", lua.create_function(move |_, this: Table| read(&this, "BrowserTab:Zoom").map(|(app, id)| app.browser.host.tab(id).map_or(1.0, |t| t.zoom)))?)?;
    viewport::methods(lua, &m)?;
    m.raw_set(
        "Navigate",
        lua.create_function(|_, (this, url): (Table, String)| later(&this, "BrowserTab:Navigate", move |s, id, _| s.navigate_browser_tab(id, &url)))?,
    )?;
    for name in ["Back", "Forward", "Reload", "Stop"] {
        m.raw_set(
            name,
            lua.create_function(move |_, this: Table| {
                later(&this, "BrowserTab", move |s, id, _| {
                    if name != "Stop" {
                        s.browser.host.clear_refusal(id);
                    }
                    let host = &s.browser.host;
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
        lua.create_function(|_, (this, direction): (Table, String)| {
            let step = match direction.as_str() {
                "in" => 1,
                "out" => -1,
                "reset" => 0,
                other => return Err(mlua::Error::runtime(format!("BrowserTab:SetZoom: unknown direction {other:?}"))),
            };
            later(&this, "BrowserTab:SetZoom", move |s, id, _| s.browser.host.step_zoom(id, step))
        })?,
    )?;
    m.raw_set(
        "Inspect",
        lua.create_function(|_, (this, enabled): (Table, Option<bool>)| {
            later(&this, "BrowserTab:Inspect", move |s, id, _| {
                let on = s.browser.host.tab(id).is_some_and(|t| t.inspecting);
                if enabled.is_none_or(|want| want != on) {
                    s.browser.host.toggle_inspect(id);
                }
            })
        })?,
    )?;
    m.raw_set(
        "Highlight",
        lua.create_function(|_, (this, node): (Table, Option<f64>)| {
            let app = app_now("BrowserTab:Highlight")?;
            app.browser.host.highlight_node(id_of(&this)?, node.map(|n| n as i64));
            Ok(())
        })?,
    )?;
    m.raw_set(
        "Show",
        lua.create_function(|lua, (this, options): (Table, Value)| {
            let o: opencode_plugin_spec::BrowserShowOptions = if options.is_nil() { Default::default() } else { mlua::FromLua::from_lua(options, lua).and_then(|d: Data| d.decode().map_err(mlua::Error::runtime))? };
            later(&this, "BrowserTab:Show", move |s, id, _| {
                let Some(sid) = o.session_id.or_else(|| s.active_session_id().map(str::to_owned)) else { return };
                s.register_existing_browser_tab_for_session(&sid, id, o.background != Some(true));
                s.browser.host.focus_tab(id);
            })
        })?,
    )?;
    m.raw_set(
        "Close",
        lua.create_function(|_, this: Table| {
            later(&this, "BrowserTab:Close", |s, id, tx| match s.browser_tab_session(id).map(str::to_owned).or_else(|| s.active_session_id().map(str::to_owned)) {
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
pub fn version(app: &App) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    format!("{:?}", app.browser.host.status()).hash(&mut h);
    app.browser.host.engine_downloads().hash(&mut h);
    app.browser.host.engine_policy().hash(&mut h);
    for t in app.browser.host.tabs() {
        if let Some(d) = &t.device { (&d.preset, d.width, d.height, d.scale.map(f32::to_bits)).hash(&mut h); }
        (t.id, &t.url, &t.title, t.loading, t.can_go_back, t.can_go_forward, t.zoom.to_bits(), t.inspecting, &t.load_error, &t.refusal, t.rejected).hash(&mut h);
        // A download's step or end: `BrowserTab:Files` reads them.
        t.files.iter().for_each(|f| (&f.id, f.bytes, f.state as u8, &f.error).hash(&mut h));
    }
    app.browser.host.focused_tab_id().hash(&mut h);
    let mut sessions: Vec<_> = app.browser.session_tabs.iter().collect();
    sessions.sort();
    sessions.hash(&mut h);
    // Only the composer's focus: a region reading the composer would build again per keystroke.
    // The side region passes it in (`panels::view::side_region`).
    (rune::Tracked::untracked(&app.composer).focused, app.side.focus.is_focused(), format!("{:?}", app.side.active_pane_focus), app.active_browser_tab_id()).hash(&mut h);
    app.active_session_id().and_then(|sid| app.side.sessions.get(sid)).map(|st| st.visibility.open()).hash(&mut h);
    h.finish()
}

/// Each open page's id and a number that moves with its address, title, loading and refusal, for
/// `Browser.Changed`.
pub fn pages(app: &App) -> Vec<(String, u64)> {
    app.browser
        .host
        .tabs()
        .iter()
        .map(|t| {
            let mut h = std::hash::DefaultHasher::new();
            (&t.url, &t.title, t.loading, &t.refusal, t.rejected).hash(&mut h);
            (t.id.to_string(), h.finish())
        })
        .collect()
}

/// `ui.browserSurface`: page `tab` filling the rest of the pane, or the engine's state.
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    let app = cx::app().ok_or("ui.browserSurface draws only in a render")?;
    cx::read(Dep::Browser);
    let id = handle_field(a, "tab", "Id").and_then(|s| s.parse::<u64>().ok()).ok_or("ui.browserSurface needs a page from the Browser service")?;
    let pal = p(a);
    let surface = crate::browser::view::surface(app, &pal, id, super::terminal::SCALE.get()).unwrap_or_else(|| col().key("browser-no-page").grow(1.0).w_full());
    let surface = viewport::frame(a, app, id, surface);
    // The node's key lands on this frame, so the page and the engine's states keep their own.
    Ok(col().grow(1.0).min_h(0.0).w_full().child(surface))
}

/// The engine's messages, DevTools answers and picks.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let mut changed = app.poll_browser();
    let now = Instant::now();
    let requests: Vec<u64> = app.browser.devtools.keys().copied().collect();
    for request in requests {
        let answer = match app.browser.host.take_cdp_response(request) {
            Some(Ok(v)) => map([("result", json_to_data(&v))]),
            Some(Err(e)) => map([("error", Data::String(e["message"].as_str().map(str::to_owned).unwrap_or_else(|| e.to_string())))]),
            None if app.browser.devtools.get(&request).is_some_and(|(_, at)| *at <= now) => map([("error", Data::String("Browser command timed out".into()))]),
            None => continue,
        };
        if let Some((done, _)) = app.browser.devtools.remove(&request) {
            done.complete(answer);
            changed = true;
        }
    }
    if let Some(next) = app.browser.devtools.values().map(|(_, at)| *at).min() {
        tx.wake_after(next.saturating_duration_since(now).max(Duration::from_millis(25)));
    }
    let picks: Vec<_> = if app.browser.host.has_picked() { app.browser.host.drain_picked_elements().collect() } else { Vec::new() };
    for pick in picks {
        let payload = map([("tabId", Data::String(pick.tab_id.to_string())), ("ref", Data::String(pick.eref)), ("selector", Data::String(pick.selector))]);
        for i in 0..app.plugins.list.len() {
            changed |= super::super::fire_named(app, i, "Browser.Picked", payload.clone());
        }
    }
    changed
}

/// IME for the page that has the keyboard.
pub fn ime(app: &mut App, ime: &Ime) -> bool {
    if !app.side_open() || app.composer.focused || super::super::field_focused(app) || app.active_browser_tab_id().is_none() {
        return false;
    }
    app.browser.host.ime(ime)
}
