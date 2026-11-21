//! Serialized per-tab `Input.*` guard for `HostMsg::TypedCdpCall`, `HostMsg::CdpCall`, and
//! `HostMsg::DropFiles`, matching official's `dispatchInput` (`packages/gui-plugins/src/browser/chromium.ts:2494-2561`,
//! `edacc868`) with Chromium scroll-offset + device-emulation scale verification, scoped frame
//! revisions, paired `mouseReleased`, and deadlock-free crash/detach/timeout cleanup.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use cef::{Browser, BrowserHost, CefString, ImplBrowser, ImplBrowserHost, ImplFrame};
use serde_json::{Value, json};

use crate::cdp_ledger::CdpPending;
use crate::engine::{Surface, classify_surface};
use crate::frame_authority::{
    Access, EXTENSION_FRAME, EngineCdpLedger, UNAVAILABLE_FRAME, UNPREPARED, input_document,
};
use crate::output::Output;
use crate::prompts::TabPrompts;
use crate::wire::{EngineMsg, TabSnapshot};

const MAX_QUEUED: usize = 64;
const MAX_FRAME_DEPTH: usize = 64;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const CHANGED: &str = "Input target changed or could not be verified. Take a fresh snapshot and use the page outside extension UI.";
const COVERED: &str = "The user is answering a prompt over this page. Wait for them, or use another tab; no input was sent.";
const WHOLE_TAB: &str = "Agent browser tools cannot read or act on extension pages or tabs with extension frames.";
const UNSUPPORTED_INPUT: &str = "Unsupported Input DevTools method.";

pub(crate) struct InputCall {
    pub request_id: u64,
    pub method: String,
    pub params: Value,
    /// When `true` (e.g. `dragEnter`/`dragOver` inside `DropFiles`), success emits no `CdpResponse`,
    /// while failure immediately fails `request_id` and drops all queued calls sharing `group_id`.
    pub silent: bool,
    pub group_id: u64,
}

impl InputCall {
    pub(crate) fn single(request_id: u64, method: String, params: Value) -> Self {
        Self { request_id, method, params, silent: false, group_id: 0 }
    }
}

#[derive(Clone, Copy)]
struct ViewportMetrics {
    page_x: f64,
    page_y: f64,
    offset_x: f64,
    offset_y: f64,
}

impl ViewportMetrics {
    fn parse(val: &Value) -> Option<Self> {
        let vp = val.get("cssVisualViewport").or_else(|| val.get("cssLayoutViewport"))?;
        let page_x = vp.get("pageX").and_then(Value::as_f64).unwrap_or(0.0);
        let page_y = vp.get("pageY").and_then(Value::as_f64).unwrap_or(0.0);
        let offset_x = vp.get("offsetX").and_then(Value::as_f64).unwrap_or(0.0);
        let offset_y = vp.get("offsetY").and_then(Value::as_f64).unwrap_or(0.0);
        if page_x.is_finite() && page_y.is_finite() && offset_x.is_finite() && offset_y.is_finite() {
            Some(Self { page_x, page_y, offset_x, offset_y })
        } else {
            None
        }
    }

    fn matches(self, other: Self) -> bool {
        (self.page_x - other.page_x).abs() < 0.5
            && (self.page_y - other.page_y).abs() < 0.5
            && (self.offset_x - other.offset_x).abs() < 0.5
            && (self.offset_y - other.offset_y).abs() < 0.5
    }
}

