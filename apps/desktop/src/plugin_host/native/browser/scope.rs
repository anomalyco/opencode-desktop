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
    pub tool_sid: Option<Option<String>>,
    pub in_flight: Vec<String>,
}

impl TabScope {
    pub(super) fn capture(lua: &Lua) -> Self {
        let is_builtin_browser = lua
            .app_data_ref::<std::rc::Rc<Vmx>>()
            .is_some_and(|vmx| &*vmx.half.id == "browser");
        let (tool_sid, in_flight) = current_tool_scope(lua);
        Self { is_builtin_browser, tool_sid, in_flight }
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
