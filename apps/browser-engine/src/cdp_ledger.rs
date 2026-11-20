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
    /// engine-verified non-extension session and (for `Runtime.evaluate`) an engine-verified
    /// default main-world `contextId` (fail-closed: `contextId` is REQUIRED on `TypedCdpCall`).
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
        if method == "Runtime.evaluate" {
            let Some(cid) = params.get("contextId").and_then(Value::as_i64) else {
                return Err("TypedCdpCall Runtime.evaluate requires a verified integer contextId.");
            };
            let key = (cid, session_id.map(str::to_owned));
            if !self.allowed_contexts.contains(&key) {
                return Err("TypedCdpCall contextId is not an engine-verified main-world execution context.");
            }
        }
        Ok(())
    }
}

/// Hard engine-side byte cap on `Runtime.evaluate` / `Runtime.callFunctionOn` CDP responses
/// (enforced BEFORE JSON parsing and IPC forwarding so giant eval results cannot OOM engine or UI).
pub(crate) const MAX_CDP_EVAL_RESPONSE_BYTES: usize = 1024 * 1024;
/// Hard engine-side byte cap on all other CDP responses (`Page.captureScreenshot`, AX tree, etc.).
pub(crate) const MAX_CDP_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn max_cdp_response_bytes(method: &str) -> usize {
    if matches!(method, "Runtime.evaluate" | "Runtime.callFunctionOn") {
        MAX_CDP_EVAL_RESPONSE_BYTES
    } else {
        MAX_CDP_RESPONSE_BYTES
    }
}

/// Validates and dispatches one `HostMsg::TypedCdpCall` against readiness, `#274`'s full
/// whole-tab extension surface guard (`tab_is_extension_surface`), `check_typed_cdp`, and the
/// engine-authoritative `EngineCdpLedger`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_typed_cdp_call(
    tab_id: u64,
    request_id: u64,
    not_ready_reason: Option<&str>,
    t: &crate::engine::ManagedTab,
    next_cdp_id: &std::sync::atomic::AtomicI32,
    out_tx: &crate::output::Output,
    method: String,
    params: Value,
    session_id: Option<String>,
) {
    use cef::ImplBrowserHost;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    let err = if let Some(reason) = not_ready_reason {
        Some(reason.to_owned())
    } else if crate::engine::tab_is_extension_surface(t) {
        Some("Agent browser tools cannot read or act on extension pages or tabs with extension frames.".into())
    } else if let Err(e) = browser_policy::check_typed_cdp(&method, &params, true) {
        Some(e)
    } else {
        t.cdp_ledger
            .lock()
            .unwrap()
            .verify_typed_call(&method, &params, session_id.as_deref())
            .err()
            .map(str::to_owned)
    };
    if let Some(err) = err {
        let _ = out_tx.send(crate::wire::EngineMsg::CdpResponse {
            tab_id,
            request_id,
            ok: false,
            result: json!({ "message": err }),
        });
        return;
    }
    let msg_id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
    t.cdp_id_map.lock().unwrap().insert(
        msg_id,
        crate::handlers::CdpPending::Raw {
            request_id,
            max_bytes: max_cdp_response_bytes(&method),
        },
    );
    let mut payload = json!({ "id": msg_id, "method": method, "params": params });
    if let Some(sid) = session_id {
        payload["sessionId"] = Value::String(sid);
    }
    let raw = payload.to_string();
    if t.host.send_dev_tools_message(Some(raw.as_bytes())) != 1 {
        t.cdp_id_map.lock().unwrap().remove(&msg_id);
        let _ = out_tx.send(crate::wire::EngineMsg::CdpResponse {
            tab_id,
            request_id,
            ok: false,
            result: json!({ "message": "send_dev_tools_message rejected" }),
        });
    }
}
