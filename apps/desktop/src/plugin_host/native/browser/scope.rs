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
            .is_some_and(|vmx| {
                vmx.half.tier == crate::plugin_host::Tier::Bundled && &*vmx.half.id == "browser"
            });
        let (tool_sid, in_flight) = current_tool_scope(lua);
        Self { is_builtin_browser, tool_sid, in_flight }
    }

    /// Whether the caller may access tab `id` (`mutate = true` for actions/CDP/cookies/viewport/
    /// downloads/navigation/close; `false` for passive metadata reads).
    pub(super) fn check_tab(&self, app: &App, id: u64, mutate: bool) -> Result<(), String> {
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
