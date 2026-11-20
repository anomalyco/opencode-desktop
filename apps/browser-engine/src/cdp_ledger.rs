//! Engine-authoritative ledger of verified non-extension OOPIF `sessionId`s and default
//! main-world `contextId`s for `HostMsg::TypedCdpCall`.
//!
//! Security boundary: the engine NEVER trusts a desktop-supplied `contextId` or `session_id`
//! blindly. Every `(contextId, sessionId)` pair and every flattened OOPIF `sessionId` must be
//! independently observed by the engine's own `DevToolsMessageObserver` as a non-extension
//! document's default main world; extension OOPIFs (or OOPIFs that navigate to an extension
//! origin) are banned immediately and detached via `Target.detachFromTarget`.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

#[derive(Default)]
pub struct EngineCdpLedger {
    allowed_sessions: HashSet<String>,
    banned_sessions: HashSet<String>,
    frame_to_session: HashMap<String, String>,
    allowed_contexts: HashSet<(i64, Option<String>)>,
}

impl EngineCdpLedger {
    /// Updates the ledger from one raw DevTools event observed directly on the engine.
    /// Returns any newly detected extension OOPIF `sessionId` that the engine should immediately
    /// detach via `Target.detachFromTarget`.
    pub fn observe_event(
        &mut self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
    ) -> Option<String> {
        match method {
            "Target.attachedToTarget" => {
                let info = params.get("targetInfo")?;
                if info.get("type").and_then(Value::as_str) != Some("iframe") {
                    return None;
                }
                let child_sid = params.get("sessionId").and_then(Value::as_str)?;
                let target_id = info.get("targetId").and_then(Value::as_str).unwrap_or("");
                let url = info.get("url").and_then(Value::as_str).unwrap_or("");
                let parent_banned = session_id.is_some_and(|s| self.banned_sessions.contains(s));
                if browser_policy::extension_origin(url) || parent_banned {
                    self.ban_session(child_sid);
                    return Some(child_sid.to_owned());
                }
                self.allowed_sessions.insert(child_sid.to_owned());
                if !target_id.is_empty() {
                    self.frame_to_session.insert(target_id.to_owned(), child_sid.to_owned());
                }
            }
            "Target.targetInfoChanged" => {
                let info = params.get("targetInfo")?;
                if info.get("type").and_then(Value::as_str) != Some("iframe") {
                    return None;
                }
                let target_id = info.get("targetId").and_then(Value::as_str)?;
                let url = info.get("url").and_then(Value::as_str).unwrap_or("");
                if browser_policy::extension_origin(url)
                    && let Some(sid) = self.frame_to_session.remove(target_id)
                {
                    self.ban_session(&sid);
                    return Some(sid);
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(sid) = params.get("sessionId").and_then(Value::as_str) {
                    self.allowed_sessions.remove(sid);
                    self.banned_sessions.remove(sid);
                    self.frame_to_session.retain(|_, v| v != sid);
                    self.allowed_contexts.retain(|(_, s)| s.as_deref() != Some(sid));
                }
            }
            "Page.frameNavigated" => {
                if let Some(frame) = params.get("frame")
                    && let Some(url) = frame.get("url").and_then(Value::as_str)
                    && browser_policy::extension_origin(url)
                    && let Some(sid) = session_id
                {
                    self.ban_session(sid);
                    return Some(sid.to_owned());
                }
            }
            "Runtime.executionContextCreated" => {
                if session_id.is_some_and(|s| self.banned_sessions.contains(s)) {
                    return None;
                }
                if let Some(sid) = session_id
                    && !self.allowed_sessions.contains(sid)
                {
                    return None;
                }
                if let Some(ctx) = params.get("context")
                    && let Some(cid) = ctx.get("id").and_then(Value::as_i64)
                    && let Some(aux) = ctx.get("auxData")
                    && aux.get("isDefault").and_then(Value::as_bool) == Some(true)
                    && aux.get("type").and_then(Value::as_str).unwrap_or("default") == "default"
                {
                    let origin = ctx.get("origin").and_then(Value::as_str).unwrap_or("");
                    if browser_policy::extension_origin(origin) {
                        if let Some(sid) = session_id {
                            self.ban_session(sid);
                            return Some(sid.to_owned());
                        }
                    } else {
                        self.allowed_contexts.insert((cid, session_id.map(str::to_owned)));
                    }
                }
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(cid) = params.get("executionContextId").and_then(Value::as_i64) {
                    self.allowed_contexts
                        .retain(|(id, s)| !(*id == cid && s.as_deref() == session_id));
                }
            }
            "Runtime.executionContextsCleared" => {
                self.allowed_contexts.retain(|(_, s)| s.as_deref() != session_id);
            }
            _ => {}
        }
        None
    }

    fn ban_session(&mut self, sid: &str) {
        self.allowed_sessions.remove(sid);
        self.banned_sessions.insert(sid.to_owned());
        self.frame_to_session.retain(|_, v| v != sid);
        self.allowed_contexts.retain(|(_, s)| s.as_deref() != Some(sid));
    }

    /// Verifies that a `TypedCdpCall`'s `(method, params, session_id)` targets ONLY an
    /// engine-verified non-extension session and (when `contextId` is supplied on
    /// `Runtime.evaluate`) an engine-verified default main-world context ID.
    pub fn verify_typed_call(
        &self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
    ) -> Result<(), &'static str> {
        if let Some(sid) = session_id {
            if method != "Target.detachFromTarget"
                && (self.banned_sessions.contains(sid) || !self.allowed_sessions.contains(sid))
            {
                return Err("TypedCdpCall sessionId is not an engine-verified non-extension iframe session.");
            }
        }
        if method == "Runtime.evaluate"
            && let Some(cid_val) = params.get("contextId")
            && !cid_val.is_null()
        {
            let Some(cid) = cid_val.as_i64() else {
                return Err("TypedCdpCall contextId must be a verified integer.");
            };
            let key = (cid, session_id.map(str::to_owned));
            if !self.allowed_contexts.contains(&key) {
                return Err("TypedCdpCall contextId is not an engine-verified main-world execution context.");
            }
        }
        Ok(())
    }
}

