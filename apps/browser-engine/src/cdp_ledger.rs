//! Engine-authoritative ledger of verified non-extension OOPIF `sessionId`s and default
//! main-world `contextId`s for `HostMsg::TypedCdpCall`.
//!
//! Security boundary: the engine NEVER trusts a desktop-supplied `contextId` or `session_id`
//! blindly. Every `(contextId, sessionId)` pair and every flattened OOPIF `sessionId` must be
//! independently observed by the engine's own `DevToolsMessageObserver` as a non-extension
//! document's default main world; extension OOPIFs (or OOPIFs that navigate to an extension
//! origin) are banned immediately and detached via `Target.detachFromTarget`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    Browser, DevToolsMessageObserver, ImplBrowser, ImplBrowserHost, ImplDevToolsMessageObserver,
    WrapDevToolsMessageObserver, wrap_dev_tools_message_observer,
};
use crate::output::Output;
use crate::wire::{EngineMsg, TabSnapshot};

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

pub(crate) fn handle_site_cookies(
    tab_id: u64,
    request_id: u64,
    not_ready_reason: Option<&str>,
    t: &crate::engine::ManagedTab,
    next_cdp_id: &std::sync::atomic::AtomicI32,
    out_tx: &crate::output::Output,
) {
    use cef::ImplBrowserHost;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    if let Some(reason) = not_ready_reason {
        let _ = out_tx.send(crate::wire::EngineMsg::CdpResponse {
            tab_id,
            request_id,
            ok: false,
            result: json!({ "message": reason }),
        });
        return;
    }
    let url = t.snap.lock().unwrap().url.clone();
    if browser_policy::destination_origin(&url).is_none() {
        let _ = out_tx.send(crate::wire::EngineMsg::CdpResponse {
            tab_id,
            request_id,
            ok: true,
            result: json!({ "cookies": 0 }),
        });
        return;
    }
    let msg_id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
    t.cdp_id_map.lock().unwrap().insert(msg_id, crate::handlers::CdpPending::CookieCount(request_id));
    let raw = json!({
        "id": msg_id,
        "method": "Network.getCookies",
        "params": { "urls": [url] },
    })
    .to_string();
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

/// Top-level JSON envelope fields of a raw CDP message extracted without building a `Value` AST.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TopLevelCdp {
    pub id: Option<i32>,
    pub has_method: bool,
    pub has_session_id: bool,
}

