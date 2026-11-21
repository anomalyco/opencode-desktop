//! Narrow `"browser"` contract for third-party Luau plugins, matching official
//! `packages/plugin/src/gui/browser.ts:1-60` and `packages/gui-plugins/src/browser/{renderer.tsx,model.ts,pane.ts}`
//! at `db2d889c0e24daf7b34c8dede641fb01fcd02c00`.
//!
//! Exposes ONLY:
//! - `attached(session) -> boolean`
//! - `canOpen(session, path?) -> boolean`
//! - `open(session, url)`
//! - `openFile(session, path)`
//! - `experimental.open(session, url) -> ExperimentalBrowserTab` (opaque per-consumer handle)
//! - `experimental.close(id) -> boolean`
//!
//! Security boundary:
//! - Never exposes raw `BrowserTab` handles, DevTools/CDP, `ReadFileAsync`, `SiteCookiesAsync`,
//!   or raw `oc-preview://` minting.
//! - Only window halves get it, and only while the built-in browser plugin runs in a wide window
//!   (`browser.ts:56`, `model.ts:731-732`); every call is for the routed session on the window's
//!   server (`model.ts:755`).
//! - Each consumer plugin VM owns its own isolated `ExperimentalBrowserTab` handles (`btab_<uuid>`),
//!   bound to its window's half: revoked on that half's reload/unload or window close, on tab
//!   close, and on the first input or navigation the person sends the tab or an agent's touch
//!   (`pane.ts:846,1076,1113,1245`, `plugin-tabs.ts:5-26`).
//! - Opens are bounded: `OPEN_BURST` a second per VM, `MAX_LIVE_TABS` live tabs per plugin.
//! - Inactive (`State() == "pending"`, `Get() == nil`) on web (`wasm32`) or when the browser is off.

use std::rc::Rc;

use mlua::{Lua, Table, Value};

