//! Serialized per-tab `Input.*` guard for `HostMsg::TypedCdpCall`, matching official's
//! `dispatchInput` (`packages/gui-plugins/src/browser/chromium.ts:2492-2552`, `b9a60d9d`) plus
//! `cssVisualViewport.{pageX,pageY}` scroll-offset conversion for `DOM.getNodeForLocation`.
//!
//! CDP can't condition a native input event on a hit test. Check the compositor's actual hit
//! after scrolling, and check again after resolving the node. Do not dispatch into a frame
//! owner whose inner hit is unknown (`chromium.ts:2492-2493`).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use cef::{Browser, BrowserHost, CefString, ImplBrowser, ImplBrowserHost, ImplFrame};
use serde_json::{Value, json};

use crate::cdp_ledger::CdpPending;
use crate::engine::{Surface, classify_surface};
use crate::frame_authority::{Access, EXTENSION_FRAME, EngineCdpLedger, UNAVAILABLE_FRAME, UNPREPARED, input_document};
use crate::output::Output;
use crate::prompts::TabPrompts;
use crate::wire::{EngineMsg, TabSnapshot};

const MAX_QUEUED: usize = 64;
const MAX_FRAME_DEPTH: usize = 64;
const CHANGED: &str = "Input target changed or could not be verified. Take a fresh snapshot and use the page outside extension UI.";
const COVERED: &str = "The user is answering a prompt over this page. Wait for them, or use another tab; no input was sent.";
const WHOLE_TAB: &str = "Agent browser tools cannot read or act on extension pages or tabs with extension frames.";

pub(crate) struct InputCall {
    pub request_id: u64,
    pub method: String,
    pub params: Value,
}

enum Stage {
    Metrics { call: InputCall, start_rev: u64, x: f64, y: f64 },
    Hit1 { call: InputCall, start_rev: u64, doc_x: i64, doc_y: i64 },
    Describe { call: InputCall, start_rev: u64, doc_x: i64, doc_y: i64, backend_id: i64, frame_id: String },
    Hit2 { call: InputCall, start_rev: u64, backend_id: i64, frame_id: String },
    Dispatched { request_id: u64 },
}

impl Stage {
    fn request_id(&self) -> u64 {
        match self {
            Self::Metrics { call, .. }
            | Self::Hit1 { call, .. }
            | Self::Describe { call, .. }
            | Self::Hit2 { call, .. } => call.request_id,
            Self::Dispatched { request_id } => *request_id,
        }
    }
}

struct QueuedSend {
    msg_id: i32,
    raw: String,
    stage: Stage,
}

#[derive(Default)]
pub(crate) struct InputGuard {
    active: Option<(i32, Stage)>,
    pending_send: Option<QueuedSend>,
    queue: VecDeque<InputCall>,
}