enum Stage {
    Metrics1 {
        call: InputCall,
        start_excl_rev: u64,
        rx: f64,
        ry: f64,
        scale1: f64,
    },
    Hit1 {
        call: InputCall,
        start_excl_rev: u64,
        rx: f64,
        ry: f64,
        scale1: f64,
        vp1: ViewportMetrics,
        doc_x: i64,
        doc_y: i64,
    },
    Describe {
        call: InputCall,
        start_excl_rev: u64,
        start_frame_rev: u64,
        rx: f64,
        ry: f64,
        scale1: f64,
        vp1: ViewportMetrics,
        doc_x: i64,
        doc_y: i64,
        backend_id: i64,
        frame_id: String,
        session: Option<String>,
    },
    /// Batched second check (1 round trip): sends `Page.getLayoutMetrics` (`metrics2_id`) and
    /// `DOM.getNodeForLocation` (`hit2_id`) back-to-back and verifies scroll, emulation scale,
    /// node identity, and scoped revisions before dispatching (READER-12 #444 Blockers #1 & #2).
    Verify2 {
        call: InputCall,
        start_excl_rev: u64,
        start_frame_rev: u64,
        rx: f64,
        ry: f64,
        scale1: f64,
        vp1: ViewportMetrics,
        backend_id: i64,
        frame_id: String,
        metrics2_id: i32,
        hit2_id: i32,
        metrics2: Option<Result<ViewportMetrics, ()>>,
        hit2: Option<Result<(i64, String), ()>>,
    },
    Dispatched {
        request_id: u64,
        silent: bool,
        group_id: u64,
    },
}

impl Stage {
    fn request_id(&self) -> u64 {
        match self {
            Self::Metrics1 { call, .. }
            | Self::Hit1 { call, .. }
            | Self::Describe { call, .. }
            | Self::Verify2 { call, .. } => call.request_id,
            Self::Dispatched { request_id, .. } => *request_id,
        }
    }

    fn group_id(&self) -> u64 {
        match self {
            Self::Metrics1 { call, .. }
            | Self::Hit1 { call, .. }
            | Self::Describe { call, .. }
            | Self::Verify2 { call, .. } => call.group_id,
            Self::Dispatched { group_id, .. } => *group_id,
        }
    }

    fn is_probe(&self) -> bool {
        !matches!(self, Self::Dispatched { .. })
    }
}

struct ActiveOp {
    msg_ids: Vec<i32>,
    started_at: Instant,
    stage: Stage,
}

impl ActiveOp {
    fn matches_id(&self, msg_id: i32) -> bool {
        self.msg_ids.contains(&msg_id)
    }
}

struct QueuedBatch {
    messages: Vec<(i32, String)>,
    stage: Stage,
}

#[derive(Default)]
pub(crate) struct InputGuard {
    active: Option<ActiveOp>,
    pending_send: Option<QueuedBatch>,
    queue: VecDeque<InputCall>,
    next_group_id: u64,
    /// Button currently held down by a verified `mousePressed` (paired with its `mouseReleased` so
    /// a `mousedown` DOM mutation never leaves the button stuck down).
    active_press: Option<String>,
}

impl InputGuard {
    pub(crate) fn alloc_group_id(&mut self) -> u64 {
        self.next_group_id = self.next_group_id.wrapping_add(1).max(1);
        self.next_group_id
    }