use super::super::cx::{self, Dep};
use super::super::vm::Vmx;
use super::method;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::path::Path;
    use std::rc::{Rc, Weak};
    use std::time::{Duration, Instant};

    use browser_pane::policy::{extension_origin, refusal};
    use mlua::{Lua, Table, Value};
    use url::{Host, Url};

    use super::super::super::commands::current_tool_scope;
    use super::super::super::cx::{self, Dep};
    use super::super::super::vm::Vmx;
    use super::super::{method, with_app};
    use crate::App;
    use crate::surfaces::browser::preview::PreviewTarget;

    const MAX_HANDLES_PER_VM: usize = 64;
    /// Live tabs one plugin may have opened in a window at once (`open`, `openFile` and `experimental.open`).
    const MAX_LIVE_TABS: usize = 16;
    /// Opens one VM may start per `OPEN_WINDOW`.
    const OPEN_BURST: usize = 4;
    const OPEN_WINDOW: Duration = Duration::from_secs(1);

    struct OwnedPluginTab {
        plugin_id: String,
        vm_id: u64,
        /// The owning half's token: one plugin in one window. Dead once that window closes.
        half: Weak<()>,
        session_id: String,
        driver_name: String,
        host_tab_id: Option<u64>,
        /// The tab's `user_takeovers` when it opened; any change since is the person's takeover.
        takeovers: u64,
    }

    thread_local! {
        static OWNED_TABS: RefCell<HashMap<String, OwnedPluginTab>> = RefCell::default();
        static OPENS: RefCell<HashMap<u64, VecDeque<Instant>>> = RefCell::default();
    }

    pub(super) fn available_now() -> bool {
        with_app("Contracts.browser.available", |app| Ok(app.browser_plugin_ready())).unwrap_or(false)
    }

    /// Forgets the handles of `half`'s VMs other than `keep_vm`; other windows' halves keep theirs.
    pub(super) fn revoke_half(half: &Rc<()>, keep_vm: Option<u64>) {
        OWNED_TABS.with_borrow_mut(|tabs| {
            tabs.retain(|_, t| {
                t.half.strong_count() > 0 && (!t.half.upgrade().is_some_and(|h| Rc::ptr_eq(&h, half)) || Some(t.vm_id) == keep_vm)
            });
        });
    }

    pub(super) fn revoke_tab(app: &App, host_id: u64) {
        OWNED_TABS.with_borrow_mut(|tabs| {
            tabs.retain(|_, t| t.half.strong_count() > 0 && !(t.host_tab_id == Some(host_id) && in_window(app, &t.half)));
        });
    }

    fn in_window(app: &App, half: &Weak<()>) -> bool {
        half.upgrade().is_some_and(|h| app.plugins.list.iter().any(|p| Rc::ptr_eq(&p.half.token, &h)))
    }

    enum OpenTarget {
        Http(String),
        Preview { display_url: String, spec: browser_pane::PreviewSpec },
    }

    /// A `SessionRef`: its id, and its server when the caller named one (`key` is `server\nid`).
    struct SessionArg {
        id: String,
        server: Option<String>,
    }

    fn parse_session_arg(v: Value) -> Option<SessionArg> {
        let split = |raw: &str| match raw.trim().rsplit_once('\n') {
            Some((server, id)) => (id.trim().to_owned(), Some(server.trim().to_owned())),
            None => (raw.trim().to_owned(), None),
        };
        let (id, server) = match v {
            Value::String(s) => split(&s.to_str().ok()?.to_owned()),
            Value::Table(t) => {
                let named = t.get::<Option<String>>("server").ok().flatten();
                match t.get::<Option<String>>("id").ok().flatten().or_else(|| t.get::<Option<String>>("sessionID").ok().flatten()) {
                    Some(id) => (id.trim().to_owned(), named),
                    None => {
                        let (id, keyed) = split(&t.get::<Option<String>>("key").ok().flatten()?);
                        (id, keyed.or(named))
                    }
                }
            }
            _ => return None,
        };
        (!id.is_empty()).then_some(SessionArg { id, server })
    }

    /// The caller may act only on the routed session, on the window's server, while the built-in
    /// browser runs in a wide window, and from a tool call only for that call's own session.
    fn routed_session(lua: &Lua, app: &App, arg: &SessionArg) -> bool {
        if !app.browser_plugin_ready() || app.active_session_id() != Some(arg.id.as_str()) {
            return false;
        }
        let (tool_sid, in_flight) = current_tool_scope(lua);
        if let Some(caller) = tool_sid {
            if caller.as_deref() != Some(arg.id.as_str()) {
                return false;
            }
        } else if !in_flight.iter().all(|s| *s == arg.id) {
            return false;
        }
        let active = crate::servers::runtime::active(app);
        !app.tabs.iter().any(|t| t.id == arg.id && (t.is_draft() || t.server != *active))
            && arg.server.as_deref().is_none_or(|s| s == super::super::session::server_id(active))
    }

    /// Refuses an open past `OPEN_BURST` a second for this VM or `MAX_LIVE_TABS` live tabs for this plugin.
    fn admit_open(app: &App, vm_id: u64, driver: &browser_pane::TabDriver) -> mlua::Result<()> {
        let live = app.workspace_panes.browser.host.tabs().iter().filter(|t| t.prompts.driver.as_ref() == Some(driver)).count();
        if live >= MAX_LIVE_TABS {
            return Err(mlua::Error::runtime("browser.pane.tooManyTabs"));
        }
        let now = Instant::now();
        OPENS.with_borrow_mut(|opens| {
            opens.retain(|_, times| {
                times.retain(|at| now.duration_since(*at) < OPEN_WINDOW);
                !times.is_empty()
            });
            let times = opens.entry(vm_id).or_default();
            if times.len() >= OPEN_BURST {
                return Err(mlua::Error::runtime("browser.pane.rateLimited"));
            }
            times.push_back(now);
            Ok(())
        })
    }

    /// The plugin still holds tab `host_id`: same session, still its driver, no takeover since.
    fn still_owned(app: &App, host_id: u64, session_id: &str, driver: &browser_pane::TabDriver, takeovers: u64) -> bool {
        app.browser_tab_session(host_id) == Some(session_id)
            && app
                .workspace_panes
                .browser
                .host
                .tab(host_id)
                .is_some_and(|t| t.prompts.driver.as_ref() == Some(driver) && t.prompts.user_takeovers == takeovers)
    }

    fn plugin_driver_name(app: &App, plugin_id: &str) -> String {
        app.plugins
            .list
            .iter()
            .find(|p| &*p.half.id == plugin_id)
            .map(|p| p.manifest.name.as_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(plugin_id)
            .to_owned()
    }

    fn is_absolute_workspace_path(raw: &str) -> bool {
        let trimmed = raw.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('/')
            || trimmed.starts_with('\\')
            || Path::new(trimmed).is_absolute()
        {
            return true;
        }
        let bytes = trimmed.as_bytes();
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
    }

    fn parse_http_url(raw: &str) -> Result<String, String> {
        let trimmed = raw.trim();
        if extension_origin(trimmed) {
            return Err("Extension URLs cannot be opened through the browser contract.".into());
        }
        let mut parsed = Url::parse(trimmed).map_err(|e| format!("Invalid URL '{trimmed}': {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("Only http(s) URLs (or local workspace file:// URLs on the built-in server) are allowed.".into());
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("Embedded credentials in URLs are not allowed.".into());
        }
        if let Some(Host::Domain(dom)) = parsed.host()
            && let Some(stripped) = dom.strip_suffix('.')
        {
            if stripped.is_empty() || stripped.ends_with('.') {
                return Err(format!("Invalid URL host in '{trimmed}'."));
            }
            let normalized = stripped.to_owned();
            parsed.set_host(Some(&normalized)).map_err(|e| format!("Invalid URL host: {e}"))?;
        }
        let canonical = parsed.as_str().to_owned();
        if refusal(&canonical, &[]).is_some() {
            return Err("URL was refused by browser URL policy.".into());
        }
        Ok(canonical)
    }

    fn resolve_open_url(app: &App, sid: &str, raw: &str) -> Result<OpenTarget, String> {
        let trimmed = raw.trim();
        if trimmed.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")) {
            if !app.session_server_is_local(Some(sid)) {
                return Err("file:// URLs are only allowed on the built-in local server; use openFile for workspace files.".into());
            }
            if !app.browser_preview_supported() {
                return Err("Previews require an updated browser engine.".into());
            }
            return match app.resolve_preview_target(Some(sid), trimmed, true) {
                PreviewTarget::Preview { display_url, spec } => Ok(OpenTarget::Preview { display_url, spec }),
                PreviewTarget::Refused(_) | PreviewTarget::NotPreview => {
                    Err("The file:// URL is outside this session's workspace or cannot be opened.".into())
                }
            };
        }
        parse_http_url(trimmed).map(OpenTarget::Http)
    }

    fn resolve_open_file(app: &App, sid: &str, raw_path: &str) -> Result<OpenTarget, String> {
        let trimmed = raw_path.trim();
        if is_absolute_workspace_path(trimmed) {
            return Err("openFile requires a workspace-relative path.".into());
        }
        if !app.browser_preview_supported() {
            return Err("Previews require an updated browser engine.".into());
        }
        match app.resolve_workspace_file_target(sid, trimmed, true) {
            PreviewTarget::Preview { display_url, spec } => Ok(OpenTarget::Preview { display_url, spec }),
            PreviewTarget::Refused(_) | PreviewTarget::NotPreview => {
                Err("The workspace file is outside this session's workspace or cannot be opened.".into())
            }
        }
    }

    fn open_target_now(app: &mut App, sid: &str, target: OpenTarget, driver: browser_pane::TabDriver) -> u64 {
        match target {
            OpenTarget::Http(url) => app.open_new_controlled_browser_tab_for_session(Some(sid), &url, None, true, driver),
            OpenTarget::Preview { display_url, spec } => {
                app.open_resolved_preview_tab(Some(sid), &display_url, spec, None, true, Some(driver))
            }
        }
    }

    pub(super) fn build_value(lua: &Lua, vmx: &Rc<Vmx>) -> mlua::Result<Table> {
        let out = lua.create_table()?;
        let plugin_id = vmx.half.id.to_string();

        method(lua, &out, "attached", |lua, session: Value| {
            cx::read(Dep::Browser);
            let Some(arg) = parse_session_arg(session) else {
                return Ok(false);
            };
            with_app("Contracts.browser.attached", |app| Ok(routed_session(lua, app, &arg)))
        })?;

        method(lua, &out, "canOpen", |lua, (session, path): (Value, Option<String>)| {
            cx::read(Dep::Browser);
            let Some(arg) = parse_session_arg(session) else {
                return Ok(false);
            };
            with_app("Contracts.browser.canOpen", |app| {
                if !routed_session(lua, app, &arg) {
                    return Ok(false);
                }
                let Some(raw) = path else {
                    return Ok(true);
                };
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Ok(false);
                }
                if trimmed.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://"))
                    || trimmed.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"))
                    || trimmed.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:"))
                {
                    Ok(resolve_open_url(app, &arg.id, trimmed).is_ok())
                } else {
                    Ok(resolve_open_file(app, &arg.id, trimmed).is_ok())
                }
            })
        })?;

        for (name, file) in [("open", false), ("openFile", true)] {
            let pid = plugin_id.clone();
            let vmx = vmx.clone();
            method(lua, &out, name, move |lua, (session, raw): (Value, String)| {
                let arg = parse_session_arg(session)
                    .ok_or_else(|| mlua::Error::runtime(format!("browser.{name} requires a session reference.")))?;
                let vm_id = vmx.vm.get();
                with_app("Contracts.browser.open", |app| {
                    if !routed_session(lua, app, &arg) {
                        return Err(mlua::Error::runtime("browser.pane.unavailable"));
                    }
                    let target = if file { resolve_open_file(app, &arg.id, &raw) } else { resolve_open_url(app, &arg.id, &raw) }
                        .map_err(mlua::Error::runtime)?;
                    let driver = browser_pane::TabDriver::plugin(&plugin_driver_name(app, &pid));
                    admit_open(app, vm_id, &driver)?;
                    let (sid, pid) = (arg.id, pid.clone());
                    app.later(Box::new(move |s, tx| {
                        // The plugin reloaded or stopped, or the person routed elsewhere, before this ran.
                        let running = s.plugins.list.iter().any(|p| *p.half.id == *pid && p.script().is_some_and(|sc| sc.vm_id() == Some(vm_id)));
                        if !running || s.active_session_id() != Some(sid.as_str()) {
                            return;
                        }
                        tx.instant();
                        let _ = open_target_now(s, &sid, target, driver);
                    }));
                    Ok(())
                })
            })?;
        }

        let exp = lua.create_table()?;
        let v_open = vmx.clone();
        let pid_exp_open = plugin_id.clone();
        method(lua, &exp, "open", move |lua, (session, url): (Value, String)| {
            let arg = parse_session_arg(session)
                .ok_or_else(|| mlua::Error::runtime("browser.experimental.open requires a session reference."))?;
            let vm_id = v_open.vm.get();
            with_app("Contracts.browser.experimental.open", |app| {
                if !app.browser_plugin_ready() {
                    return Err(mlua::Error::runtime("browser.pane.unavailable"));
                }
                if !routed_session(lua, app, &arg) {
                    return Err(mlua::Error::runtime("browser.pane.notOnScreen"));
                }
                let target = resolve_open_url(app, &arg.id, &url).map_err(mlua::Error::runtime)?;
                let driver_name = plugin_driver_name(app, &pid_exp_open);
                let driver = browser_pane::TabDriver::plugin(&driver_name);
                let held = OWNED_TABS.with_borrow(|tabs| {
                    tabs.values().filter(|t| t.plugin_id == pid_exp_open && t.vm_id == vm_id && t.half.strong_count() > 0).count()
                });
                if held >= MAX_HANDLES_PER_VM {
                    return Err(mlua::Error::runtime("Too many open experimental browser tab handles for this plugin."));
                }
                admit_open(app, vm_id, &driver)?;
                let handle = opencode_kit::browser_rpc::new_tab_id().replacen("tab_", "btab_", 1);
                OWNED_TABS.with_borrow_mut(|tabs| {
                    tabs.insert(
                        handle.clone(),
                        OwnedPluginTab {
                            plugin_id: pid_exp_open.clone(),
                            vm_id,
                            half: Rc::downgrade(&v_open.half.token),
                            session_id: arg.id.clone(),
                            driver_name,
                            host_tab_id: None,
                            takeovers: 0,
                        },
                    );
                });
                let handle_for_cb = handle.clone();
                let sid = arg.id;
                app.later(Box::new(move |s, tx| {
                    let live = OWNED_TABS.with_borrow(|tabs| tabs.get(&handle_for_cb).is_some_and(|t| t.half.strong_count() > 0));
                    if !live || s.active_session_id() != Some(sid.as_str()) {
                        OWNED_TABS.with_borrow_mut(|tabs| tabs.remove(&handle_for_cb));
                        return;
                    }
                    tx.instant();
                    let host_id = open_target_now(s, &sid, target, driver);
                    let takeovers = s.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.prompts.user_takeovers);
                    OWNED_TABS.with_borrow_mut(|tabs| {
                        if let Some(entry) = tabs.get_mut(&handle_for_cb) {
                            entry.host_tab_id = Some(host_id);
                            entry.takeovers = takeovers;
                        }
                    });
                }));
                Ok(handle)
            })
        })?;

        let v_close = vmx.clone();
        let pid_exp_close = plugin_id;
        method(lua, &exp, "close", move |_, handle: String| {
            let vm_id = v_close.vm.get();
            let entry = OWNED_TABS.with_borrow(|tabs| {
                tabs.get(&handle)
                    .filter(|t| t.plugin_id == pid_exp_close && t.vm_id == vm_id && t.half.strong_count() > 0)
                    .map(|t| (t.host_tab_id, t.session_id.clone(), t.driver_name.clone(), t.takeovers))
            });
            let Some((host_id, sid, driver_name, takeovers)) = entry else {
                return Ok(false);
            };
            let Some(host_id) = host_id else {
                // Closed before `app.later` ran; removing the entry cancels the pending open.
                OWNED_TABS.with_borrow_mut(|tabs| tabs.remove(&handle));
                return Ok(true);
            };
            let driver = browser_pane::TabDriver::plugin(&driver_name);
            with_app("Contracts.browser.experimental.close", |app| {
                // The person's input or navigation, or an agent's touch, ended this plugin's right.
                if !still_owned(app, host_id, &sid, &driver, takeovers) {
                    OWNED_TABS.with_borrow_mut(|tabs| tabs.remove(&handle));
                    return Ok(false);
                }
                // A tab waiting on the person (a page dialog, "Leave site?", a sign-in, a permission)
                // is theirs to answer; the handle stays.
                if app.workspace_panes.browser.host.tab(host_id).is_some_and(|t| t.prompts.waiting_on_user()) {
                    return Ok(false);
                }
                // No takeover since the tab opened means the person never gave this page input, so it
                // holds none of their work, and Chromium shows no "Leave site?" to a page without a
                // user activation: closing now is what official's `leave()` answers for such a page.
                OWNED_TABS.with_borrow_mut(|tabs| tabs.remove(&handle));
                app.later(Box::new(move |s, tx| {
                    let waiting = s.workspace_panes.browser.host.tab(host_id).is_some_and(|t| t.prompts.waiting_on_user());
                    if still_owned(s, host_id, &sid, &driver, takeovers) && !waiting {
                        tx.instant();
                        s.close_browser_tab_for_session(&sid, host_id, tx);
                    }
                }));
                Ok(true)
            })
        })?;

        out.raw_set("experimental", exp)?;
        Ok(out)
    }
}

