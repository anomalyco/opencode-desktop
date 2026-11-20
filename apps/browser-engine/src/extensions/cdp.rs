//! CDP session attachment, service worker / extension page shim injection, SW restart handling,
//! execution-context origin verification, binding dispatch, and deterministic Chromium `tabId` probing.

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::time::Instant;

use serde_json::{Value, json};

use super::manifest::{ExtensionManifest, extension_id_from_url, is_valid_ext_id};
use super::{
    AttachedSession, ExpectedTarget, ExtEffect, ExtensionHost, PendingRpc, SessionKind, rpc,
    shim_source,
};

const MAX_METRIC_SAMPLES: usize = 256;
const MAX_PENDING_RPCS: usize = 256;

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
                    self.sessions.remove(sid);
                    self.execution_contexts.retain(|(s, _), _| s != sid);
                    if self.probe_in_flight.as_ref().is_some_and(|(_, psid)| psid == sid) {
                        self.probe_in_flight = None;
                        self.trigger_tab_id_probe();
                    }
                    for by_ext in self.subscribers.values_mut() {
                        for set in by_ext.values_mut() {
                            set.remove(sid);
                        }
                    }
                }
            }
            "Runtime.executionContextCreated" => {
                if let Some(sid) = session_id
                    && let Some(ctx) = params.get("context")
                    && let Some(ctx_id) = ctx.get("id").and_then(Value::as_i64)
                {
                    let origin = ctx
                        .get("origin")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim_end_matches('/')
                        .to_owned();
                    self.execution_contexts.insert((sid, ctx_id), origin);
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
                    && let Ok(payload) = serde_json::from_str::<Value>(payload_str)
                {
                    // Defense-in-depth: require the calling execution context's tracked origin
                    // (`chrome-extension://<id>` on pages, `chrome-extension://<id>/<script>` on
                    // service workers) to match this session's verified extension ID.
                    let ctx_id = params.get("executionContextId").and_then(Value::as_i64);
                    let expected_id = self.sessions.get(&sid).map(|s| s.ext_id.as_str());
                    let actual_id = ctx_id
                        .and_then(|cid| self.execution_contexts.get(&(sid.clone(), cid)))
                        .and_then(|orig| extension_id_from_url(orig));
                    if expected_id.is_some() && actual_id == expected_id {
                        effects.extend(self.handle_binding_call(&sid, payload));
                    } else {
                        self.sessions.remove(&sid);
                        self.execution_contexts.retain(|(s, _), _| s != &sid);
                        let _ = self.send_cdp("Target.detachFromTarget", json!({ "sessionId": sid }), None);
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

    fn handle_cdp_reply(&mut self, id: i64, msg: &Value) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
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
                // Replay any toolbar click queued while this extension finished background load.
                if let Some(pos) = self.queued_clicks.iter().position(|cid| cid == &expected_id) {
                    self.queued_clicks.remove(pos);
                    effects.extend(self.click_extension(&expected_id));
                }
            } else {
                self.extensions.remove(&expected_id);
                self.actions.remove(&expected_id);
                self.queued_clicks.retain(|cid| cid != &expected_id);
                let _ = self.send_cdp("Extensions.uninstall", json!({ "id": expected_id }), None);
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
                for by_ext in self.subscribers.values_mut() {
                    by_ext.remove(&ext_id);
                }
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
            if ok
                && is_valid_ext_id(&loaded_id)
                && loaded_id == expected_id
                && let Some(item) = self.verified_startup.get(&expected_id).cloned()
                && let Some(m) = ExtensionManifest::read(&item.dir)
            {
                self.register_extension(item.id, item.dir, item.sha256, m, true, true);
                effects.push(ExtEffect::ExtensionReloaded { ext_id: loaded_id, ok: true, error: None });
            } else {
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
                self.sessions.remove(&sid);
                self.execution_contexts.retain(|(s, _), _| s != &sid);
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

    fn handle_attached(&mut self, params: &Value) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        let Some(sid) = params.get("sessionId").and_then(Value::as_str) else {
            return effects;
        };
        let waiting = params.get("waitingForDebugger").and_then(Value::as_bool).unwrap_or(false);
        let tinfo = params.get("targetInfo").unwrap_or(&Value::Null);
        let ttype = tinfo.get("type").and_then(Value::as_str).unwrap_or("");
        let url = tinfo.get("url").and_then(Value::as_str).unwrap_or("");

        // Only attach to verified extensions loaded by us (never unknown IDs).
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
                    json!({ "id": self.alloc_id(), "sessionId": sid, "method": "Runtime.addBinding", "params": { "name": "__ocExtCall" } }),
                    json!({ "id": eval_id, "sessionId": sid, "method": "Runtime.evaluate", "params": { "expression": code, "returnByValue": true } }),
                ];
                self.pipe.send_batch(&batch);
            }
            effects.extend(self.maybe_start_seed_probe());
            return effects;
        }

        let ext_target = if let Some(ext_id) = extension_id_from_url(url)
            && self.extensions.contains_key(ext_id)
        {
            if url.ends_with("/manifest.json") && self.seed_session_id.is_none() {
                Some((ext_id.to_owned(), SessionKind::SeedProbe))
            } else {
                Some((ext_id.to_owned(), SessionKind::ExtensionPage))
            }
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
                let allowed = self
                    .extensions
                    .get(&sess_ext_id)
                    .is_some_and(|e| rpc::is_api_allowed(&e.manifest, name));
                if !allowed {
                    return effects;
                }
                let listen = payload.get("listen").and_then(Value::as_bool).unwrap_or(true);
                let set = self.subscribers.entry(name.to_owned()).or_default().entry(sess_ext_id).or_default();
                if listen {
                    set.insert(sid.to_owned());
                } else {
                    set.remove(sid);
                }
            }
            "popup_size" => {
                if self.sessions.get(sid).is_some_and(|s| s.kind == SessionKind::PopupPage)
                    && let (Some(w), Some(h)) = (
                        payload.get("width").and_then(Value::as_u64),
                        payload.get("height").and_then(Value::as_u64),
                    )
                {
                    effects.push(ExtEffect::PopupResized {
                        ext_id: sess_ext_id,
                        width: (w as u32).clamp(25, 800),
                        height: (h as u32).clamp(25, 600),
                    });
                }
            }
            "tab_seen" => {
                if let (Some(cid), Some(url)) = (
                    payload.get("id").and_then(Value::as_i64),
                    payload.get("url").and_then(Value::as_str),
                ) {
                    self.reconcile_chromium_tab_id(cid as i32, url);
                }
            }
            "probe_tabs_reply" => {
                let probe_id = payload.get("probeId").and_then(Value::as_i64);
                if self.probe_in_flight.as_ref().is_some_and(|(pid, _)| Some(*pid) == probe_id) {
                    self.probe_in_flight = None;
                }
                if let Some(found) = payload.get("found").and_then(Value::as_array) {
                    self.apply_probed_tabs(found);
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
            .find(|(_, s)| s.shimmed && s.kind != SessionKind::SeedProbe)
            .map(|(sid, _)| sid.clone())
        else {
            return;
        };
        let base = self.last_known_session_id.max(seed);
        let lo = (seed - 16).max(1);
        let candidates: Vec<i32> = (lo..=(base + 24)).collect();
        let probe_id = self.next_probe_id;
        self.next_probe_id += 1;
        self.probe_in_flight = Some((probe_id, sid.clone()));
        self.push_to_session(&sid, &json!({ "kind": "probe_tabs", "probeId": probe_id, "candidates": candidates }));
    }

    fn apply_probed_tabs(&mut self, found: &[Value]) {
        let already_used: HashSet<i32> = self.tabs.iter().filter_map(|t| t.chromium_tab_id).collect();
        let mut available: Vec<(i32, String)> = found
            .iter()
            .filter_map(|v| {
                let id = v.get("id")?.as_i64()? as i32;
                if already_used.contains(&id) {
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