/// Streaming depth-tracking scanner that inspects ONLY root-level (`depth == 1`) JSON keys so
/// nested `"id"`/`"sessionId"` inside `params` or `result` can never spoof reply/event routing.
pub(crate) fn scan_top_level_cdp(raw: &[u8]) -> TopLevelCdp {
    let mut out = TopLevelCdp::default();
    let n = raw.len();
    let mut i = 0usize;
    while i < n && raw[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= n || raw[i] != b'{' {
        return out;
    }
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    let mut expect_key = false;
    while i < n {
        let b = raw[i];
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'{' => {
                depth += 1;
                if depth == 1 {
                    expect_key = true;
                }
                i += 1;
            }
            b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
                i += 1;
            }
            b',' => {
                if depth == 1 {
                    expect_key = true;
                }
                i += 1;
            }
            b'"' if depth == 1 && expect_key => {
                expect_key = false;
                let key_start = i + 1;
                i += 1;
                let mut esc = false;
                while i < n {
                    if esc {
                        esc = false;
                    } else if raw[i] == b'\\' {
                        esc = true;
                    } else if raw[i] == b'"' {
                        break;
                    }
                    i += 1;
                }
                let key = &raw[key_start..i.min(n)];
                if i < n {
                    i += 1;
                }
                while i < n && raw[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i < n && raw[i] == b':' {
                    i += 1;
                    while i < n && raw[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if key == b"method" {
                        out.has_method = true;
                        break;
                    } else if key == b"sessionId" {
                        out.has_session_id = true;
                    } else if key == b"id" {
                        let num_start = i;
                        if i < n && raw[i] == b'-' {
                            i += 1;
                        }
                        while i < n && raw[i].is_ascii_digit() {
                            i += 1;
                        }
                        if let Ok(s) = std::str::from_utf8(&raw[num_start..i]) {
                            out.id = s.parse::<i32>().ok();
                        }
                    }
                }
            }
            b'"' => {
                in_str = true;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    out
}


pub enum CdpPending {
    Raw { request_id: u64, max_bytes: usize },
    CookieCount(u64),
    ClearSite { request_id: u64, origin: String },
}

pub struct PendingClear {
    pub tab_id: u64,
    pub request_id: u64,
    pub origin: String,
    pub cookies: Vec<(String, String, String)>,
}

wrap_dev_tools_message_observer! {
    pub struct TabCdpObserver {
        tab_id: u64,
        is_popup: bool,
        isolated: bool,
        is_preview: bool,
        snap: Arc<Mutex<TabSnapshot>>,
        id_map: Arc<Mutex<HashMap<i32, CdpPending>>>,
        internal_picked: Arc<Mutex<Vec<i64>>>,
        pending_clears: Arc<Mutex<Vec<PendingClear>>>,
        cdp_ledger: Arc<Mutex<crate::cdp_ledger::EngineCdpLedger>>,
        prompts: Arc<crate::prompts::TabPrompts>,
        permissions: Arc<crate::permissions::PermissionTracker>,
        extensions_active: bool,
        out_tx: Output,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            browser: Option<&mut Browser>,
            message_id: i32,
            success: i32,
            result: Option<&[u8]>,
        ) {
            let Some(pending) = self.id_map.lock().unwrap().remove(&message_id) else {
                return;
            };
            // Guard 1b (reply-side choke point): re-check committed origin uniformly before ANY
            // `CdpPending` variant (`Raw`, `CookieCount`, `ClearSite`) leaves the engine.
            let pending_req_id = match &pending {
                CdpPending::Raw { request_id, .. }
                | CdpPending::CookieCount(request_id)
                | CdpPending::ClearSite { request_id, .. } => *request_id,
            };
            if let Some(blocked_url) = self.permissions.tab_blocked_url(
                browser.as_deref(),
                self.isolated,
                self.is_preview,
                &self.snap,
                &self.prompts,
            ) {
                let _ = self.out_tx.send(EngineMsg::ControlledPermissionRefused {
                    tab_id: self.tab_id,
                    url: blocked_url,
                });
                let _ = self.out_tx.send(EngineMsg::CdpResponse {
                    tab_id: self.tab_id,
                    request_id: pending_req_id,
                    ok: false,
                    result: serde_json::json!({
                        "message": crate::permissions::INHERITED_PERMISSION_MSG
                    }),
                });
                return;
            }
            if let CdpPending::Raw { request_id, max_bytes } = pending {
                // Enforce hard byte cap BEFORE parsing JSON or forwarding across IPC.
                if result.is_some_and(|b| b.len() > max_bytes) {
                    let _ = self.out_tx.send(EngineMsg::CdpResponse {
                        tab_id: self.tab_id,
                        request_id,
                        ok: false,
                        result: serde_json::json!({
                            "message": format!("CDP response exceeded maximum allowed size ({max_bytes} bytes).")
                        }),
                    });
                    return;
                }
                // Re-check extension surface at reply time so an extension frame appearing
                // mid-flight (e.g. during `Page.captureScreenshot` or awaited eval) can never
                // leak into the CDP response (`review-rd274.md` Round-5 §2).
                if crate::engine::browser_has_extension_surface(
                    browser.as_deref(),
                    self.is_popup,
                    &self.snap,
                    &self.prompts,
                ) {
                    let _ = self.out_tx.send(EngineMsg::CdpResponse {
                        tab_id: self.tab_id,
                        request_id,
                        ok: false,
                        result: serde_json::json!({
                            "message": "Agent browser tools cannot read or act on extension pages or tabs with extension frames."
                        }),
                    });
                    return;
                }
                let val = result
                    .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                    .unwrap_or(Value::Null);
                let _ = self.out_tx.send(EngineMsg::CdpResponse {
                    tab_id: self.tab_id,
                    request_id,
                    ok: success != 0,
                    result: val,
                });
                return;
            }
            let val = result
                .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                .unwrap_or(Value::Null);
            match pending {
                CdpPending::Raw { .. } => unreachable!(),
                CdpPending::CookieCount(req_id) => {
                    let count = val.get("cookies").and_then(Value::as_array).map_or(0, Vec::len);
                    let _ = self.out_tx.send(EngineMsg::CdpResponse {
                        tab_id: self.tab_id,
                        request_id: req_id,
                        ok: success != 0,
                        result: serde_json::json!({ "cookies": count }),
                    });
                }
                CdpPending::ClearSite { request_id, origin } => {
                    if success == 0 {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id: self.tab_id,
                            request_id,
                            ok: false,
                            result: val,
                        });
                        return;
                    }
                    let cookies = val
                        .get("cookies")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|c| {
                                    Some((
                                        c.get("name")?.as_str()?.to_owned(),
                                        c.get("domain")?.as_str()?.to_owned(),
                                        c.get("path")?.as_str().unwrap_or("/").to_owned(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    self.pending_clears.lock().unwrap().push(PendingClear {
                        tab_id: self.tab_id,
                        request_id,
                        origin,
                        cookies,
                    });
                }
            }
        }

        fn on_dev_tools_message(
            &self,
            browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            let Some(raw) = message else { return 0 };
            // Pre-parse size gate using strict depth-1 top-level JSON scanning (zero AST allocation):
            // - Events (`method` at depth 1) over `MAX_CDP_RESPONSE_BYTES` (16 MiB) are dropped
            //   UNPARSED before `serde_json::from_slice` so `console.log("A".repeat(100e6))` never
            //   allocates a giant JSON AST (all bookkeeping events stay well below 16 MiB).
            // - Top-level replies (`id`, no `method`, no `sessionId`) return `0` immediately so
            //   CEF delivers them ONCE to `on_dev_tools_method_result` (pre-parse cap check).
            // - Flattened OOPIF replies (`id` + `sessionId`) over `max_bytes` reject BEFORE parse.
            if raw.len() > crate::cdp_ledger::MAX_CDP_EVAL_RESPONSE_BYTES {
                let top = crate::cdp_ledger::scan_top_level_cdp(raw);
                if top.has_method {
                    if raw.len() > crate::cdp_ledger::MAX_CDP_RESPONSE_BYTES {
                        return 1;
                    }
                } else if let Some(msg_id) = top.id {
                    if !top.has_session_id {
                        return 0;
                    }
                    let over = {
                        let mut map = self.id_map.lock().unwrap();
                        match map.get(&msg_id) {
                            Some(CdpPending::Raw { max_bytes, .. }) if raw.len() > *max_bytes => {
                                map.remove(&msg_id)
                            }
                            _ => None,
                        }
                    };
                    if let Some(CdpPending::Raw { request_id, max_bytes }) = over {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id: self.tab_id,
                            request_id,
                            ok: false,
                            result: serde_json::json!({
                                "message": format!("CDP response exceeded maximum allowed size ({max_bytes} bytes).")
                            }),
                        });
                        return 1;
                    }
                    if raw.len() > crate::cdp_ledger::MAX_CDP_RESPONSE_BYTES {
                        return 1;
                    }
                } else if raw.len() > crate::cdp_ledger::MAX_CDP_RESPONSE_BYTES {
                    return 1;
                }
            }
            let Ok(mut val) = serde_json::from_slice::<Value>(raw) else { return 0 };
            // Flattened OOPIF responses carry both `"id"` and `"sessionId"`; CEF's
            // `on_dev_tools_method_result` omits them, so handle them directly here.
            if let Some(msg_id) = val.get("id").and_then(Value::as_i64) {
                if val.get("sessionId").is_some()
                    && let Some(CdpPending::Raw { request_id, max_bytes }) =
                        self.id_map.lock().unwrap().remove(&(msg_id as i32))
                {
                    if let Some(blocked_url) = self.permissions.tab_blocked_url(
                        browser.as_deref(),
                        self.isolated,
                        self.is_preview,
                        &self.snap,
                        &self.prompts,
                    ) {
                        let _ = self.out_tx.send(EngineMsg::ControlledPermissionRefused {
                            tab_id: self.tab_id,
                            url: blocked_url,
                        });
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id: self.tab_id,
                            request_id,
                            ok: false,
                            result: serde_json::json!({
                                "message": crate::permissions::INHERITED_PERMISSION_MSG
                            }),
                        });
                        return 1;
                    }
                    if raw.len() > max_bytes {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id: self.tab_id,
                            request_id,
                            ok: false,
                            result: serde_json::json!({
                                "message": format!("CDP response exceeded maximum allowed size ({max_bytes} bytes).")
                            }),
                        });
                        return 1;
                    }
                    if crate::engine::browser_has_extension_surface(
                        browser.as_deref(),
                        self.is_popup,
                        &self.snap,
                        &self.prompts,
                    ) {
                        let _ = self.out_tx.send(EngineMsg::CdpResponse {
                            tab_id: self.tab_id,
                            request_id,
                            ok: false,
                            result: serde_json::json!({
                                "message": "Agent browser tools cannot read or act on extension pages or tabs with extension frames."
                            }),
                        });
                        return 1;
                    }
                    let (ok, payload) = match val.get("error") {
                        Some(err) => (false, err.clone()),
                        None => (true, val.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = self.out_tx.send(EngineMsg::CdpResponse {
                        tab_id: self.tab_id,
                        request_id,
                        ok,
                        result: payload,
                    });
                    return 1;
                }
                return 0;
            }
            let Some(method) = val.get("method").and_then(Value::as_str).map(str::to_owned) else {
                return 0;
            };
            let session_id = val.get("sessionId").and_then(Value::as_str).map(str::to_owned);
            let params = val
                .as_object_mut()
                .and_then(|o| o.remove("params"))
                .unwrap_or(Value::Null);
            // Track true CDP `frameId` (`devtools_frame_token`) lifecycle for `#274`'s whole-tab
            // extension surface guard (`review-rd274.md` Round-4 ruling).
            match method.as_str() {
                "Page.frameNavigated" => {
                    if let Some(frame) = params.get("frame") {
                        let is_main = session_id.is_none()
                            && frame.get("parentId").and_then(Value::as_str).is_none();
                        if is_main {
                            self.prompts.clear_cdp_ext_frames();
                        }
                        let fid = frame.get("id").and_then(Value::as_str).unwrap_or("");
                        let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
                        let origin = frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("");
                        let is_ext = crate::extensions::manifest::is_extension_scheme(url)
                            || crate::extensions::manifest::is_extension_scheme(origin);
                        self.prompts.set_cdp_ext_frame(fid, is_ext);
                    }
                }
                "Page.navigatedWithinDocument" => {
                    let fid = params.get("frameId").and_then(Value::as_str).unwrap_or("");
                    let url = params.get("url").and_then(Value::as_str).unwrap_or("");
                    let is_ext = crate::extensions::manifest::is_extension_scheme(url);
                    self.prompts.set_cdp_ext_frame(fid, is_ext);
                }
                "Page.frameDetached" => {
                    if let Some(fid) = params.get("frameId").and_then(Value::as_str) {
                        self.prompts.set_cdp_ext_frame(fid, false);
                    }
                }
                "Overlay.inspectNodeRequested" => {
                    if let Some(backend_id) = params["backendNodeId"].as_i64() {
                        self.internal_picked.lock().unwrap().push(backend_id);
                    }
                }
                _ => {}
            }
            // Update engine-authoritative CDP ledger and immediately detach any extension OOPIF.
            let detach_sid = self
                .cdp_ledger
                .lock()
                .unwrap()
                .observe_event(&method, &params, session_id.as_deref());
            if let Some(ext_sid) = &detach_sid {
                self.prompts.set_cdp_ext_frame(ext_sid, true);
                if let Some(host) = browser.as_deref().and_then(|b| b.host()) {
                    let detach = serde_json::json!({
                        "id": -1,
                        "method": "Target.detachFromTarget",
                        "params": { "sessionId": ext_sid },
                    })
                    .to_string();
                    let _ = host.send_dev_tools_message(Some(detach.as_bytes()));
                }
            }
            // Preserve `#274` Round-5 §4 verbatim: when extensions are active, drop all `Page.*`
            // events from `out_tx` after internal bookkeeping (OOPIF & top-level frame URLs stay
            // tracked via `Page.getFrameTree` responses on clean web tabs); even when extensions
            // are inactive, never forward `Page.frameRequestedNavigation` / `Page.windowOpen`, and
            // drop any event when the tab has an extension surface or detached extension OOPIF.
            if (self.extensions_active && method.starts_with("Page."))
                || matches!(method.as_str(), "Page.frameRequestedNavigation" | "Page.windowOpen")
                || detach_sid.is_some()
                || crate::engine::browser_has_extension_surface(
                    browser.as_deref(),
                    self.is_popup,
                    &self.snap,
                    &self.prompts,
                )
                || self
                    .permissions
                    .tab_blocked_url(
                        browser.as_deref(),
                        self.isolated,
                        self.is_preview,
                        &self.snap,
                        &self.prompts,
                    )
                    .is_some()
            {
                return 1;
            }
            let _ = self.out_tx.send(EngineMsg::CdpEvent {
                tab_id: self.tab_id,
                method,
                params,
                session_id,
            });
            1
        }
    }
}
