//! CDP session attachment, service worker / extension page shim injection, SW restart handling,
//! default-world & off-origin binding enforcement, bounded probe/timeout handling, and
//! deterministic Chromium `tabId` probing.

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::manifest::{extension_id_from_url, is_valid_ext_id};
use super::{
    AttachedSession, ExecutionContextMeta, ExpectedTarget, ExtEffect, ExtensionHost, PendingRpc,
    ProbeInFlight, SessionKind, rpc, shim_source,
};

const MAX_METRIC_SAMPLES: usize = 256;
const MAX_PENDING_RPCS: usize = 256;
const MAX_BINDING_PAYLOAD_BYTES: usize = 256 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

impl ExtensionHost {
    pub fn on_cdp_message(&mut self, msg: Value) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        if let Some(id) = msg.get("id").and_then(Value::as_i64) {
            return self.handle_cdp_reply(id, &msg);
        }
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return effects;
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let session_id = msg.get("sessionId").and_then(Value::as_str).map(str::to_owned);

        match method {
            "Target.attachedToTarget" => effects.extend(self.handle_attached(&params)),
            "Target.detachedFromTarget" => {
                if let Some(sid) = params.get("sessionId").and_then(Value::as_str) {
                    self.cleanup_detached_session(sid);
                }
            }
            "Fetch.requestPaused" => {
                if let Some(sid) = session_id
                    && let Some(req_id) = params.get("requestId").and_then(Value::as_str)
                {
                    // Any request matching our `file://*` / `blob:file://*` Fetch patterns from
                    // an extension service worker, offscreen document, or extension page fails
                    // closed immediately (`review-rd274.md` §2).
                    let _ = self.send_cdp(
                        "Fetch.failRequest",
                        json!({ "requestId": req_id, "errorReason": "BlockedByClient" }),
                        Some(&sid),
                    );
                }
            }
            "Page.frameNavigated" => {
                if let Some(sid) = session_id
                    && let Some(frame) = params.get("frame")
                    && frame.get("parentId").is_none()
                    && let Some(url) = frame.get("url").and_then(Value::as_str)
                {
                    let frame_id = frame.get("id").and_then(Value::as_str).map(str::to_owned);
                    let should_detach = match self.sessions.get_mut(&sid) {
                        Some(sess) if sess.kind != SessionKind::ServiceWorker => {
                            if extension_id_from_url(url) == Some(sess.ext_id.as_str()) {
                                sess.main_frame_id = frame_id;
                                false
                            } else {
                                // Main frame left `chrome-extension://<ext_id>/` (or an initially
                                // blank target turned out to be a web/isolated tab): detach NOW.
                                true
                            }
                        }
                        _ => false,
                    };
                    if should_detach {
                        self.detach_session(&sid);
                    }
                }
            }
            "Runtime.executionContextCreated" => {
                if let Some(sid) = session_id
                    && let Some(ctx) = params.get("context")
                    && let Some(ctx_id) = ctx.get("id").and_then(Value::as_i64)
                    && let Some(sess) = self.sessions.get_mut(&sid)
                {
                    let origin = ctx
                        .get("origin")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim_end_matches('/')
                        .to_owned();
                    let aux = ctx.get("auxData");
                    let (is_default, ctx_type, frame_id) = if sess.kind == SessionKind::ServiceWorker {
                        (true, "default".to_owned(), None)
                    } else {
                        let def = aux.and_then(|a| a.get("isDefault")).and_then(Value::as_bool).unwrap_or(false);
                        let ty = aux.and_then(|a| a.get("type")).and_then(Value::as_str).unwrap_or("").to_owned();
                        let fid = aux.and_then(|a| a.get("frameId")).and_then(Value::as_str).map(str::to_owned);
                        if def
                            && ty == "default"
                            && sess.main_frame_id.is_none()
                            && extension_id_from_url(&origin) == Some(sess.ext_id.as_str())
                        {
                            sess.main_frame_id = fid.clone();
                        }
                        (def, ty, fid)
                    };
                    self.execution_contexts.insert(
                        (sid, ctx_id),
                        ExecutionContextMeta { origin, is_default, ctx_type, frame_id },
                    );
                }
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(sid) = session_id
                    && let Some(ctx_id) = params.get("executionContextId").and_then(Value::as_i64)
                {
                    self.execution_contexts.remove(&(sid, ctx_id));
                }
            }
            "Runtime.executionContextsCleared" => {
                if let Some(sid) = session_id {
                    self.execution_contexts.retain(|(s, _), _| s != &sid);
                }
            }
            "Inspector.targetReloadedAfterCrash" => {
                if let Some(sid) = session_id
                    && let Some(sess) = self.sessions.get_mut(&sid)
                {
                    sess.attach_started = Instant::now();
                    sess.pause_started = None;
                    sess.pending_shim_eval_id = None;
                    sess.was_paused = true;
                    sess.shimmed = false;
                    self.execution_contexts.retain(|(s, _), _| s != &sid);
                    let batch = vec![
                        json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.enable", "params": {} }),
                        json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Fetch.enable", "params": { "patterns": [{ "urlPattern": "file://*" }, { "urlPattern": "FILE://*" }, { "urlPattern": "blob:file://*" }] } }),
                        json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.addBinding", "params": { "name": "__ocExtCall" } }),
                        json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Debugger.enable", "params": {} }),
                        json!({ "id": self.alloc_id(), "sessionId": sid, "method": "EventBreakpoints.setInstrumentationBreakpoint", "params": { "eventName": "scriptFirstStatement" } }),
                        json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.runIfWaitingForDebugger", "params": {} }),
                    ];
                    self.pipe.send_batch(&batch);
                }
            }
            "Debugger.paused" => {
                if let Some(sid) = session_id
                    && let Some(sess) = self.sessions.get(&sid)
                {
                    let code = shim_source(sess.kind == SessionKind::PopupPage);
                    let pause_now = Instant::now();
                    let eval_id = self.send_cdp(
                        "Runtime.evaluate",
                        json!({ "expression": code, "returnByValue": true }),
                        Some(&sid),
                    );
                    if let Some(s) = self.sessions.get_mut(&sid) {
                        s.pause_started = Some(pause_now);
                        s.pending_shim_eval_id = Some(eval_id);
                    }
                }
            }
            "Runtime.bindingCalled" => {
                if let Some(sid) = session_id
                    && params.get("name").and_then(Value::as_str) == Some("__ocExtCall")
                    && let Some(payload_str) = params.get("payload").and_then(Value::as_str)
                    && payload_str.len() <= MAX_BINDING_PAYLOAD_BYTES
                    && let Ok(payload) = serde_json::from_str::<Value>(payload_str)
                {
                    // Defense-in-depth (`review-rd274.md` §3):
                    // 1. Require tracked `executionContextId` origin == `chrome-extension://<ext_id>`.
                    // 2. For pages/popups, strictly require `isDefault == true && type == "default"`
                    //    (rejecting content-script isolated worlds) AND main frame match.
                    // 3. Drop invalid sub-frame / isolated-world calls without detaching the parent
                    //    extension page (prevents a web iframe inside an extension page from DoS-ing it).
                    let ctx_id = params.get("executionContextId").and_then(Value::as_i64);
                    let check = (|| -> Option<(bool, bool)> {
                        let sess = self.sessions.get(&sid)?;
                        let meta = ctx_id.and_then(|cid| self.execution_contexts.get(&(sid.clone(), cid)))?;
                        let origin_ok = extension_id_from_url(&meta.origin) == Some(sess.ext_id.as_str());
                        let world_ok = match sess.kind {
                            SessionKind::ServiceWorker => true,
                            SessionKind::FileGuardOnly | SessionKind::SeedProbe => false,
                            SessionKind::ExtensionPage | SessionKind::PopupPage => {
                                let frame_ok = sess.main_frame_id.is_none() || meta.frame_id == sess.main_frame_id;
                                meta.is_default && meta.ctx_type == "default" && frame_ok
                            }
                        };
                        let is_main_world_violation = sess.kind == SessionKind::ServiceWorker
                            || (meta.is_default
                                && (sess.main_frame_id.is_none() || meta.frame_id == sess.main_frame_id));
                        Some((origin_ok && world_ok, is_main_world_violation))
                    })();
                    match check {
                        Some((true, _)) => effects.extend(self.handle_binding_call(&sid, payload)),
                        Some((false, true)) => self.detach_session(&sid),
                        _ => {}
                    }
                }
            }
            "Page.loadEventFired" => {
                if let Some(sid) = session_id
                    && self.sessions.get(&sid).is_some_and(|s| s.kind == SessionKind::SeedProbe)
                    && self.seed_eval_id.is_none()
                {
                    let eval_id = self.send_cdp(
                        "Runtime.evaluate",
                        json!({
                            "expression": "new Promise(r => chrome.tabs.getCurrent(t => r(t ? t.id : -1)))",
                            "awaitPromise": true,
                            "returnByValue": true
                        }),
                        Some(&sid),
                    );
                    self.seed_eval_id = Some(eval_id);
                }
            }
            _ => {}
        }
        effects
    }

    pub(super) fn tick_timeouts(&mut self) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        if let Some(ref probe) = self.probe_in_flight
            && probe.sent_at.elapsed() >= PROBE_TIMEOUT
        {
            let stale_sid = probe.sid.clone();
            self.failed_probe_sessions.insert(stale_sid);
            self.probe_in_flight = None;
            self.trigger_tab_id_probe();
            if self.probe_in_flight.is_none() && !self.pending_rpcs.is_empty() {
                effects.extend(self.flush_pending_rpcs());
            }
        }
        effects
    }

    pub(super) fn on_frame_dropped(&mut self) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        if let Some(probe) = self.probe_in_flight.take() {
            self.failed_probe_sessions.insert(probe.sid);
            self.trigger_tab_id_probe();
            if self.probe_in_flight.is_none() && !self.pending_rpcs.is_empty() {
                effects.extend(self.flush_pending_rpcs());
            }
        }
        effects
    }

    pub(super) fn on_pipe_closed(&mut self) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        self.probe_in_flight = None;
        self.pending_rpcs.clear();
        if !self.filters_ready_emitted {
            self.filters_ready_emitted = true;
            effects.push(ExtEffect::FiltersReady);
        }
        for (_, ext_id) in std::mem::take(&mut self.pending_uninstall_cmd) {
            effects.push(ExtEffect::ExtensionUninstalled {
                ext_id,
                ok: false,
                error: Some("Extension CDP pipe closed.".into()),
            });
        }
        for (_, ext_id) in std::mem::take(&mut self.pending_uninstall_upgrade) {
            effects.push(ExtEffect::ExtensionUninstalled {
                ext_id,
                ok: false,
                error: Some("Extension CDP pipe closed.".into()),
            });
        }
        for (_, ext_id) in std::mem::take(&mut self.pending_reload_cmd) {
            self.extensions.remove(&ext_id);
            effects.push(ExtEffect::ExtensionReloaded {
                ext_id,
                ok: false,
                error: Some("Extension CDP pipe closed.".into()),
            });
        }
        effects
    }

    fn detach_session(&mut self, sid: &str) {
        self.cleanup_detached_session(sid);
        let _ = self.send_cdp("Target.detachFromTarget", json!({ "sessionId": sid }), None);
    }

    fn cleanup_detached_session(&mut self, sid: &str) {
        self.sessions.remove(sid);
        self.execution_contexts.retain(|(s, _), _| s != sid);
        self.failed_probe_sessions.remove(sid);
        if self.probe_in_flight.as_ref().is_some_and(|p| p.sid == sid) {
            self.probe_in_flight = None;
            self.trigger_tab_id_probe();
        }
        self.subscribers.retain(|_, by_ext| {
            by_ext.retain(|_, set| {
                set.remove(sid);
                !set.is_empty()
            });
            !by_ext.is_empty()
        });
    }

    fn handle_cdp_reply(&mut self, id: i64, msg: &Value) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        if let Some(bad_id) = self.pending_stub_cleanup.remove(&id) {
            if msg.get("error").is_none() {
                let _ = self.send_cdp("Extensions.uninstall", json!({ "id": bad_id }), None);
            }
            return effects;
        }
        if let Some(expected_id) = self.pending_startup_loads.remove(&id) {
            let ok = msg.get("error").is_none();
            let loaded_id = msg.pointer("/result/id").and_then(Value::as_str).unwrap_or("");
            if ok && loaded_id == expected_id {
                if let Some(ext) = self.extensions.get_mut(&expected_id) {
                    ext.is_unpacked = true;
                    ext.loaded = true;
                }
                if let Some(snap) = self.action_snapshot(&expected_id) {
                    effects.push(ExtEffect::ActionUpdated(snap));
                }
                if let Some(pos) = self.queued_clicks.iter().position(|cid| cid == &expected_id) {
                    self.queued_clicks.remove(pos);
                    effects.extend(self.click_extension(&expected_id));
                }
            } else {
                self.extensions.remove(&expected_id);
                self.actions.remove(&expected_id);
                self.queued_clicks.retain(|cid| cid != &expected_id);
                if is_valid_ext_id(loaded_id) && loaded_id != expected_id {
                    let _ = self.send_cdp("Extensions.uninstall", json!({ "id": loaded_id }), None);
                }
                self.uninstall_inert_stub(&expected_id);
            }
            self.pending_filter_ids.remove(&expected_id);
            if self.pending_filter_ids.is_empty() && !self.filters_ready_emitted {
                self.filters_ready_emitted = true;
                effects.push(ExtEffect::FiltersReady);
            }
            if self.pending_startup_loads.is_empty() {
                effects.push(ExtEffect::ExtensionsSynced(self.list_actions()));
            }
            return effects;
        }
        if let Some(ext_id) = self.pending_uninstall_upgrade.remove(&id) {
            if msg.get("error").is_none() {
                if let Some(ext) = self.extensions.get_mut(&ext_id) {
                    ext.is_unpacked = true;
                }
                let cid = self.send_cdp("Extensions.uninstall", json!({ "id": ext_id }), None);
                self.pending_uninstall_cmd.insert(cid, ext_id);
            } else {
                effects.push(ExtEffect::ExtensionUninstalled {
                    ext_id,
                    ok: false,
                    error: msg.get("error").map(Value::to_string),
                });
            }
            return effects;
        }
        if let Some(ext_id) = self.pending_uninstall_cmd.remove(&id) {
            let ok = msg.get("error").is_none();
            if ok {
                self.extensions.remove(&ext_id);
                self.actions.remove(&ext_id);
                self.context_menus.remove(&ext_id);
                self.subscribers.retain(|_, by_ext| {
                    by_ext.remove(&ext_id);
                    !by_ext.is_empty()
                });
            }
            effects.push(ExtEffect::ExtensionUninstalled {
                ext_id,
                ok,
                error: msg.get("error").map(Value::to_string),
            });
            return effects;
        }
        if let Some(expected_id) = self.pending_reload_cmd.remove(&id) {
            let ok = msg.get("error").is_none();
            let loaded_id = msg.pointer("/result/id").and_then(Value::as_str).unwrap_or("").to_owned();
            if ok && is_valid_ext_id(&loaded_id) && loaded_id == expected_id {
                if let Some(ext) = self.extensions.get_mut(&expected_id) {
                    ext.is_unpacked = true;
                    ext.loaded = true;
                }
                effects.push(ExtEffect::ExtensionReloaded { ext_id: loaded_id, ok: true, error: None });
            } else {
                self.extensions.remove(&expected_id);
                if is_valid_ext_id(&loaded_id) && loaded_id != expected_id {
                    let _ = self.send_cdp("Extensions.uninstall", json!({ "id": loaded_id }), None);
                }
                effects.push(ExtEffect::ExtensionReloaded {
                    ext_id: expected_id,
                    ok: false,
                    error: msg.get("error").map(Value::to_string).or_else(|| Some("Loaded ID mismatch".into())),
                });
            }
            return effects;
        }
        if self.seed_eval_id == Some(id) {
            self.seed_eval_id = None;
            if let Some(seed_val) = msg.pointer("/result/result/value").and_then(Value::as_i64)
                && seed_val > 0
                && seed_val < (i32::MAX - 1024) as i64
            {
                let seed = seed_val as i32;
                self.seed_session_id = Some(seed);
                self.last_known_session_id = self.last_known_session_id.max(seed);
                self.trigger_tab_id_probe();
            }
            effects.push(ExtEffect::CloseSeedProbe);
            return effects;
        }

        let matching_sid = self
            .sessions
            .iter()
            .find(|(_, s)| s.pending_shim_eval_id == Some(id))
            .map(|(sid, _)| sid.clone());
        if let Some(sid) = matching_sid {
            let val = msg.pointer("/result/result/value").and_then(Value::as_str).unwrap_or("");
            let was_paused = self.sessions.get(&sid).is_some_and(|s| s.was_paused);
            if val == "WAIT" {
                if let Some(s) = self.sessions.get_mut(&sid) {
                    s.pending_shim_eval_id = None;
                }
                if was_paused {
                    self.send_cdp("Debugger.resume", json!({}), Some(&sid));
                }
            } else if val == "DENIED" {
                self.cleanup_detached_session(&sid);
                let mut batch = Vec::new();
                if was_paused {
                    batch.push(json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Debugger.disable", "params": {} }));
                    batch.push(json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Debugger.resume", "params": {} }));
                }
                batch.push(json!({ "id": self.alloc_id(), "method": "Target.detachFromTarget", "params": { "sessionId": sid } }));
                self.pipe.send_batch(&batch);
            } else {
                let mut is_sw = false;
                let mut ext_id = String::new();
                if let Some(sess) = self.sessions.get_mut(&sid) {
                    sess.pending_shim_eval_id = None;
                    sess.shimmed = true;
                    let total_us = sess.attach_started.elapsed().as_micros() as u64;
                    let eval_us = sess.pause_started.map_or(0, |t| t.elapsed().as_micros() as u64);
                    is_sw = sess.kind == SessionKind::ServiceWorker;
                    ext_id = sess.ext_id.clone();
                    if is_sw {
                        self.metrics.sw_boots += 1;
                        push_bounded(&mut self.metrics.sw_attach_to_resume_us, total_us);
                        push_bounded(&mut self.metrics.sw_shim_eval_us, eval_us);
                    } else {
                        self.metrics.page_boots += 1;
                        push_bounded(&mut self.metrics.page_attach_to_resume_us, total_us);
                        push_bounded(&mut self.metrics.page_shim_eval_us, eval_us);
                    }
                }
                if was_paused {
                    let batch = vec![
                        json!({
                            "id": self.alloc_id(),
                            "sessionId": sid,
                            "method": "EventBreakpoints.removeInstrumentationBreakpoint",
                            "params": { "eventName": "scriptFirstStatement" }
                        }),
                        json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Debugger.disable", "params": {} }),
                        json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Debugger.resume", "params": {} }),
                    ];
                    self.pipe.send_batch(&batch);
                }
                if let Some(cid) = self.active_chromium_tab_id() {
                    self.push_to_session(&sid, &json!({ "kind": "active_tab", "tabId": cid }));
                }
                if is_sw {
                    effects.extend(self.maybe_start_seed_probe());
                    if let Some(ext) = self.extensions.get_mut(&ext_id) {
                        ext.boot_count += 1;
                        if ext.boot_count > 1 {
                            self.metrics.sw_restarts += 1;
                        }
                    }
                }
                self.trigger_tab_id_probe();
            }
        }
        effects
    }

    fn consume_expected_for_ext(&mut self, ext_id: &str, url: &str) -> Option<SessionKind> {
        let is_manifest = url.ends_with("/manifest.json") && self.seed_session_id.is_none();
        let pos = self.expected_targets.iter().position(|t| match t {
            ExpectedTarget::SeedProbe { ext_id: id } => is_manifest && id == ext_id,
            ExpectedTarget::Popup { ext_id: id } => !is_manifest && id == ext_id,
            ExpectedTarget::ExtensionPage { ext_id: id } => !is_manifest && id == ext_id,
            _ => false,
        });
        pos.and_then(|i| match self.expected_targets.remove(i)? {
            ExpectedTarget::SeedProbe { .. } => Some(SessionKind::SeedProbe),
            ExpectedTarget::Popup { .. } => Some(SessionKind::PopupPage),
            ExpectedTarget::ExtensionPage { .. } => Some(SessionKind::ExtensionPage),
            _ => None,
        })
    }

    fn handle_attached(&mut self, params: &Value) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        let Some(sid) = params.get("sessionId").and_then(Value::as_str) else {
            return effects;
        };
        let waiting = params.get("waitingForDebugger").and_then(Value::as_bool).unwrap_or(false);
        let tinfo = params.get("targetInfo").unwrap_or(&Value::Null);
        let ttype = tinfo.get("type").and_then(Value::as_str).unwrap_or("");
        let url = tinfo.get("url").and_then(Value::as_str).unwrap_or("");

        // 1. Extension service worker: match strictly by verified extension ID.
        if ttype == "service_worker"
            && let Some(ext_id) = extension_id_from_url(url)
            && self.extensions.contains_key(ext_id)
        {
            let ext_id = ext_id.to_owned();
            self.sessions.insert(
                sid.to_owned(),
                AttachedSession {
                    ext_id,
                    kind: SessionKind::ServiceWorker,
                    main_frame_id: None,
                    last_popup_size: None,
                    attach_started: Instant::now(),
                    pause_started: None,
                    pending_shim_eval_id: None,
                    was_paused: waiting,
                    shimmed: false,
                },
            );
            if waiting {
                let batch = vec![
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.enable", "params": {} }),
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Fetch.enable", "params": { "patterns": [{ "urlPattern": "file://*" }, { "urlPattern": "FILE://*" }, { "urlPattern": "blob:file://*" }] } }),
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.addBinding", "params": { "name": "__ocExtCall" } }),
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Debugger.enable", "params": {} }),
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "EventBreakpoints.setInstrumentationBreakpoint", "params": { "eventName": "scriptFirstStatement" } }),
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.runIfWaitingForDebugger", "params": {} }),
                ];
                self.pipe.send_batch(&batch);
            } else {
                let eval_id = self.alloc_id();
                if let Some(s) = self.sessions.get_mut(sid) {
                    s.pause_started = Some(Instant::now());
                    s.pending_shim_eval_id = Some(eval_id);
                }
                let code = shim_source(false);
                let batch = vec![
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.enable", "params": {} }),
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Fetch.enable", "params": { "patterns": [{ "urlPattern": "file://*" }, { "urlPattern": "FILE://*" }, { "urlPattern": "blob:file://*" }] } }),
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.addBinding", "params": { "name": "__ocExtCall" } }),
                    json!({ "id": eval_id, "sessionId": sid, "method": "Runtime.evaluate", "params": { "expression": code, "returnByValue": true } }),
                ];
                self.pipe.send_batch(&batch);
            }
            effects.extend(self.maybe_start_seed_probe());
            return effects;
        }

        // Extension dedicated/shared workers and extension iframes: attach ONLY `Fetch.enable`
        // to block `file://` fetches (never install `__ocExtCall` or `shim.js`).
        if matches!(ttype, "worker" | "shared_worker" | "iframe")
            && let Some(ext_id) = extension_id_from_url(url)
            && self.extensions.contains_key(ext_id)
        {
            self.sessions.insert(
                sid.to_owned(),
                AttachedSession {
                    ext_id: ext_id.to_owned(),
                    kind: SessionKind::FileGuardOnly,
                    main_frame_id: None,
                    last_popup_size: None,
                    attach_started: Instant::now(),
                    pause_started: None,
                    pending_shim_eval_id: None,
                    was_paused: false,
                    shimmed: false,
                },
            );
            let mut batch = vec![json!({
                "id": self.alloc_id(),
                "sessionId": sid,
                "method": "Fetch.enable",
                "params": { "patterns": [{ "urlPattern": "file://*" }, { "urlPattern": "FILE://*" }, { "urlPattern": "blob:file://*" }] }
            })];
            if waiting {
                batch.push(json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.runIfWaitingForDebugger", "params": {} }));
            }
            self.pipe.send_batch(&batch);
            return effects;
        }

        // Never attach shims to iframes, dedicated/shared workers, or non-page targets.
        let is_page_like = matches!(ttype, "page" | "other");
        let ext_target = if is_page_like
            && let Some(ext_id) = extension_id_from_url(url)
            && self.extensions.contains_key(ext_id)
        {
            // Identity match: also consume any matching queued expected target so it is never stranded.
            let kind = self.consume_expected_for_ext(ext_id, url).unwrap_or_else(|| {
                if url.ends_with("/manifest.json") && self.seed_session_id.is_none() {
                    SessionKind::SeedProbe
                } else {
                    SessionKind::ExtensionPage
                }
            });
            Some((ext_id.to_owned(), kind))
        } else if ttype == "other" && url.is_empty() {
            match self.expected_targets.pop_front() {
                Some(ExpectedTarget::Popup { ext_id }) if self.extensions.contains_key(&ext_id) => {
                    Some((ext_id, SessionKind::PopupPage))
                }
                Some(ExpectedTarget::ExtensionPage { ext_id }) if self.extensions.contains_key(&ext_id) => {
                    Some((ext_id, SessionKind::ExtensionPage))
                }
                Some(ExpectedTarget::SeedProbe { ext_id }) if self.extensions.contains_key(&ext_id) => {
                    Some((ext_id, SessionKind::SeedProbe))
                }
                _ => None,
            }
        } else {
            None
        };

        if let Some((ext_id, kind)) = ext_target {
            self.sessions.insert(
                sid.to_owned(),
                AttachedSession {
                    ext_id,
                    kind,
                    main_frame_id: None,
                    last_popup_size: None,
                    attach_started: Instant::now(),
                    pause_started: None,
                    pending_shim_eval_id: None,
                    was_paused: false,
                    shimmed: false,
                },
            );
            let mut batch = vec![
                json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.enable", "params": {} }),
                json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Page.enable", "params": {} }),
                json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Fetch.enable", "params": { "patterns": [{ "urlPattern": "file://*" }, { "urlPattern": "FILE://*" }, { "urlPattern": "blob:file://*" }] } }),
            ];
            if kind != SessionKind::SeedProbe {
                let code = shim_source(kind == SessionKind::PopupPage);
                batch.push(json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.addBinding", "params": { "name": "__ocExtCall" } }));
                batch.push(json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Page.addScriptToEvaluateOnNewDocument", "params": { "source": code } }));
                if let Some(s) = self.sessions.get_mut(sid) {
                    self.metrics.page_boots += 1;
                    push_bounded(
                        &mut self.metrics.page_attach_to_resume_us,
                        s.attach_started.elapsed().as_micros() as u64,
                    );
                }
            }
            if waiting {
                batch.push(json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.runIfWaitingForDebugger", "params": {} }));
            }
            self.pipe.send_batch(&batch);
            return effects;
        }

        // Regular web tabs, isolated tabs, iframes, site workers: resume & detach immediately.
        let mut batch = Vec::new();
        if waiting {
            batch.push(json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.runIfWaitingForDebugger", "params": {} }));
        }
        batch.push(json!({ "id": self.alloc_id(), "method": "Target.detachFromTarget", "params": { "sessionId": sid } }));
        self.pipe.send_batch(&batch);
        effects
    }

    fn handle_binding_call(&mut self, sid: &str, payload: Value) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        let Some(sess_ext_id) = self.sessions.get(sid).map(|s| s.ext_id.clone()) else {
            return effects;
        };
        let op = payload.get("op").and_then(Value::as_str).unwrap_or("");
        match op {
            "shim_ready" => {
                if let Some(sess) = self.sessions.get_mut(sid) {
                    sess.shimmed = true;
                }
                if let Some(cid) = self.active_chromium_tab_id() {
                    self.push_to_session(sid, &json!({ "kind": "active_tab", "tabId": cid }));
                }
                self.trigger_tab_id_probe();
            }
            "subscribe" => {
                let Some(name) = payload.get("name").and_then(Value::as_str) else {
                    return effects;
                };
                // Exact allow-list of supported events (prevents unbounded `subscribers` OOM).
                if !rpc::is_supported_event(name) {
                    return effects;
                }
                let allowed = self
                    .extensions
                    .get(&sess_ext_id)
                    .is_some_and(|e| rpc::is_event_allowed(&e.manifest, name));
                if !allowed {
                    return effects;
                }
                let listen = payload.get("listen").and_then(Value::as_bool).unwrap_or(true);
                if listen {
                    self.subscribers
                        .entry(name.to_owned())
                        .or_default()
                        .entry(sess_ext_id)
                        .or_default()
                        .insert(sid.to_owned());
                } else if let Some(by_ext) = self.subscribers.get_mut(name)
                    && let Some(set) = by_ext.get_mut(&sess_ext_id)
                {
                    set.remove(sid);
                    if set.is_empty() {
                        by_ext.remove(&sess_ext_id);
                    }
                    if by_ext.is_empty() {
                        self.subscribers.remove(name);
                    }
                }
            }
            "popup_size" => {
                if self.sessions.get(sid).is_some_and(|s| s.kind == SessionKind::PopupPage)
                    && let (Some(w), Some(h)) = (
                        payload.get("width").and_then(Value::as_u64),
                        payload.get("height").and_then(Value::as_u64),
                    )
                {
                    let clamped = (w.clamp(25, 800) as u32, h.clamp(25, 600) as u32);
                    let changed = self
                        .sessions
                        .get(sid)
                        .is_none_or(|s| s.last_popup_size != Some(clamped));
                    if changed && self.allow_popup_resize(&sess_ext_id) {
                        if let Some(s) = self.sessions.get_mut(sid) {
                            s.last_popup_size = Some(clamped);
                        }
                        effects.push(ExtEffect::PopupResized {
                            ext_id: sess_ext_id,
                            width: clamped.0,
                            height: clamped.1,
                        });
                    }
                }
            }
            "tab_seen" => {
                // Bounded strictly around known session IDs; never allows untrusted extensions to
                // jump `last_known_session_id` to huge values (`review-rd274.md` §4).
                if let (Some(cid_i64), Some(url)) = (
                    payload.get("id").and_then(Value::as_i64),
                    payload.get("url").and_then(Value::as_str),
                ) && cid_i64 > 0
                    && cid_i64 < (i32::MAX - 1024) as i64
                {
                    self.reconcile_chromium_tab_id(cid_i64 as i32, url);
                }
            }
            "probe_tabs_reply" => {
                // Strictly require matching BOTH `probe_id` AND `sid` against `probe_in_flight`.
                let probe_id = payload.get("probeId").and_then(Value::as_i64);
                let matched = self
                    .probe_in_flight
                    .as_ref()
                    .is_some_and(|p| Some(p.probe_id) == probe_id && p.sid == sid);
                if !matched {
                    return effects;
                }
                let in_flight = self.probe_in_flight.take().unwrap();
                if let Some(found) = payload.get("found").and_then(Value::as_array) {
                    self.apply_probed_tabs(found, &in_flight.candidates);
                }
                effects.extend(self.flush_pending_rpcs());
            }
            "call" => {
                self.metrics.rpc_calls += 1;
                let req_id = payload.get("reqId").and_then(Value::as_i64).unwrap_or(0);
                let name = payload.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
                let args = payload.get("args").and_then(Value::as_array).cloned().unwrap_or_default();
                let needs_tab_ids = matches!(
                    name.as_str(),
                    "tabs.query" | "tabs.get" | "windows.get" | "windows.getCurrent" | "windows.getLastFocused" | "windows.getAll"
                );
                if needs_tab_ids
                    && self.tabs.iter().any(|t| t.chromium_tab_id.is_none())
                    && self.seed_session_id.is_some()
                    && self.pending_rpcs.len() < MAX_PENDING_RPCS
                {
                    self.pending_rpcs.push(PendingRpc {
                        session_id: sid.to_owned(),
                        ext_id: sess_ext_id,
                        req_id,
                        name,
                        args,
                    });
                    self.trigger_tab_id_probe();
                    return effects;
                }
                effects.extend(self.dispatch_rpc(sid, &sess_ext_id, req_id, &name, &args));
            }
            _ => {}
        }
        effects
    }

    pub(super) fn maybe_start_seed_probe(&mut self) -> Vec<ExtEffect> {
        if self.seed_probe_started || self.seed_session_id.is_some() {
            return Vec::new();
        }
        let Some(first_ext_id) = self
            .extensions
            .iter()
            .find(|(_, e)| e.loaded)
            .map(|(id, _)| id.clone())
        else {
            return Vec::new();
        };
        self.seed_probe_started = true;
        self.expected_targets.push_back(ExpectedTarget::SeedProbe { ext_id: first_ext_id.clone() });
        vec![ExtEffect::OpenSeedProbe {
            url: format!("chrome-extension://{first_ext_id}/manifest.json"),
        }]
    }

    pub(super) fn trigger_tab_id_probe(&mut self) {
        if self.probe_in_flight.is_some() {
            return;
        }
        let Some(seed) = self.seed_session_id else {
            return;
        };
        if !self.tabs.iter().any(|t| t.chromium_tab_id.is_none()) {
            return;
        }
        let Some(sid) = self
            .sessions
            .iter()
            .find(|(id, s)| {
                s.shimmed
                    && !matches!(s.kind, SessionKind::SeedProbe | SessionKind::FileGuardOnly)
                    && !self.failed_probe_sessions.contains(*id)
            })
            .map(|(sid, _)| sid.clone())
        else {
            return;
        };
        // Bounded candidate window (<= 49 IDs, saturating arithmetic — zero overflow / OOM).
        let base = self.last_known_session_id.max(seed);
        let lo = base.saturating_sub(16).max(1);
        let hi = base.saturating_add(32);
        let candidates: Vec<i32> = (lo..=hi).collect();
        let probe_id = self.next_probe_id;
        self.next_probe_id += 1;
        self.probe_in_flight = Some(ProbeInFlight {
            probe_id,
            sid: sid.clone(),
            sent_at: Instant::now(),
            candidates: candidates.clone(),
        });
        self.push_to_session(
            &sid,
            &json!({ "kind": "probe_tabs", "probeId": probe_id, "candidates": candidates }),
        );
    }

    fn apply_probed_tabs(&mut self, found: &[Value], allowed_candidates: &[i32]) {
        let already_used: HashSet<i32> = self.tabs.iter().filter_map(|t| t.chromium_tab_id).collect();
        let mut available: Vec<(i32, String)> = found
            .iter()
            .take(64)
            .filter_map(|v| {
                let id_i64 = v.get("id")?.as_i64()?;
                let id = i32::try_from(id_i64).ok()?;
                if !allowed_candidates.contains(&id) || already_used.contains(&id) {
                    return None;
                }
                let url = v.get("url").and_then(Value::as_str).unwrap_or("").to_owned();
                if url.is_empty() || url.starts_with("chrome-extension://") {
                    return None;
                }
                Some((id, url))
            })
            .collect();
        for tab in &mut self.tabs {
            if tab.chromium_tab_id.is_some() {
                continue;
            }
            if let Some(pos) = available.iter().position(|(_, u)| !u.is_empty() && u == &tab.url) {
                let (cid, _) = available.remove(pos);
                tab.chromium_tab_id = Some(cid);
                self.last_known_session_id = self.last_known_session_id.max(cid);
            }
        }
        for tab in &mut self.tabs {
            if tab.chromium_tab_id.is_some() {
                continue;
            }
            if !available.is_empty() {
                let (cid, _) = available.remove(0);
                tab.chromium_tab_id = Some(cid);
                self.last_known_session_id = self.last_known_session_id.max(cid);
            }
        }
        if let Some(cid) = self.active_chromium_tab_id() {
            self.broadcast_active_tab(cid);
        }
    }

    fn reconcile_chromium_tab_id(&mut self, cid: i32, url: &str) {
        let Some(seed) = self.seed_session_id else {
            return;
        };
        let min_ok = seed.saturating_sub(16).max(1);
        let max_ok = self.last_known_session_id.max(seed).saturating_add(64);
        if cid < min_ok || cid > max_ok {
            return;
        }
        if self.tabs.iter().any(|t| t.chromium_tab_id == Some(cid)) {
            return;
        }
        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|t| t.chromium_tab_id.is_none() && (t.url == url || t.url == "about:blank"))
        {
            tab.chromium_tab_id = Some(cid);
            self.last_known_session_id = self.last_known_session_id.max(cid);
        }
    }

    fn flush_pending_rpcs(&mut self) -> Vec<ExtEffect> {
        let queued = std::mem::take(&mut self.pending_rpcs);
        let mut effects = Vec::new();
        for rpc in queued {
            effects.extend(self.dispatch_rpc(&rpc.session_id, &rpc.ext_id, rpc.req_id, &rpc.name, &rpc.args));
        }
        effects
    }
}

fn push_bounded(vec: &mut Vec<u64>, val: u64) {
    if vec.len() < MAX_METRIC_SAMPLES {
        vec.push(val);
    }
}
