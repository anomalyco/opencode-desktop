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
        let (plugin_id, is_builtin_browser) = lua
            .app_data_ref::<std::rc::Rc<Vmx>>()
            .map(|vmx| {
                let id = vmx.half.id.to_string();
                let is_builtin =
                    vmx.half.tier == crate::plugin_host::Tier::Bundled && id == "browser";
                (id, is_builtin)
            })
            .unwrap_or_default();
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