impl InputGuard {
    pub(crate) fn enqueue(&mut self, call: InputCall) -> Result<(), &'static str> {
        if self.queue.len() >= MAX_QUEUED {
            return Err("Too many queued input commands on this tab.");
        }
        self.queue.push_back(call);
        Ok(())
    }

    pub(crate) fn active_request_id(&self, msg_id: i32) -> Option<u64> {
        self.active.as_ref().filter(|(id, _)| *id == msg_id).map(|(_, st)| st.request_id())
    }

    /// Clears the in-flight probe/dispatch when Guard 1b refuses the tab mid-flight.
    pub(crate) fn abort_active(&mut self, msg_id: i32) -> bool {
        if self.active.as_ref().is_some_and(|(id, _)| *id == msg_id) {
            self.active = None;
            return !self.queue.is_empty();
        }
        false
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
        snap: &Mutex<TabSnapshot>,
        prompts: &TabPrompts,
        ledger: &Mutex<EngineCdpLedger>,
        next_cdp_id: &AtomicI32,
        out_tx: &Output,
    ) -> bool {
        let Some((active_id, stage)) = self.active.take() else { return false };
        if active_id != msg_id {
            self.active = Some((active_id, stage));
            return false;
        }
        let fail = |request_id: u64, msg: &str| {
            let _ = out_tx.send(EngineMsg::CdpResponse { tab_id, request_id, ok: false, result: json!({ "message": msg }) });
        };
        match stage {
            Stage::Dispatched { request_id } => {
                let _ = out_tx.send(EngineMsg::CdpResponse { tab_id, request_id, ok, result: val });
                !self.queue.is_empty()
            }
            Stage::Metrics { call, start_rev, x, y } => {
                if !ok {
                    fail(call.request_id, CHANGED);
                    return !self.queue.is_empty();
                }
                let vp = val.get("cssVisualViewport").or_else(|| val.get("cssLayoutViewport"));
                let page_x = vp.and_then(|v| v.get("pageX")).and_then(Value::as_f64).unwrap_or(0.0);
                let page_y = vp.and_then(|v| v.get("pageY")).and_then(Value::as_f64).unwrap_or(0.0);
                let (doc_x, doc_y) = ((x + page_x).round() as i64, (y + page_y).round() as i64);
                let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let raw = json!({
                    "id": id,
                    "method": "DOM.getNodeForLocation",
                    "params": { "x": doc_x, "y": doc_y, "ignorePointerEventsNone": false },
                })
                .to_string();
                self.pending_send = Some(QueuedSend { msg_id: id, raw, stage: Stage::Hit1 { call, start_rev, doc_x, doc_y } });
                true
            }
            Stage::Hit1 { call, start_rev, doc_x, doc_y } => {
                let (Some(backend_id), Some(frame_id)) = (
                    ok.then(|| val.get("backendNodeId").and_then(Value::as_i64)).flatten(),
                    ok.then(|| val.get("frameId").and_then(Value::as_str).map(str::to_owned)).flatten(),
                ) else {
                    fail(call.request_id, CHANGED);
                    return !self.queue.is_empty();
                };
                let (access, session) = {
                    let l = ledger.lock().unwrap();
                    (l.access(&frame_id), l.session_for_frame(&frame_id))
                };
                match access {
                    Access::Frame => {}
                    Access::Extension => {
                        fail(call.request_id, EXTENSION_FRAME);
                        return !self.queue.is_empty();
                    }
                    Access::Unavailable => {
                        fail(call.request_id, UNAVAILABLE_FRAME);
                        return !self.queue.is_empty();
                    }
                }
                let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let mut req = json!({ "id": id, "method": "DOM.describeNode", "params": { "backendNodeId": backend_id } });
                if let Some(sid) = session {
                    req["sessionId"] = Value::String(sid);
                }
                self.pending_send = Some(QueuedSend {
                    msg_id: id,
                    raw: req.to_string(),
                    stage: Stage::Describe { call, start_rev, doc_x, doc_y, backend_id, frame_id },
                });
                true
            }
            Stage::Describe { call, start_rev, doc_x, doc_y, backend_id, frame_id } => {
                let name = val.get("node").and_then(|n| n.get("nodeName")).and_then(Value::as_str).unwrap_or("");
                if !ok || is_frame_owner(name) {
                    fail(call.request_id, CHANGED);
                    return !self.queue.is_empty();
                }
                let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let raw = json!({
                    "id": id,
                    "method": "DOM.getNodeForLocation",
                    "params": { "x": doc_x, "y": doc_y, "ignorePointerEventsNone": false },
                })
                .to_string();
                self.pending_send = Some(QueuedSend { msg_id: id, raw, stage: Stage::Hit2 { call, start_rev, backend_id, frame_id } });
                true
            }
            Stage::Hit2 { mut call, start_rev, backend_id, frame_id } => {
                let hit2_ok = ok
                    && val.get("backendNodeId").and_then(Value::as_i64) == Some(backend_id)
                    && val.get("frameId").and_then(Value::as_str) == Some(frame_id.as_str());
                let ledger_ok = {
                    let l = ledger.lock().unwrap();
                    !l.is_unprepared() && l.revision == start_rev && l.access(&frame_id) == Access::Frame
                };
                let surface_ok = classify_surface(browser, is_popup, snap, prompts, ledger) != Surface::WholeTab;
                let main_ok = browser_main_input_ok(browser);
                if prompts.is_covered() {
                    fail(call.request_id, COVERED);
                    return !self.queue.is_empty();
                }
                if !hit2_ok || !ledger_ok || !surface_ok || !main_ok {
                    fail(call.request_id, CHANGED);
                    return !self.queue.is_empty();
                }
                round_coords(&mut call.params);
                let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
                let raw = json!({ "id": id, "method": call.method, "params": call.params }).to_string();
                self.pending_send = Some(QueuedSend { msg_id: id, raw, stage: Stage::Dispatched { request_id: call.request_id } });
                true
            }
        }
    }
}