    pub(crate) fn enqueue(&mut self, call: InputCall) -> Result<(), &'static str> {
        if self.queue.len() >= MAX_QUEUED {
            return Err("Too many queued input commands on this tab.");
        }
        self.queue.push_back(call);
        Ok(())
    }

    pub(crate) fn active_request_id(&self, msg_id: i32) -> Option<u64> {
        self.active.as_ref().filter(|op| op.matches_id(msg_id)).map(|op| op.stage.request_id())
    }

    /// Fails any active probe/dispatch matching `msg_id` (e.g. Guard 1b refusal or >16 MiB reply)
    /// and returns remaining `msg_id`s to evict from `cdp_id_map` plus whether `pump` should wake.
    pub(crate) fn abort_msg_id(
        &mut self,
        tab_id: u64,
        msg_id: i32,
        out_tx: &Output,
        reason: Option<&str>,
    ) -> (Vec<i32>, bool) {
        let Some(op) = self.active.as_ref() else {
            return (Vec::new(), false);
        };
        if !op.matches_id(msg_id) {
            return (Vec::new(), false);
        }
        let op = self.active.take().unwrap();
        let req_id = op.stage.request_id();
        let gid = op.stage.group_id();
        if let Some(msg) = reason {
            let _ = out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id: req_id,
                ok: false,
                result: json!({ "message": msg }),
            });
        }
        self.drop_group(gid);
        (op.msg_ids, !self.queue.is_empty())
    }

    /// Aborts an in-flight `Stage::Describe` targeting an OOPIF `session_id` that just detached
    /// (`Target.detachedFromTarget`, READER-12 #444 Blocker #5).
    pub(crate) fn abort_session(
        &mut self,
        tab_id: u64,
        detached_sid: &str,
        out_tx: &Output,
    ) -> (Vec<i32>, bool) {
        let hit = self.active.as_ref().is_some_and(|op| {
            matches!(&op.stage, Stage::Describe { session: Some(s), .. } if s == detached_sid)
        });
        if !hit {
            return (Vec::new(), false);
        }
        let op = self.active.take().unwrap();
        let req_id = op.stage.request_id();
        let gid = op.stage.group_id();
        let _ = out_tx.send(EngineMsg::CdpResponse {
            tab_id,
            request_id: req_id,
            ok: false,
            result: json!({ "message": CHANGED }),
        });
        self.drop_group(gid);
        (op.msg_ids, !self.queue.is_empty())
    }

    /// Fails all in-flight and queued calls (e.g. on renderer crash; READER-12 #444 Blocker #5).
    pub(crate) fn reset_all(&mut self, tab_id: u64, out_tx: &Output, reason: &str) {
        self.active_press = None;
        let mut failed_reqs = Vec::new();
        if let Some(op) = self.active.take() {
            failed_reqs.push(op.stage.request_id());
        }
        if let Some(pending) = self.pending_send.take() {
            failed_reqs.push(pending.stage.request_id());
        }
        while let Some(call) = self.queue.pop_front() {
            if !failed_reqs.contains(&call.request_id) {
                failed_reqs.push(call.request_id);
            }
        }
        for request_id in failed_reqs {
            let _ = out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: false,
                result: json!({ "message": reason }),
            });
        }
    }

    fn drop_group(&mut self, group_id: u64) {
        if group_id != 0 {
            self.queue.retain(|c| c.group_id != group_id);
        }
    }

    /// Advances the active input guard on its CDP reply inside `TabCdpObserver` (NEVER sends to
    /// CEF directly; queues `pending_send` and returns `true` when [`pump`] should run).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_reply(
        &mut self,
        tab_id: u64,
        msg_id: i32,
        ok: bool,
        val: Value,
        browser: Option<&Browser>,
        is_popup: bool,
        isolated: bool,
        is_preview: bool,
        snap: &Mutex<TabSnapshot>,
        prompts: &TabPrompts,
        permissions: &crate::permissions::PermissionTracker,
        ledger: &Mutex<EngineCdpLedger>,
        emulation_scale: &AtomicU32,
        next_cdp_id: &AtomicI32,
        out_tx: &Output,
    ) -> bool {
        let Some(mut op) = self.active.take() else { return false };
        if !op.matches_id(msg_id) {
            self.active = Some(op);
            return false;
        }
        let fail = |guard: &mut Self, request_id: u64, group_id: u64, msg: &str| {
            let _ = out_tx.send(EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: false,
                result: json!({ "message": msg }),
            });
            guard.drop_group(group_id);
        };
        match op.stage {
            Stage::Dispatched { request_id, silent, group_id } => {
                if !ok {
                    let _ = out_tx.send(EngineMsg::CdpResponse { tab_id, request_id, ok: false, result: val });
                    self.drop_group(group_id);
                } else if !silent {
                    let _ = out_tx.send(EngineMsg::CdpResponse { tab_id, request_id, ok: true, result: val });
                }
                !self.queue.is_empty()
            }
            Stage::Metrics1 { call, start_excl_rev, rx, ry, scale1 } => {
                let Some(vp1) = ok.then(|| ViewportMetrics::parse(&val)).flatten() else {
                    fail(self, call.request_id, call.group_id, CHANGED);
                    return !self.queue.is_empty();
                };
                // Convert viewport CSS pixels to root-document CSS pixels, accounting for
                // Chromium's device-emulation scale (`chromium.ts:2494-2499`, READER-12 Blocker #2).
                let doc_x = (rx / scale1 + vp1.page_x).round() as i64;
                let doc_y = (ry / scale1 + vp1.page_y).round() as i64;
                let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let raw = json!({
                    "id": id,
                    "method": "DOM.getNodeForLocation",
                    "params": { "x": doc_x, "y": doc_y, "ignorePointerEventsNone": false },
                })
                .to_string();
                self.pending_send = Some(QueuedBatch {
                    messages: vec![(id, raw)],
                    stage: Stage::Hit1 { call, start_excl_rev, rx, ry, scale1, vp1, doc_x, doc_y },
                });
                true
            }
            Stage::Hit1 { call, start_excl_rev, rx, ry, scale1, vp1, doc_x, doc_y } => {
                let (Some(backend_id), Some(frame_id)) = (
                    ok.then(|| val.get("backendNodeId").and_then(Value::as_i64)).flatten(),
                    ok.then(|| val.get("frameId").and_then(Value::as_str).map(str::to_owned)).flatten(),
                ) else {
                    fail(self, call.request_id, call.group_id, CHANGED);
                    return !self.queue.is_empty();
                };
                let (access, session, start_frame_rev, hit_url) = {
                    let l = ledger.lock().unwrap();
                    let sid = l.session_for_frame(&frame_id);
                    let url = l.target_frame_url(sid.as_deref(), Some(&frame_id), &Value::Null);
                    (l.access(&frame_id), sid, l.frame_chain_rev(&frame_id), url)
                };
                match access {
                    Access::Frame => {}
                    Access::Extension => {
                        fail(self, call.request_id, call.group_id, EXTENSION_FRAME);
                        return !self.queue.is_empty();
                    }
                    Access::Unavailable => {
                        fail(self, call.request_id, call.group_id, UNAVAILABLE_FRAME);
                        return !self.queue.is_empty();
                    }
                }
                if !isolated
                    && !is_preview
                    && prompts.agent_driven.load(Ordering::Acquire)
                    && hit_url.is_some_and(|u| permissions.has_inherited_default_grant(&u))
                {
                    fail(self, call.request_id, call.group_id, crate::permissions::INHERITED_PERMISSION_MSG);
                    return !self.queue.is_empty();
                }
                let Some(start_frame_rev) = start_frame_rev else {
                    fail(self, call.request_id, call.group_id, UNAVAILABLE_FRAME);
                    return !self.queue.is_empty();
                };
                let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let mut req = json!({ "id": id, "method": "DOM.describeNode", "params": { "backendNodeId": backend_id } });
                if let Some(ref sid) = session {
                    req["sessionId"] = Value::String(sid.clone());
                }
                self.pending_send = Some(QueuedBatch {
                    messages: vec![(id, req.to_string())],
                    stage: Stage::Describe {
                        call,
                        start_excl_rev,
                        start_frame_rev,
                        rx,
                        ry,
                        scale1,
                        vp1,
                        doc_x,
                        doc_y,
                        backend_id,
                        frame_id,
                        session,
                    },
                });
                true
            }
            Stage::Describe {
                call,
                start_excl_rev,
                start_frame_rev,
                rx,
                ry,
                scale1,
                vp1,
                doc_x,
                doc_y,
                backend_id,
                frame_id,
                ..
            } => {
                let name = val.get("node").and_then(|n| n.get("nodeName")).and_then(Value::as_str).unwrap_or("");
                if !ok || is_frame_owner(name) {
                    fail(self, call.request_id, call.group_id, CHANGED);
                    return !self.queue.is_empty();
                }
                // Batch `Page.getLayoutMetrics` + `DOM.getNodeForLocation` in a single round trip
                // so scroll shifts between `Metrics1` and `Hit2` are caught at zero extra RT cost.
                let metrics2_id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let hit2_id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let raw_metrics = json!({ "id": metrics2_id, "method": "Page.getLayoutMetrics", "params": {} }).to_string();
                let raw_hit = json!({
                    "id": hit2_id,
                    "method": "DOM.getNodeForLocation",
                    "params": { "x": doc_x, "y": doc_y, "ignorePointerEventsNone": false },
                })
                .to_string();
                self.pending_send = Some(QueuedBatch {
                    messages: vec![(metrics2_id, raw_metrics), (hit2_id, raw_hit)],
                    stage: Stage::Verify2 {
                        call,
                        start_excl_rev,
                        start_frame_rev,
                        rx,
                        ry,
                        scale1,
                        vp1,
                        backend_id,
                        frame_id,
                        metrics2_id,
                        hit2_id,
                        metrics2: None,
                        hit2: None,
                    },
                });
                true
            }
            Stage::Verify2 {
                mut call,
                start_excl_rev,
                start_frame_rev,
                rx,
                ry,
                scale1,
                vp1,
                backend_id,
                frame_id,
                metrics2_id,
                hit2_id,
                mut metrics2,
                mut hit2,
            } => {
                if msg_id == metrics2_id {
                    metrics2 = Some(ok.then(|| ViewportMetrics::parse(&val)).flatten().ok_or(()));
                } else if msg_id == hit2_id {
                    let parsed = ok
                        .then(|| {
                            Some((
                                val.get("backendNodeId")?.as_i64()?,
                                val.get("frameId")?.as_str()?.to_owned(),
                            ))
                        })
                        .flatten();
                    hit2 = Some(parsed.ok_or(()));
                }
                if metrics2.is_none() || hit2.is_none() {
                    op.msg_ids.retain(|&id| id != msg_id);
                    op.stage = Stage::Verify2 {
                        call,
                        start_excl_rev,
                        start_frame_rev,
                        rx,
                        ry,
                        scale1,
                        vp1,
                        backend_id,
                        frame_id,
                        metrics2_id,
                        hit2_id,
                        metrics2,
                        hit2,
                    };
                    self.active = Some(op);
                    return false;
                }
                let m2 = metrics2.unwrap();
                let h2 = hit2.unwrap();
                if prompts.is_covered() {
                    fail(self, call.request_id, call.group_id, COVERED);
                    return !self.queue.is_empty();
                }
                let scale2 = read_emulation_scale(emulation_scale);
                let scroll_ok = m2.is_ok_and(|vp2| vp1.matches(vp2)) && (scale2 - scale1).abs() < 1e-4;
                let hit2_ok = h2.is_ok_and(|(b2, f2)| b2 == backend_id && f2 == frame_id);
                let ledger_ok = {
                    let l = ledger.lock().unwrap();
                    !l.is_unprepared()
                        && l.exclusion_revision == start_excl_rev
                        && l.frame_chain_rev(&frame_id) == Some(start_frame_rev)
                        && l.access(&frame_id) == Access::Frame
                        && l.main_frame_input_ok()
                };
                let surface_ok = classify_surface(browser, is_popup, snap, prompts, ledger) != Surface::WholeTab;
                let main_ok = browser_main_input_ok(browser, ledger);
                if !scroll_ok || !hit2_ok || !ledger_ok || !surface_ok || !main_ok {
                    fail(self, call.request_id, call.group_id, CHANGED);
                    return !self.queue.is_empty();
                }
                set_rounded_coords(&mut call.params, rx, ry);
                prompts.suppress_context_menu.store(true, Ordering::Release);
                if call.method == "Input.dispatchMouseEvent"
                    && call.params.get("type").and_then(Value::as_str) == Some("mousePressed")
                {
                    let btn = call.params.get("button").and_then(Value::as_str).unwrap_or("left");
                    self.active_press = Some(btn.to_owned());
                }
                let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let raw = json!({ "id": id, "method": call.method, "params": call.params }).to_string();
                self.pending_send = Some(QueuedBatch {
                    messages: vec![(id, raw)],
                    stage: Stage::Dispatched {
                        request_id: call.request_id,
                        silent: call.silent,
                        group_id: call.group_id,
                    },
                });
                true
            }
        }
    }
}

