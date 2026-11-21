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
//! - Each consumer plugin VM owns its own isolated `ExperimentalBrowserTab` handles (`btab_<uuid>`),
//!   revoked on plugin reload/unload (`revoke_owner`), tab close, or user/agent takeover navigation.
//! - Inactive (`State() == "pending"`, `Get() == nil`) on web (`wasm32`) or when the browser is off.

use std::rc::Rc;

use mlua::{Lua, Table, Value};

use super::super::cx::{self, Dep};
use super::super::vm::Vmx;
use super::method;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::Path;
    use std::rc::Rc;

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

    struct OwnedPluginTab {
        plugin_id: String,
        vm_id: u64,
        session_id: String,
        driver_name: String,
        host_tab_id: Option<u64>,
    }

    thread_local! {
        static OWNED_TABS: RefCell<HashMap<String, OwnedPluginTab>> = RefCell::default();
    }

    pub(super) fn available_now() -> bool {
        with_app("Contracts.browser.available", |app| Ok(!app.browser_unsupported())).unwrap_or(false)
    }

    pub(super) fn revoke_owner(owner: &str, keep_vm: Option<u64>) {
        OWNED_TABS.with_borrow_mut(|tabs| {
            tabs.retain(|_, t| t.plugin_id != owner || Some(t.vm_id) == keep_vm);
        });
    }

    pub(super) fn revoke_tab(host_id: u64) {
        OWNED_TABS.with_borrow_mut(|tabs| {
            tabs.retain(|_, t| t.host_tab_id != Some(host_id));
        });
    }

    enum OpenTarget {
        Http(String),
        Preview { display_url: String, spec: browser_pane::PreviewSpec },
    }

    fn parse_session_arg(v: Value) -> Option<String> {
        let raw = match v {
            Value::String(s) => s.to_str().ok()?.to_owned(),
            Value::Table(t) => t
                .get::<Option<String>>("id")
                .ok()
                .flatten()
                .or_else(|| t.get::<Option<String>>("sessionID").ok().flatten())
                .or_else(|| {
                    t.get::<Option<String>>("key")
                        .ok()
                        .flatten()
                        .map(|k| k.rsplit('\n').next().unwrap_or(&k).to_owned())
                })?,
            _ => return None,
        };
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    fn same_machine_local(app: &App) -> bool {
        let server_key = crate::servers::runtime::active(app);
        server_key.is_sidecar()
            && match &*app.source {
                crate::Source::Live { server, .. } => server.is_local(),
                crate::Source::Fixture => true,
            }
    }

    fn caller_may_access_session(lua: &Lua, app: &App, sid: &str) -> bool {
        if app.browser_unsupported() {
            return false;
        }
        let (tool_sid, in_flight) = current_tool_scope(lua);
        if let Some(caller) = tool_sid {
            if caller.as_deref() != Some(sid) {
                return false;
            }
        } else if !in_flight.iter().all(|s| s == sid) {
            return false;
        }
        app.active_session_id() == Some(sid)
            || app.tabs.iter().any(|t| !t.is_draft() && t.id == sid)
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
            if !same_machine_local(app) {
                return Err("file:// URLs are only allowed on the built-in local server; use openFile for workspace files.".into());
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
            let Some(sid) = parse_session_arg(session) else {
                return Ok(false);
            };
            with_app("Contracts.browser.attached", |app| Ok(caller_may_access_session(lua, app, &sid)))
        })?;

        method(lua, &out, "canOpen", |lua, (session, path): (Value, Option<String>)| {
            cx::read(Dep::Browser);
            let Some(sid) = parse_session_arg(session) else {
                return Ok(false);
            };
            with_app("Contracts.browser.canOpen", |app| {
                if !caller_may_access_session(lua, app, &sid) {
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
                    Ok(resolve_open_url(app, &sid, trimmed).is_ok())
                } else {
                    Ok(resolve_open_file(app, &sid, trimmed).is_ok())
                }
            })
        })?;

        let pid_open = plugin_id.clone();
        method(lua, &out, "open", move |lua, (session, url): (Value, String)| {
            let sid = parse_session_arg(session)
                .ok_or_else(|| mlua::Error::runtime("browser.open requires a session reference."))?;
            with_app("Contracts.browser.open", |app| {
                if !caller_may_access_session(lua, app, &sid) {
                    return Err(mlua::Error::runtime("browser.pane.unavailable"));
                }
                let target = resolve_open_url(app, &sid, &url).map_err(mlua::Error::runtime)?;
                let driver = browser_pane::TabDriver::plugin(&plugin_driver_name(app, &pid_open));
                let sid_owned = sid.clone();
                app.later(Box::new(move |s, tx| {
                    tx.instant();
                    let _ = open_target_now(s, &sid_owned, target, driver);
                }));
                Ok(())
            })
        })?;

        let pid_file = plugin_id.clone();
        method(lua, &out, "openFile", move |lua, (session, path): (Value, String)| {
            let sid = parse_session_arg(session)
                .ok_or_else(|| mlua::Error::runtime("browser.openFile requires a session reference."))?;
            with_app("Contracts.browser.openFile", |app| {
                if !caller_may_access_session(lua, app, &sid) {
                    return Err(mlua::Error::runtime("browser.pane.unavailable"));
                }
                let target = resolve_open_file(app, &sid, &path).map_err(mlua::Error::runtime)?;
                let driver = browser_pane::TabDriver::plugin(&plugin_driver_name(app, &pid_file));
                let sid_owned = sid.clone();
                app.later(Box::new(move |s, tx| {
                    tx.instant();
                    let _ = open_target_now(s, &sid_owned, target, driver);
                }));
                Ok(())
            })
        })?;

        let exp = lua.create_table()?;
        let v_open = vmx.clone();
        let pid_exp_open = plugin_id.clone();
        method(lua, &exp, "open", move |lua, (session, url): (Value, String)| {
            let sid = parse_session_arg(session)
                .ok_or_else(|| mlua::Error::runtime("browser.experimental.open requires a session reference."))?;
            let vm_id = v_open.vm.get();
            with_app("Contracts.browser.experimental.open", |app| {
                if !caller_may_access_session(lua, app, &sid) {
                    return Err(mlua::Error::runtime("browser.pane.unavailable"));
                }
                if app.active_session_id() != Some(sid.as_str()) {
                    return Err(mlua::Error::runtime("browser.pane.notOnScreen"));
                }
                let target = resolve_open_url(app, &sid, &url).map_err(mlua::Error::runtime)?;
                let driver_name = plugin_driver_name(app, &pid_exp_open);
                let driver = browser_pane::TabDriver::plugin(&driver_name);
                let handle = opencode_kit::browser_rpc::new_tab_id().replacen("tab_", "btab_", 1);
                let inserted = OWNED_TABS.with_borrow_mut(|tabs| {
                    let owned_count = tabs.values().filter(|t| t.plugin_id == pid_exp_open && t.vm_id == vm_id).count();
                    if owned_count >= MAX_HANDLES_PER_VM {
                        return false;
                    }
                    tabs.insert(
                        handle.clone(),
                        OwnedPluginTab {
                            plugin_id: pid_exp_open.clone(),
                            vm_id,
                            session_id: sid.clone(),
                            driver_name,
                            host_tab_id: None,
                        },
                    );
                    true
                });
                if !inserted {
                    return Err(mlua::Error::runtime("Too many open experimental browser tab handles for this plugin."));
                }
                let handle_for_cb = handle.clone();
                let sid_owned = sid.clone();
                app.later(Box::new(move |s, tx| {
                    let still_valid = OWNED_TABS.with_borrow(|tabs| tabs.contains_key(&handle_for_cb));
                    if !still_valid || s.active_session_id() != Some(sid_owned.as_str()) {
                        OWNED_TABS.with_borrow_mut(|tabs| {
                            tabs.remove(&handle_for_cb);
                        });
                        return;
                    }
                    tx.instant();
                    let host_id = open_target_now(s, &sid_owned, target, driver);
                    OWNED_TABS.with_borrow_mut(|tabs| {
                        if let Some(entry) = tabs.get_mut(&handle_for_cb) {
                            entry.host_tab_id = Some(host_id);
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
            let entry = OWNED_TABS.with_borrow_mut(|tabs| {
                let matches = tabs
                    .get(&handle)
                    .is_some_and(|t| t.plugin_id == pid_exp_close && t.vm_id == vm_id);
                if matches { tabs.remove(&handle) } else { None }
            });
            let Some(entry) = entry else {
                return Ok(false);
            };
            let Some(host_id) = entry.host_tab_id else {
                // Closed before `app.later` ran; removing the entry cancels the pending open.
                return Ok(true);
            };
            let expected_driver = browser_pane::TabDriver::plugin(&entry.driver_name);
            with_app("Contracts.browser.experimental.close", |app| {
                if app.browser_tab_session(host_id) != Some(entry.session_id.as_str()) {
                    return Ok(false);
                }
                let Some(tab) = app.workspace_panes.browser.host.tab(host_id) else {
                    return Ok(false);
                };
                // Refuse close if taken over by user/agent navigation or blocked by beforeunload.
                if tab.prompts.driver.as_ref() != Some(&expected_driver)
                    || tab
                        .prompts
                        .dialog
                        .open()
                        .is_some_and(|d| d.kind == browser_pane::DialogKind::Leave)
                {
                    return Ok(false);
                }
                let sid = entry.session_id.clone();
                app.later(Box::new(move |s, tx| {
                    if s.browser_tab_session(host_id) == Some(sid.as_str()) {
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

pub fn revoke_owner(owner: &str, keep_vm: Option<u64>) {
    #[cfg(not(target_arch = "wasm32"))]
    native::revoke_owner(owner, keep_vm);
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (owner, keep_vm);
    }
}

/// Revokes any third-party `ExperimentalBrowserTab` handle pointing to `host_id` (on tab close or
/// user takeover navigation, matching `packages/plugin/src/gui/browser.ts:43`).
pub fn revoke_tab(host_id: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    native::revoke_tab(host_id);
    #[cfg(target_arch = "wasm32")]
    {
        let _ = host_id;
    }
}

/// Builds a `Contracts:Use("browser")` handle table (`State`, `Generation`, `Get`, `Changed`).
pub fn use_handle(
    lua: &Lua,
    vmx: &Rc<Vmx>,
    make_signal: impl Fn(&Lua, &str) -> mlua::Result<Value>,
) -> mlua::Result<Table> {
    let o = lua.create_table()?;
    method(lua, &o, "State", |_, ()| {
        cx::read(Dep::Browser);
        Ok(if available() { "active" } else { "pending" })
    })?;
    method(lua, &o, "Generation", |_, ()| {
        cx::read(Dep::Browser);
        Ok(if available() { 1.0 } else { 0.0 })
    })?;
    let v_get = vmx.clone();
    method(lua, &o, "Get", move |lua, ()| {
        cx::read(Dep::Browser);
        if !available() {
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