/// Flushes a queued probe/dispatch or starts the next queued `Input.*` call outside any CEF
/// DevTools observer callback.
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
    cdp_id_map: &Mutex<HashMap<i32, CdpPending>>,
    next_cdp_id: &AtomicI32,
    out_tx: &Output,
) {
    loop {
        let next = {
            let mut g = guard.lock().unwrap();
            if let Some(q) = g.pending_send.take() {
                let id = q.msg_id;
                let req_id = q.stage.request_id();
                g.active = Some((id, q.stage));
                Some((id, req_id, q.raw))
            } else if g.active.is_some() {
                return;
            } else {
                let Some(mut call) = g.queue.pop_front() else { return };
                match prepare_call(&mut call, browser, is_popup, snap, prompts, ledger, next_cdp_id) {
                    Ok(q) => {
                        let id = q.msg_id;
                        let req_id = q.stage.request_id();
                        g.active = Some((id, q.stage));
                        Some((id, req_id, q.raw))
                    }
                    Err(msg) => {
                        drop(g);
                        let _ = out_tx.send(EngineMsg::CdpResponse {
                            tab_id,
                            request_id: call.request_id,
                            ok: false,
                            result: json!({ "message": msg }),
                        });
                        continue;
                    }
                }
            }
        };
        if let Some((msg_id, request_id, raw)) = next {
            cdp_id_map.lock().unwrap().insert(msg_id, CdpPending::InputGuard);
            if host.send_dev_tools_message(Some(raw.as_bytes())) != 1 {
                cdp_id_map.lock().unwrap().remove(&msg_id);
                guard.lock().unwrap().abort_active(msg_id);
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
    call: &mut InputCall,
    browser: &Browser,
    is_popup: bool,
    snap: &Mutex<TabSnapshot>,
    prompts: &TabPrompts,
    ledger: &Mutex<EngineCdpLedger>,
    next_cdp_id: &AtomicI32,
) -> Result<QueuedSend, &'static str> {
    if prompts.is_covered() {
        return Err(COVERED);
    }
    if classify_surface(Some(browser), is_popup, snap, prompts, ledger) == Surface::WholeTab {
        return Err(WHOLE_TAB);
    }
    let (unprepared, start_rev) = {
        let l = ledger.lock().unwrap();
        (l.is_unprepared(), l.revision)
    };
    if unprepared {
        return Err(UNPREPARED);
    }
    if !browser_main_input_ok(browser_opt(browser)) {
        return Err(CHANGED);
    }
    if matches!(call.method.as_str(), "Input.dispatchMouseEvent" | "Input.dispatchDragEvent") {
        let (Some(x), Some(y)) = (
            call.params.get("x").and_then(Value::as_f64).filter(|v| v.is_finite()),
            call.params.get("y").and_then(Value::as_f64).filter(|v| v.is_finite()),
        ) else {
            return Err(CHANGED);
        };
        // Always run scroll-offset `DOM.getNodeForLocation` -> `DOM.describeNode` (refusing
        // iframe/frame/object/embed owners) -> second hit-test + `revision` stability, matching
        // official `chromium.ts:2492-2552` even when zero extensions are loaded (`review-rd294.md` #4).
        let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
        let raw = json!({ "id": id, "method": "Page.getLayoutMetrics", "params": {} }).to_string();
        let owned = InputCall { request_id: call.request_id, method: std::mem::take(&mut call.method), params: call.params.take() };
        return Ok(QueuedSend { msg_id: id, raw, stage: Stage::Metrics { call: owned, start_rev, x, y } });
    }
    if matches!(call.method.as_str(), "Input.dispatchKeyEvent" | "Input.insertText") {
        check_focused_frame(browser)?;
    }
    let id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
    let raw = json!({ "id": id, "method": call.method, "params": call.params }).to_string();
    Ok(QueuedSend { msg_id: id, raw, stage: Stage::Dispatched { request_id: call.request_id } })
}

fn browser_opt(browser: &Browser) -> Option<&Browser> {
    Some(browser)
}

fn browser_main_input_ok(browser: Option<&Browser>) -> bool {
    let Some(frame) = browser.and_then(|b| b.main_frame()) else { return false };
    let url = CefString::from(&frame.url()).to_string();
    input_document(&url, "")
}

/// Walks `browser.focused_frame()` through `frame.parent()` to `browser.main_frame()`, checking
/// every ancestor's `url()` with `input_document` (never comparing CEF ids against CDP tokens,
/// matching `chromium.ts:2538-2550`).
fn check_focused_frame(browser: &Browser) -> Result<(), &'static str> {
    let Some(main) = browser.main_frame() else { return Err(CHANGED) };
    let main_id = CefString::from(&main.identifier()).to_string();
    let Some(mut cur) = browser.focused_frame() else { return Err(CHANGED) };
    for _ in 0..MAX_FRAME_DEPTH {
        if cur.is_valid() != 1 {
            return Err(CHANGED);
        }
        let url = CefString::from(&cur.url()).to_string();
        if !input_document(&url, "") {
            return Err(CHANGED);
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return (CefString::from(&cur.identifier()).to_string() == main_id).then_some(()).ok_or(CHANGED),
        }
    }
    Err(CHANGED)
}

fn is_frame_owner(name: &str) -> bool {
    name.eq_ignore_ascii_case("iframe")
        || name.eq_ignore_ascii_case("frame")
        || name.eq_ignore_ascii_case("object")
        || name.eq_ignore_ascii_case("embed")
}

fn round_coords(params: &mut Value) {
    if let Some(obj) = params.as_object_mut() {
        for k in ["x", "y"] {
            if let Some(v) = obj.get(k).and_then(Value::as_f64) {
                obj.insert(k.into(), json!(v.round()));
            }
        }
    }
}