/// Flushes a queued probe/dispatch batch, enforces probe timeout, or starts the next queued
/// `Input.*` call outside any CEF DevTools observer callback.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pump(
    tab_id: u64,
    guard: &Mutex<InputGuard>,
    browser: &Browser,
    host: &BrowserHost,
    is_popup: bool,
    snap: &Mutex<TabSnapshot>,
    prompts: &TabPrompts,
    ledger: &Mutex<EngineCdpLedger>,
    emulation_scale: &AtomicU32,
    cdp_id_map: &Mutex<HashMap<i32, CdpPending>>,
    next_cdp_id: &AtomicI32,
    out_tx: &Output,
) {
    loop {
        let next = {
            let mut g = guard.lock().unwrap();
            if let Some(ref op) = g.active
                && op.stage.is_probe()
                && op.started_at.elapsed() >= PROBE_TIMEOUT
            {
                let timed_out = g.active.take().unwrap();
                let req_id = timed_out.stage.request_id();
                let gid = timed_out.stage.group_id();
                g.drop_group(gid);
                drop(g);
                {
                    let mut map = cdp_id_map.lock().unwrap();
                    for mid in timed_out.msg_ids {
                        map.remove(&mid);
                    }
                }
                let _ = out_tx.send(EngineMsg::CdpResponse {
                    tab_id,
                    request_id: req_id,
                    ok: false,
                    result: json!({ "message": CHANGED }),
                });
                continue;
            }
            if let Some(batch) = g.pending_send.take() {
                let msg_ids: Vec<i32> = batch.messages.iter().map(|(id, _)| *id).collect();
                let req_id = batch.stage.request_id();
                let gid = batch.stage.group_id();
                g.active = Some(ActiveOp { msg_ids, started_at: Instant::now(), stage: batch.stage });
                Some((req_id, gid, batch.messages))
            } else if g.active.is_some() {
                return;
            } else {
                let Some(mut call) = g.queue.pop_front() else { return };
                match prepare_call(
                    &mut g.active_press,
                    &mut call,
                    browser,
                    is_popup,
                    snap,
                    prompts,
                    ledger,
                    emulation_scale,
                    next_cdp_id,
                ) {
                    Ok(batch) => {
                        let msg_ids: Vec<i32> = batch.messages.iter().map(|(id, _)| *id).collect();
                        let req_id = batch.stage.request_id();
                        let gid = batch.stage.group_id();
                        g.active = Some(ActiveOp { msg_ids, started_at: Instant::now(), stage: batch.stage });
                        Some((req_id, gid, batch.messages))
                    }
                    Err(msg) => {
                        let req_id = call.request_id;
                        let gid = call.group_id;
                        g.drop_group(gid);
                        drop(g);
                        let _ = out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id: req_id,
                            ok: false,
                            result: json!({ "message": msg }),
                        });
                        continue;
                    }
                }
            }
        };
        if let Some((request_id, group_id, messages)) = next {
            {
                let mut map = cdp_id_map.lock().unwrap();
                for (mid, _) in &messages {
                    map.insert(*mid, CdpPending::InputGuard);
                }
            }
            let mut send_failed = false;
            for (_, raw) in &messages {
                if host.send_dev_tools_message(Some(raw.as_bytes())) != 1 {
                    send_failed = true;
                    break;
                }
            }
            if send_failed {
                {
                    let mut map = cdp_id_map.lock().unwrap();
                    for (mid, _) in &messages {
                        map.remove(mid);
                    }
                }
                {
                    let mut g = guard.lock().unwrap();
                    g.active = None;
                    g.drop_group(group_id);
                }
                let _ = out_tx.send(EngineMsg::CdpResponse {
                    tab_id,
                    request_id,
                    ok: false,
                    result: json!({ "message": "send_dev_tools_message rejected" }),
                });
                continue;
            }
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_call(
    active_press: &mut Option<String>,
    call: &mut InputCall,
    browser: &Browser,
    is_popup: bool,
    snap: &Mutex<TabSnapshot>,
    prompts: &TabPrompts,
    ledger: &Mutex<EngineCdpLedger>,
    emulation_scale: &AtomicU32,
    next_cdp_id: &AtomicI32,
) -> Result<QueuedBatch, &'static str> {
    if prompts.is_covered() {
        return Err(COVERED);
    }
    if classify_surface(Some(browser), is_popup, snap, prompts, ledger) == Surface::WholeTab {
        *active_press = None;
        return Err(WHOLE_TAB);
    }
    let (unprepared, start_excl_rev) = {
        let l = ledger.lock().unwrap();
        (l.is_unprepared(), l.exclusion_revision)
    };
    if unprepared {
        return Err(UNPREPARED);
    }
    if !browser_main_input_ok(Some(browser), ledger) {
        *active_press = None;
        return Err(CHANGED);
    }
    match call.method.as_str() {
        "Input.dispatchMouseEvent" | "Input.dispatchDragEvent" => {
            let (Some(x), Some(y)) = (
                call.params.get("x").and_then(Value::as_f64).filter(|v| v.is_finite()),
                call.params.get("y").and_then(Value::as_f64).filter(|v| v.is_finite()),
            ) else {
                return Err(CHANGED);
            };
            let (rx, ry) = (x.round(), y.round());
            // Paired `mouseReleased` for an active verified `mousePressed` on the same button:
            // dispatches directly after synchronous safety checks so a `mousedown` DOM change
            // never leaves the button stuck down.
            if call.method == "Input.dispatchMouseEvent"
                && call.params.get("type").and_then(Value::as_str) == Some("mouseReleased")
                && let Some(btn) = call.params.get("button").and_then(Value::as_str)
                && active_press.as_deref() == Some(btn)
            {
                *active_press = None;
                prompts.suppress_context_menu.store(true, Ordering::Release);
                set_rounded_coords(&mut call.params, rx, ry);
                let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let raw = json!({ "id": id, "method": call.method, "params": call.params }).to_string();
                return Ok(QueuedBatch {
                    messages: vec![(id, raw)],
                    stage: Stage::Dispatched {
                        request_id: call.request_id,
                        silent: call.silent,
                        group_id: call.group_id,
                    },
                });
            }
            let scale1 = read_emulation_scale(emulation_scale);
            let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
            let raw = json!({ "id": id, "method": "Page.getLayoutMetrics", "params": {} }).to_string();
            let owned = InputCall {
                request_id: call.request_id,
                method: std::mem::take(&mut call.method),
                params: call.params.take(),
                silent: call.silent,
                group_id: call.group_id,
            };
            Ok(QueuedBatch {
                messages: vec![(id, raw)],
                stage: Stage::Metrics1 { call: owned, start_excl_rev, rx, ry, scale1 },
            })
        }
        "Input.dispatchKeyEvent" | "Input.insertText" => {
            check_focused_frame(browser, ledger, prompts)?;
            prompts.suppress_context_menu.store(true, Ordering::Release);
            let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
            let raw = json!({ "id": id, "method": call.method, "params": call.params }).to_string();
            Ok(QueuedBatch {
                messages: vec![(id, raw)],
                stage: Stage::Dispatched {
                    request_id: call.request_id,
                    silent: call.silent,
                    group_id: call.group_id,
                },
            })
        }
        "Input.setInterceptDrags" => {
            let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
            let raw = json!({ "id": id, "method": call.method, "params": call.params }).to_string();
            Ok(QueuedBatch {
                messages: vec![(id, raw)],
                stage: Stage::Dispatched {
                    request_id: call.request_id,
                    silent: call.silent,
                    group_id: call.group_id,
                },
            })
        }
        _ => Err(UNSUPPORTED_INPUT),
    }
}

