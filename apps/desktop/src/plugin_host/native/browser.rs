//! `Browser`, `Composer` and `ui.browserSurface`: the window's pages as the app's embedded
//! browser engine (`crate::browser`, `browser_pane`) runs them, which starts and downloads the
//! engine, composites each page's frames, forwards input and carries DevTools calls. The browser
//! plugin keeps the tabs, their titles, the address bar, history, the element picker's notes,
//! the commands and the MCP tools.

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use browser_pane::EngineStatus;
use mlua::{Function, Lua, MultiValue, Table, Value};
use rune::{El, Ime, Tx, col};
use rune_luau::{Completer, Data, Host, NodeArgs};

use super::super::cx::{self, Dep};
use super::super::services::{app_now, json_to_data, lazy_signals, map, method};
use super::super::vm::Vmx;
use super::{handle_field, object, p};
use crate::App;

/// How long a DevTools call may take before it fails.
const DEVTOOLS_TIMEOUT: Duration = Duration::from_secs(10);

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
        let open = app.side.sessions.get(&sid).is_some_and(|st| st.open) && !app.narrow();
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
        mlua::IntoLua::into_lua(map([("state", Data::String(state.into())), ("message", message.map_or(Data::Nil, Data::String))]), lua)
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
            s.side.focused = false;
            super::super::blur_fields(s);
        }));
        Ok(())
    })
}

/// The host's async functions: `Browser:OpenAsync` and `BrowserTab:DevtoolsAsync`.
pub fn asyncs(host: &Host<'_>, t: &Table) -> mlua::Result<()> {
    t.raw_set("Browser.OpenAsync", host.async_function("Browser:OpenAsync", open)?)?;
    t.raw_set("BrowserTab.DevtoolsAsync", host.async_function("BrowserTab:DevtoolsAsync", devtools)?)
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

/// Sends the DevTools command on the next poll; `poll` answers it with `{ result }`, or with
/// `{ error }` when the engine refused it, isn't running, or took too long.
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
        let request = s.browser.host.send_cdp(id, cdp, params, target);
        s.browser.devtools.insert(request, (done, Instant::now() + DEVTOOLS_TIMEOUT));
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
        EngineStatus::Failed(f) => ("failed", Some(f.message)),
        EngineStatus::Unchecked | EngineStatus::Checking | EngineStatus::Installed | EngineStatus::Connecting => ("starting", None),
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
    m.raw_set("Zoom", lua.create_function(move |_, this: Table| read(&this, "BrowserTab:Zoom").map(|(app, id)| app.browser.host.tab(id).map_or(1.0, |t| t.zoom)))?)?;
    m.raw_set(
        "Navigate",
        lua.create_function(|_, (this, url): (Table, String)| later(&this, "BrowserTab:Navigate", move |s, id, _| s.navigate_browser_tab(id, &url)))?,
    )?;
    for name in ["Back", "Forward", "Reload", "Stop"] {
        m.raw_set(
            name,
            lua.create_function(move |_, this: Table| {
                let app = app_now("BrowserTab")?;
                let id = id_of(&this)?;
                let host = &app.browser.host;
                match name {
                    "Back" => host.back(id),
                    "Forward" => host.forward(id),
                    "Reload" => host.reload(id),
                    _ => host.stop(id),
                }
                Ok(())
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
    Ok(m)
}

/// What `Dep::Browser` reads now, as a number that changes with it.
pub fn version(app: &App) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    format!("{:?}", app.browser.host.status()).hash(&mut h);
    for t in app.browser.host.tabs() {
        (t.id, &t.url, &t.title, t.loading, t.can_go_back, t.can_go_forward, t.zoom.to_bits(), t.inspecting).hash(&mut h);
    }
    app.browser.host.focused_tab_id().hash(&mut h);
    let mut sessions: Vec<_> = app.browser.session_tabs.iter().collect();
    sessions.sort();
    sessions.hash(&mut h);
    (app.composer.focused, app.side.focused, format!("{:?}", app.side.active_pane_focus), app.active_browser_tab_id()).hash(&mut h);
    app.active_session_id().and_then(|sid| app.side.sessions.get(sid)).map(|st| st.open).hash(&mut h);
    h.finish()
}

/// Each open page's id and a number that moves with its address, title and loading, for
/// `Browser.Changed`.
pub fn pages(app: &App) -> Vec<(String, u64)> {
    app.browser
        .host
        .tabs()
        .iter()
        .map(|t| {
            let mut h = std::hash::DefaultHasher::new();
            (&t.url, &t.title, t.loading).hash(&mut h);
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
    // The node's key lands on this frame, so the page and the engine's states keep their own.
    Ok(col().grow(1.0).min_h(0.0).w_full().child(surface))
}

/// The engine's messages, the background download, DevTools answers and picks.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let mut changed = app.poll_browser(tx);
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
    let picks: Vec<_> = app.browser.host.drain_picked_elements().collect();
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