/// Checks whether the tab's snapshot, live main frame, or any live sub-frame is an extension origin.
pub(crate) fn tab_has_extension_frame(
    browser: &cef::Browser,
    snap: &std::sync::Mutex<crate::wire::TabSnapshot>,
) -> bool {
    use cef::{CefString, ImplBrowser, ImplFrame};

    if browser_policy::extension_origin(&snap.lock().unwrap().url) {
        return true;
    }
    if let Some(frame) = browser.main_frame() {
        let url = CefString::from(&frame.url()).to_string();
        if browser_policy::extension_origin(&url) {
            return true;
        }
    }
    let mut ids = cef::CefStringList::new();
    browser.frame_identifiers(Some(&mut ids));
    for id in crate::menu::read_cef_string_list(&ids) {
        let cef_id = CefString::from(id.as_str());
        if let Some(frame) = browser.frame_by_identifier(Some(&cef_id)) {
            let url = CefString::from(&frame.url()).to_string();
            if browser_policy::extension_origin(&url) {
                return true;
            }
        }
    }
    false
}

/// Validates a `HostMsg::TypedCdpCall` against readiness, extension frames, `check_typed_cdp`,
/// and the engine-authoritative `EngineCdpLedger`.
pub(crate) fn check_typed_call_allowed(
    tab_ready: bool,
    browser: &cef::Browser,
    snap: &std::sync::Mutex<crate::wire::TabSnapshot>,
    ledger: &std::sync::Mutex<EngineCdpLedger>,
    method: &str,
    params: &Value,
    session_id: Option<&str>,
) -> Result<(), String> {
    if !tab_ready {
        return Err("browser.context.initializing".into());
    }
    if tab_has_extension_frame(browser, snap) {
        return Err("Agent browser tools cannot run DevTools methods on tabs containing extension documents.".into());
    }
    browser_policy::check_typed_cdp(method, params, true)?;
    ledger.lock().unwrap().verify_typed_call(method, params, session_id).map_err(str::to_owned)
}