fn read_emulation_scale(scale: &AtomicU32) -> f64 {
    let s = f64::from(f32::from_bits(scale.load(Ordering::Acquire)));
    if s.is_finite() && s >= 0.25 && s <= 4.0 { s } else { 1.0 }
}

fn browser_main_input_ok(browser: Option<&Browser>, ledger: &Mutex<EngineCdpLedger>) -> bool {
    let Some(frame) = browser.and_then(|b| b.main_frame()) else { return false };
    let url = CefString::from(&frame.url()).to_string();
    input_document(&url, "") && ledger.lock().unwrap().main_frame_input_ok()
}

fn is_blank_or_srcdoc(url: &str) -> bool {
    let t = url.trim();
    t.is_empty()
        || t.eq_ignore_ascii_case("about:blank")
        || t.eq_ignore_ascii_case("about:srcdoc")
        || (t.len() >= 11 && t.as_bytes()[..11].eq_ignore_ascii_case(b"javascript:"))
}

/// Walks `browser.focused_frame()` through `frame.parent()` to `browser.main_frame()`, checking
/// every ancestor's `url()` with `input_document` AND consulting `EngineCdpLedger`/`TabPrompts` so
/// `about:blank`/`about:srcdoc` frames that inherited a `chrome-extension://` origin fail closed
/// (`chromium.ts:1121-1124, 2552`; READER-12 #444 Blocker #4).
fn check_focused_frame(
    browser: &Browser,
    ledger: &Mutex<EngineCdpLedger>,
    prompts: &TabPrompts,
) -> Result<(), &'static str> {
    let Some(main) = browser.main_frame() else { return Err(CHANGED) };
    let main_id = CefString::from(&main.identifier()).to_string();
    let Some(mut cur) = browser.focused_frame() else { return Err(CHANGED) };
    let has_ext_risk = ledger.lock().unwrap().has_excluded() || prompts.has_any_pending_ext_subframe();
    for _ in 0..MAX_FRAME_DEPTH {
        if cur.is_valid() != 1 {
            return Err(CHANGED);
        }
        let url = CefString::from(&cur.url()).to_string();
        if !input_document(&url, "") {
            return Err(CHANGED);
        }
        if is_blank_or_srcdoc(&url) && has_ext_risk {
            return Err(CHANGED);
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => {
                return (CefString::from(&cur.identifier()).to_string() == main_id)
                    .then_some(())
                    .ok_or(CHANGED);
            }
        }
    }
    Err(CHANGED)
}

fn is_frame_owner(name: &str) -> bool {
    name.eq_ignore_ascii_case("iframe")
        || name.eq_ignore_ascii_case("frame")
        || name.eq_ignore_ascii_case("object")
        || name.eq_ignore_ascii_case("embed")
        || name.eq_ignore_ascii_case("fencedframe")
        || name.eq_ignore_ascii_case("portal")
}

fn set_rounded_coords(params: &mut Value, rx: f64, ry: f64) {
    if let Some(obj) = params.as_object_mut() {
        obj.insert("x".into(), json!(rx));
        obj.insert("y".into(), json!(ry));
    }
}
