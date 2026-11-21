//! Per-session `BrowserTab` ownership enforcement for the Luau `Browser` service and
//! `BrowserTab` methods (`notes/review-browser-policy.md` §4, `notes/for-dax.md` §0).
//!
//! - Inside any MCP tool coroutine (`current_tool_scope` reports `Some(caller)`), every read,
//!   mutation, lookup (`Browser:Get`/`Tabs`/`Active`/`Selected`), URL normalization, and tab open
//!   is confined strictly to `caller`.
//! - Outside MCP tool coroutines, third-party plugins (`!is_builtin_browser`) are confined to the
//!   active session's tabs and blocked whenever an in-flight MCP tool belongs to another session
//!   (preventing `task.spawn` escapes).
//! - The built-in `browser` plugin's UI outside tool coroutines may read background tabs (for
//!   `Browser.Changed` epochs/history) and mutate tabs of the active session.

use mlua::Lua;

use super::super::super::commands::current_tool_scope;
use super::super::super::vm::Vmx;
use crate::App;

#[derive(Clone, Debug)]
pub(super) struct TabScope {
    pub is_builtin_browser: bool,
    pub plugin_id: String,
    pub tool_sid: Option<Option<String>>,
    pub in_flight: Vec<String>,
}

impl TabScope {
    pub(super) fn capture(lua: &Lua) -> Self {
        let (is_builtin_browser, plugin_id) = lua
            .app_data_ref::<std::rc::Rc<Vmx>>()
            .map(|vmx| (vmx.half.is_builtin("browser"), vmx.half.id.to_string()))
            .unwrap_or((false, String::new()));
        let (tool_sid, in_flight) = current_tool_scope(lua);
        Self { is_builtin_browser, plugin_id, tool_sid, in_flight }
    }

    /// Returns the controlling driver when this caller is an MCP tool (`Agent`) or a third-party
    /// Luau plugin (`Plugin(display_name)`), or `None` for the bundled built-in browser chrome UI.
    pub(super) fn driver(&self, app: &App) -> Option<browser_pane::TabDriver> {
        if self.tool_sid.is_some() {
            return Some(browser_pane::TabDriver::Agent);
        }
        if !self.is_builtin_browser {
            let name = app
                .plugins
                .list
                .iter()
                .find(|p| &*p.half.id == self.plugin_id.as_str())
                .map(|p| p.manifest.name.as_str())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(self.plugin_id.as_str());
            return Some(browser_pane::TabDriver::plugin(name));
        }
        None
    }

    /// Marks `tab_id` controlled when driven by an MCP tool or third-party plugin.
    pub(super) fn mark_tab_driven(&self, app: &mut App, tab_id: u64) {
        if let Some(driver) = self.driver(app) {
            app.workspace_panes.browser.host.set_tab_driver(tab_id, driver);
        }
    }

    pub(super) fn marks_tab_driven(what: &str) -> bool {
        !matches!(
            what,
            "BrowserTab:Close"
                | "BrowserTab:Show"
                | "BrowserTab:SetZoom"
                | "BrowserTab:Find"
                | "BrowserTab:StopFinding"
                | "BrowserTab:SetViewport"
        )
    }