pub fn available() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::available_now()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// Forgets the handles of `half`'s VMs other than `keep_vm` (its reload, stop or window close).
pub fn revoke_half(half: &Rc<()>, keep_vm: Option<u64>) {
    #[cfg(not(target_arch = "wasm32"))]
    native::revoke_half(half, keep_vm);
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (half, keep_vm);
    }
}

/// Revokes this window's `ExperimentalBrowserTab` handles for `host_id` (tab close or address-bar
/// takeover, `packages/plugin/src/gui/browser.ts:43`); other takeovers fail `close`'s own check.
pub fn revoke_tab(app: &crate::App, host_id: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    native::revoke_tab(app, host_id);
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, host_id);
    }
}

/// Builds a `Contracts:Use("browser")` handle table (`State`, `Generation`, `Get`, `Changed`).
pub fn use_handle(
    lua: &Lua,
    vmx: &Rc<Vmx>,
    make_signal: impl Fn(&Lua, &str) -> mlua::Result<Value>,
) -> mlua::Result<Table> {
    let o = lua.create_table()?;
    // Pane tabs belong to a window: an app half (`app.luau`) never gets the contract.
    let window = vmx.half.kind == crate::plugin_host::plugin::Kind::Window;
    method(lua, &o, "State", move |_, ()| {
        cx::read(Dep::Browser);
        Ok(if window && available() { "active" } else { "pending" })
    })?;
    method(lua, &o, "Generation", move |_, ()| {
        cx::read(Dep::Browser);
        Ok(if window && available() { 1.0 } else { 0.0 })
    })?;
    let v_get = vmx.clone();
    method(lua, &o, "Get", move |lua, ()| {
        cx::read(Dep::Browser);
        if !window || !available() {
            return Ok(Value::Nil);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(Value::Table(native::build_value(lua, &v_get)?))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (&lua, &v_get);
            Ok(Value::Nil)
        }
    })?;
    o.raw_set("Changed", make_signal(lua, "contract:browser")?)?;
    Ok(o)
}