    /// Central enforcement for Luau access to `Browser:*` / `BrowserTab:*`:
    /// - Built-in `browser` plugin UI callbacks outside MCP tool calls (`tool_sid.is_none() &&
    ///   in_flight.is_empty()`) are permitted so the browser pane chrome stays functional.
    /// - Every third-party plugin (`!self.is_builtin_browser`) — regardless of whether invoked
    ///   inside an MCP tool, via `task.delay`/`task.defer`/event handler, via another plugin's
    ///   `Contracts`, or outside MCP — AND every built-in MCP tool call is unconditionally gated
    ///   on remote-server refusal and `experimental.browser` suppression.
    pub(super) fn check_local_mcp_allowed(&self, app: &App) -> Result<(), String> {
        if self.is_builtin_browser && self.tool_sid.is_none() && self.in_flight.is_empty() {
            return Ok(());
        }
        let server_key = crate::servers::runtime::active(app);
        let same_machine = server_key.is_sidecar()
            && match &*app.source {
                crate::Source::Live { server, .. } => server.is_local(),
                crate::Source::Fixture => true,
            };
        if !same_machine {
            return Err(crate::rune_mcp::REMOTE_BROWSER_MCP_ERROR.to_owned());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let suppressed = match &self.tool_sid {
                Some(caller_opt) => crate::surfaces::browser::rpc::is_mcp_browser_suppressed(
                    app,
                    server_key,
                    caller_opt.as_deref(),
                ),
                None => {
                    crate::surfaces::browser::rpc::is_mcp_browser_suppressed(
                        app,
                        server_key,
                        app.active_session_id(),
                    ) || self.in_flight.iter().any(|sid| {
                        crate::surfaces::browser::rpc::is_mcp_browser_suppressed(
                            app,
                            server_key,
                            Some(sid.as_str()),
                        )
                    })
                }
            };
            if suppressed {
                return Err(crate::rune_mcp::RPC_ATTACHED_BROWSER_MCP_ERROR.to_owned());
            }
        }
        Ok(())
    }

    /// Whether the caller may access tab `id` (`mutate = true` for actions/CDP/cookies/viewport/
    /// downloads/navigation/close; `false` for passive metadata reads).
    pub(super) fn check_tab(&self, app: &App, id: u64, mutate: bool) -> Result<(), String> {
        self.check_local_mcp_allowed(app)?;
        // Third-party plugins may NEVER script, navigate, or attach DevTools to preview/artifact tabs
        // (`notes/review-rd281.md` PR 2 Blocker #1).
        if mutate
            && !self.is_builtin_browser
            && app.workspace_panes.browser.host.tab(id).is_some_and(|t| t.preview.is_some())
        {
            return Err("Third-party plugins cannot control or attach DevTools to preview tabs.".into());
        }
        let Some(owner) = app.browser_tab_session(id) else {
            if self.tool_sid.is_none() && self.is_builtin_browser && self.in_flight.is_empty() {
                return Ok(());
            }
            return Err(format!("Browser tab {id} does not belong to an open session."));
        };
        let allowed = match &self.tool_sid {
            Some(caller) => caller.as_deref() == Some(owner),
            None => {
                if !mutate && self.is_builtin_browser {
                    true
                } else {
                    app.active_session_id() == Some(owner)
                        && (self.is_builtin_browser || self.in_flight.iter().all(|sid| sid == owner))
                }
            }
        };
        if !allowed {
            return Err(format!("Browser tab {id} belongs to another session."));
        }
        Ok(())
    }

    /// Resolves which session ID a caller may target for `Browser:Tabs`, `Active`, `Selected`,
    /// `NormalizeUrl`, `OpenAsync`, or `BrowserTab:Show`. Returns `None` when a scoped caller
    /// tries to cross into another session.
    pub(super) fn target_session(&self, app: &App, requested: Option<&str>) -> Option<String> {
        if self.check_local_mcp_allowed(app).is_err() {
            return None;
        }
        match &self.tool_sid {
            Some(Some(caller)) => match requested {
                Some(req) if req != caller => None,
                _ => Some(caller.clone()),
            },
            Some(None) => None,
            None if self.is_builtin_browser => requested
                .map(str::to_owned)
                .or_else(|| app.active_session_id().map(str::to_owned)),
            None => {
                let active = app.active_session_id()?;
                if requested.is_some_and(|req| req != active) {
                    return None;
                }
                if !self.in_flight.iter().all(|sid| sid == active) {
                    return None;
                }
                Some(active.to_owned())
            }
        }
    }
}

/// Whether a third-party plugin may hear passive signals (`Browser.Changed` / `Browser.Picked`)
/// about `tab_id`: strictly limited to non-isolated tabs owned by the currently active session.
pub(super) fn tab_visible_to_third_party(app: &App, tab_id: u64) -> bool {
    app.active_session_id().is_some_and(|sid| app.browser_tab_session(tab_id) == Some(sid))
        && app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| !t.context.is_isolated())
}
