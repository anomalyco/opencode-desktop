//! Profiling and audit executors for `experimental.browser` v4 (`trace.start`/`stop`/`analyze`,
//! `cpu.start`/`stop`/`analyze`, `heap.snapshot`/`summary`/`query`/`object`/`compare`, and
//! truthful `lighthouse` `unsupported` refusal).

use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;
use opencode_kit::browser_rpc::{Command, MAX_FILE_BYTES, Outcome, WireFile, failure_code, new_file_id};
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::session::{CapturedFile, SessionAttachment};
use super::profile_analyze::{
    exec_cpu_analyze, exec_heap_compare, exec_heap_object, exec_heap_query, exec_heap_summary,
    exec_trace_analyze,
};
use super::{ExecPhase, StepOutcome, cdp_err_msg, check_page_guards};
use crate::App;

pub(super) struct ProfileState {
    pub host_id: u64,
    pub stage: ProfileStage,
}

pub(super) enum ProfileStage {
    TraceStart { cdp_req: u64 },
    TraceStop { cdp_req: u64, ended: bool },
    CpuEnable { cdp_req: u64 },
    CpuStart { cdp_req: u64 },
    CpuStop { cdp_req: u64 },
    HeapEnable { cdp_req: u64 },
    HeapTake { cdp_req: u64 },
}

impl ProfileState {
    pub fn cancel_cleanup(&mut self, att: &mut SessionAttachment, app: &mut App) {
        app.workspace_panes.browser.host.cancel_cdp(self.stage.cdp_req());
        match self.stage {
            ProfileStage::TraceStart { .. } | ProfileStage::TraceStop { .. } => {
                if att.trace_recording.as_ref().is_some_and(|r| r.host_id == self.host_id) {
                    att.trace_recording = None;
                }
                abort_trace(app, self.host_id);
            }
            ProfileStage::CpuEnable { .. } | ProfileStage::CpuStart { .. } | ProfileStage::CpuStop { .. } => {
                att.cpu_started.remove(&self.host_id);
                if app.workspace_panes.browser.host.tab_open_in_engine(self.host_id) {
                    let _ = app.workspace_panes.browser.host.send_typed_cdp(self.host_id, "Profiler.stop", json!({}), None);
                    let _ = app.workspace_panes.browser.host.send_typed_cdp(self.host_id, "Profiler.disable", json!({}), None);
                }
            }
            ProfileStage::HeapEnable { .. } | ProfileStage::HeapTake { .. } => {
                att.heap_chunks.remove(&self.host_id);
                if app.workspace_panes.browser.host.tab_open_in_engine(self.host_id) {
                    let _ = app.workspace_panes.browser.host.send_typed_cdp(self.host_id, "HeapProfiler.disable", json!({}), None);
                }
            }
        }
    }
}

pub(in super::super) fn clear_trace_owner(app: &mut App, host_id: u64) {
    let st = app.kernel.slots.get_mut(&super::super::BROWSER_RPC);
    if st.active_trace_host == Some(host_id) {
        st.active_trace_host = None;
    }
}

/// Sends `Tracing.end` (when the tab is still open in the engine) and releases the app-wide trace
/// owner lock so tab close, session disposal, or command cancel never wedges Chromium's tracing.
pub(in super::super) fn abort_trace(app: &mut App, host_id: u64) {
    clear_trace_owner(app, host_id);
    if app.workspace_panes.browser.host.tab_open_in_engine(host_id) {
        let _ = app.workspace_panes.browser.host.send_typed_cdp(host_id, "Tracing.end", json!({}), None);
    }
}

impl ProfileStage {
    pub fn cdp_req(&self) -> u64 {
        match self {
            Self::TraceStart { cdp_req }
            | Self::TraceStop { cdp_req, .. }
            | Self::CpuEnable { cdp_req }
            | Self::CpuStart { cdp_req }
            | Self::CpuStop { cdp_req }
            | Self::HeapEnable { cdp_req }
            | Self::HeapTake { cdp_req } => *cdp_req,
        }
    }
}

const MAX_TRACE_EVENTS: usize = 250_000;
const MAX_HEAP_RAW_BYTES: usize = 64 * 1024 * 1024;
const TRACE_DEFAULT_DURATION_MS: u64 = 30_000;
const CPU_MAX_DURATION_MS: u64 = 30_000;

/// Routes trace data and heap snapshot chunks from the tab's CDP events into bounded buffers.
/// Defense in depth: requires engine-stamped `rendererPid` and filters trace events strictly to
/// that single renderer OS PID (`packages/gui-plugins/src/browser/profiling.ts:221-223`).
pub(in super::super) fn ingest_cdp_event(att: &mut SessionAttachment, ev: &browser_pane::CdpEvent) {
    if ev.method == "Tracing.dataCollected"
        && let Some(rec) = att.trace_recording.as_mut()
        && rec.host_id == ev.tab_id
        && let Some(pid) = ev.params.get("rendererPid").and_then(Value::as_u64).filter(|&p| p > 0)
        && let Some(arr) = ev.params.get("value").and_then(Value::as_array)
    {
        let locked = *rec.renderer_pid.get_or_insert(pid);
        if locked != pid {
            return;
        }
        let matching = arr
            .iter()
            .filter(|e| e.get("pid").and_then(Value::as_u64) == Some(locked));
        for e in matching {
            if rec.events.len() >= MAX_TRACE_EVENTS {
                rec.data_loss = true;
                break;
            }
            rec.events.push(e.clone());
        }
    } else if ev.method == "Tracing.tracingComplete"
        && let Some(rec) = att.trace_recording.as_mut()
        && rec.host_id == ev.tab_id
    {
        if let Some(pid) = ev.params.get("rendererPid").and_then(Value::as_u64).filter(|&p| p > 0) {
            rec.renderer_pid.get_or_insert(pid);
        }
        rec.data_loss |= ev.params.get("dataLossOccurred").and_then(Value::as_bool).unwrap_or(false);
        rec.complete = true;
    } else if ev.method == "HeapProfiler.addHeapSnapshotChunk"
        && let Some((buf, overflowed)) = att.heap_chunks.get_mut(&ev.tab_id)
        && let Some(chunk) = ev.params.get("chunk").and_then(Value::as_str)
    {
        if buf.len().saturating_add(chunk.len()) > MAX_HEAP_RAW_BYTES {
            *overflowed = true;
            buf.clear();
        } else if !*overflowed {
            buf.push_str(chunk);
        }
    }
}

/// Drives `trace.start` (`durationMs`) and `cpu.start` (`CPU_MAX_DURATION_MS = 30 s`) auto-stop
/// timers (`packages/gui-plugins/src/browser/profiling-machine.ts:8-11, 200-270`). Returns the
/// earliest deadline so idle tabs incur zero timer wakeups.
pub(in super::super) fn poll_auto_stops(
    att: &mut SessionAttachment,
    app: &mut App,
    now: Instant,
) -> Option<Instant> {
    let mut wake: Option<Instant> = None;
    let mut note = |t: Instant| wake = Some(wake.map_or(t, |cur| cur.min(t)));

    // 1. Performance trace auto-stop.
    let mut finish_trace = false;
    if let Some(rec) = att.trace_recording.as_mut()
        && rec.auto_stopped.is_none()
    {
        if let Some(req) = rec.auto_stop_req {
            if !rec.auto_stop_ended
                && let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req)
            {
                match resp {
                    Ok(_) => rec.auto_stop_ended = true,
                    Err(e) => {
                        rec.auto_stop_req = None;
                        rec.auto_stopped = Some(Err(cdp_err_msg(&e)));
                        clear_trace_owner(app, rec.host_id);
                    }
                }
            }
            if rec.auto_stop_ended && rec.complete {
                finish_trace = true;
            }
        } else if let Some(stop_at) = rec.stop_at {
            if now >= stop_at {
                rec.stop_at = None;
                match app.workspace_panes.browser.host.send_typed_cdp(rec.host_id, "Tracing.end", json!({}), None) {
                    Ok(req) => rec.auto_stop_req = Some(req),
                    Err(e) => {
                        rec.auto_stopped = Some(Err(e));
                        clear_trace_owner(app, rec.host_id);
                    }
                }
            } else {
                note(stop_at);
            }
        }
    }
    if finish_trace && let Some(mut rec) = att.trace_recording.take() {
        let saved = finalize_trace(att, app, rec.host_id, rec.started_at, rec.renderer_pid, std::mem::take(&mut rec.events), std::mem::take(&mut rec.resources), rec.data_loss, now);
        clear_trace_owner(app, rec.host_id);
        rec.auto_stop_req = None;
        rec.auto_stopped = Some(saved);
        att.trace_recording = Some(rec);
    }

    // 2. CPU profile 30 s auto-stop.
    let cpu_tabs: Vec<u64> = att.cpu_started.keys().copied().collect();
    for hid in cpu_tabs {
        let mut trigger_stop = false;
        let mut completed: Option<(Instant, Vec<String>, Result<Value, Value>)> = None;
        if let Some(rec) = att.cpu_started.get_mut(&hid)
            && rec.auto_stopped.is_none()
        {
            if let Some(req) = rec.auto_stop_req {
                if let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req) {
                    rec.auto_stop_req = None;
                    completed = Some((rec.started_at, rec.resources.clone(), resp));
                }
            } else if let Some(stop_at) = rec.stop_at {
                if now >= stop_at {
                    rec.stop_at = None;
                    trigger_stop = true;
                } else {
                    note(stop_at);
                }
            }
        }
        if trigger_stop {
            let res = app.workspace_panes.browser.host.send_typed_cdp(hid, "Profiler.stop", json!({}), None);
            if let Some(rec) = att.cpu_started.get_mut(&hid) {
                match res {
                    Ok(req) => rec.auto_stop_req = Some(req),
                    Err(e) => rec.auto_stopped = Some(Err(e)),
                }
            }
        }
        if let Some((started_at, resources, resp)) = completed {
            let _ = app.workspace_panes.browser.host.send_typed_cdp(hid, "Profiler.disable", json!({}), None);
            let saved = finalize_cpu(att, app, hid, Some(started_at), resources, resp, now);
            if let Some(rec) = att.cpu_started.get_mut(&hid) {
                rec.auto_stopped = Some(saved);
            }
        }
    }
    wake
}

pub(super) fn try_start(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
) -> Option<StepOutcome> {
    match cmd.action_type()? {
        "trace.start" => Some(start_trace(att, app, cmd, now)),
        "trace.stop" => Some(stop_trace(att, app, cmd)),
        "trace.analyze" => Some(exec_trace_analyze(att, app, cmd)),
        "cpu.start" => Some(start_cpu(att, app, cmd, now)),
        "cpu.stop" => Some(stop_cpu(att, app, cmd)),
        "cpu.analyze" => Some(exec_cpu_analyze(att, app, cmd)),
        "heap.snapshot" => Some(start_heap_snapshot(att, app, cmd)),
        "heap.summary" => Some(exec_heap_summary(att, app, cmd)),
        "heap.query" => Some(exec_heap_query(att, app, cmd)),
        "heap.object" => Some(exec_heap_object(att, app, cmd)),
        "heap.compare" => Some(exec_heap_compare(att, app, cmd)),
        "lighthouse" => Some(StepOutcome::Done(Outcome::fail(
            failure_code::UNSUPPORTED,
            "browser.lighthouse is unavailable in OpenCode (Rune): Node.js Lighthouse is not bundled in the native Rust desktop app. Use browser.trace.*, browser.cpu.*, browser.heap.*, and browser.snapshot instead.",
        ))),
        _ => None,
    }
}

/// Recording requires BOTH `frame_authority` (engine extensions-active profiling refusal) AND
/// `trace_isolation` (dedicated capability bit for committed-main-frame renderer PID filtering
/// and the engine-wide single-trace lock; prevents unfiltered traces on `156.0.26-4`).
fn require_profiling_engine(app: &App, name: &str) -> Result<(), Outcome> {
    let host = &app.workspace_panes.browser.host;
    if !host.engine_frame_authority() || !host.engine_trace_isolation() {
        return Err(Outcome::fail(
            failure_code::UNSUPPORTED,
            format!("browser.{name} requires an updated browser engine."),
        ));
    }
    Ok(())
}

fn start_trace(att: &mut SessionAttachment, app: &mut App, cmd: &Command, now: Instant) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "trace.start") {
        return StepOutcome::Done(o);
    }
    if let Err(o) = require_profiling_engine(app, "trace.start") {
        return StepOutcome::Done(o);
    }
    if let Some(owner) = app.kernel.slots.get(&super::super::BROWSER_RPC).active_trace_host {
        let msg = if owner == host_id {
            "A performance trace is already active in this tab. Use browser.trace.stop({tabID}) to finish it before starting another."
        } else {
            "Another tab owns the active performance trace. Wait for its owner to finish; do not stop or replace another tab's recording."
        };
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg));
    }
    if att.trace_recording.is_some() {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "A performance trace is already recording. Stop it with browser.trace.stop first.",
        ));
    }
    let duration_ms = cmd
        .action
        .get("durationMs")
        .and_then(Value::as_u64)
        .unwrap_or(TRACE_DEFAULT_DURATION_MS)
        .clamp(1_000, 30_000);
    let resources = match att.source_urls(app, host_id) {
        Ok(urls) => urls,
        Err(e) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    match app.workspace_panes.browser.host.send_typed_cdp(
        host_id,
        "Tracing.start",
        json!({
            "transferMode": "ReportEvents",
            "traceConfig": {
                "recordMode": "recordUntilFull",
                "traceBufferSizeInKb": 8192,
                "includedCategories": [
                    "devtools.timeline",
                    "disabled-by-default-devtools.timeline",
                    "disabled-by-default-devtools.timeline.stack",
                    "v8.execute",
                    "blink.user_timing",
                    "disabled-by-default-v8.cpu_profiler"
                ],
                "excludedCategories": ["*"]
            }
        }),
        None,
    ) {
        Ok(cdp_req) => {
            app.kernel.slots.get_mut(&super::super::BROWSER_RPC).active_trace_host = Some(host_id);
            att.trace_recording = Some(super::super::session::TraceRecording {
                host_id,
                started_at: now,
                duration_ms,
                stop_at: None,
                renderer_pid: None,
                events: Vec::new(),
                resources,
                complete: false,
                data_loss: false,
                auto_stop_req: None,
                auto_stop_ended: false,
                auto_stopped: None,
            });
            StepOutcome::Pending(ExecPhase::Profile(ProfileState {
                host_id,
                stage: ProfileStage::TraceStart { cdp_req },
            }))
        }
        Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    }
}

fn stop_trace(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if att.trace_recording.as_ref().is_none_or(|r| r.host_id != host_id) {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "No performance trace is currently recording on this tab.",
        ));
    }
    if att.trace_recording.as_ref().is_some_and(|r| r.auto_stopped.is_some()) {
        let Some(rec) = att.trace_recording.take() else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Trace unavailable."));
        };
        return match rec.auto_stopped.unwrap() {
            Ok((wf, duration_ms, incomplete)) => match att.wire_tab(app, host_id) {
                Some(tab) => StepOutcome::Done(Outcome::ok(json!({ "tab": tab, "durationMs": duration_ms, "incomplete": incomplete }), vec![wf])),
                None => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable.")),
            },
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        };
    }
    if let Some(rec) = att.trace_recording.as_mut() {
        rec.stop_at = None;
        if let Some(cdp_req) = rec.auto_stop_req.take() {
            return StepOutcome::Pending(ExecPhase::Profile(ProfileState {
                host_id,
                stage: ProfileStage::TraceStop { cdp_req, ended: rec.auto_stop_ended },
            }));
        }
    }
    match app.workspace_panes.browser.host.send_typed_cdp(host_id, "Tracing.end", json!({}), None) {
        Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Profile(ProfileState {
            host_id,
            stage: ProfileStage::TraceStop { cdp_req, ended: false },
        })),
        Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    }
}

fn start_cpu(att: &mut SessionAttachment, app: &mut App, cmd: &Command, now: Instant) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "cpu.start") {
        return StepOutcome::Done(o);
    }
    if let Err(o) = require_profiling_engine(app, "cpu.start") {
        return StepOutcome::Done(o);
    }
    if att.cpu_started.contains_key(&host_id) {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "CPU profiling is already active on this tab. Call browser.cpu.stop first.",
        ));
    }
    let resources = match att.source_urls(app, host_id) {
        Ok(urls) => urls,
        Err(e) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    att.cpu_started.insert(host_id, super::super::session::CpuRecording {
        started_at: now,
        stop_at: None,
        resources,
        auto_stop_req: None,
        auto_stopped: None,
    });
    match app.workspace_panes.browser.host.send_typed_cdp(host_id, "Profiler.enable", json!({}), None) {
        Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Profile(ProfileState {
            host_id,
            stage: ProfileStage::CpuEnable { cdp_req },
        })),
        Err(e) => {
            att.cpu_started.remove(&host_id);
            StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e))
        }
    }
}

fn stop_cpu(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if !att.cpu_started.contains_key(&host_id) {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "No CPU profile is recording on this tab.",
        ));
    }
    if att.cpu_started.get(&host_id).is_some_and(|r| r.auto_stopped.is_some()) {
        let Some(rec) = att.cpu_started.remove(&host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "CPU profile unavailable."));
        };
        return match rec.auto_stopped.unwrap() {
            Ok((wf, duration_ms)) => match att.wire_tab(app, host_id) {
                Some(tab) => StepOutcome::Done(Outcome::ok(json!({ "tab": tab, "durationMs": duration_ms }), vec![wf])),
                None => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable.")),
            },
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        };
    }
    if let Some(rec) = att.cpu_started.get_mut(&host_id) {
        rec.stop_at = None;
        if let Some(cdp_req) = rec.auto_stop_req.take() {
            return StepOutcome::Pending(ExecPhase::Profile(ProfileState {
                host_id,
                stage: ProfileStage::CpuStop { cdp_req },
            }));
        }
    }
    match app.workspace_panes.browser.host.send_typed_cdp(host_id, "Profiler.stop", json!({}), None) {
        Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Profile(ProfileState {
            host_id,
            stage: ProfileStage::CpuStop { cdp_req },
        })),
        Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    }
}

fn start_heap_snapshot(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "heap.snapshot") {
        return StepOutcome::Done(o);
    }
    if let Err(o) = require_profiling_engine(app, "heap.snapshot") {
        return StepOutcome::Done(o);
    }
    if att.heap_chunks.contains_key(&host_id) {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "A heap snapshot is already being captured in this tab.",
        ));
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    att.heap_chunks.insert(host_id, (String::new(), false));
    match app.workspace_panes.browser.host.send_typed_cdp(host_id, "HeapProfiler.enable", json!({}), None) {
        Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Profile(ProfileState {
            host_id,
            stage: ProfileStage::HeapEnable { cdp_req },
        })),
        Err(e) => {
            att.heap_chunks.remove(&host_id);
            StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e))
        }
    }
}

pub(super) fn step_profile(
    att: &mut SessionAttachment,
    app: &mut App,
    st: &mut ProfileState,
    now: Instant,
) -> Option<Outcome> {
    match &mut st.stage {
        ProfileStage::TraceStart { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            match resp {
                Ok(_) => {
                    if let Some(rec) = att.trace_recording.as_mut()
                        && rec.host_id == st.host_id
                    {
                        rec.started_at = now;
                        rec.stop_at = Some(now + std::time::Duration::from_millis(rec.duration_ms));
                    }
                    let tab = att.wire_tab(app, st.host_id)?;
                    Some(Outcome::ok(json!({ "tab": tab, "recording": true }), Vec::new()))
                }
                Err(e) => {
                    att.trace_recording = None;
                    clear_trace_owner(app, st.host_id);
                    Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)))
                }
            }
        }
        ProfileStage::TraceStop { cdp_req, ended } => {
            if !*ended {
                let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
                if let Err(e) = resp {
                    att.trace_recording = None;
                    clear_trace_owner(app, st.host_id);
                    return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)));
                }
                *ended = true;
            }
            let ready = att.trace_recording.as_ref().is_some_and(|r| r.complete);
            if !ready {
                return None;
            }
            let rec = att.trace_recording.take()?;
            clear_trace_owner(app, st.host_id);
            match finalize_trace(att, app, st.host_id, rec.started_at, rec.renderer_pid, rec.events, rec.resources, rec.data_loss, now) {
                Ok((wf, duration_ms, incomplete)) => {
                    let tab = att.wire_tab(app, st.host_id)?;
                    Some(Outcome::ok(json!({ "tab": tab, "durationMs": duration_ms, "incomplete": incomplete }), vec![wf]))
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
        ProfileStage::CpuEnable { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            if let Err(e) = resp {
                att.cpu_started.remove(&st.host_id);
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)));
            }
            match app.workspace_panes.browser.host.send_typed_cdp(st.host_id, "Profiler.start", json!({}), None) {
                Ok(req) => {
                    st.stage = ProfileStage::CpuStart { cdp_req: req };
                    None
                }
                Err(e) => {
                    att.cpu_started.remove(&st.host_id);
                    Some(Outcome::fail(failure_code::OPERATION_FAILED, e))
                }
            }
        }
        ProfileStage::CpuStart { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            match resp {
                Ok(_) => {
                    if let Some(rec) = att.cpu_started.get_mut(&st.host_id) {
                        rec.started_at = now;
                        rec.stop_at = Some(now + std::time::Duration::from_millis(CPU_MAX_DURATION_MS));
                    }
                    let tab = att.wire_tab(app, st.host_id)?;
                    Some(Outcome::ok(json!({ "tab": tab, "recording": true }), Vec::new()))
                }
                Err(e) => {
                    att.cpu_started.remove(&st.host_id);
                    Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)))
                }
            }
        }
        ProfileStage::CpuStop { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let started = att.cpu_started.remove(&st.host_id);
            let _ = app.workspace_panes.browser.host.send_typed_cdp(st.host_id, "Profiler.disable", json!({}), None);
            let (t0, resources) = started.map_or((None, Vec::new()), |r| (Some(r.started_at), r.resources));
            match finalize_cpu(att, app, st.host_id, t0, resources, resp, now) {
                Ok((wf, duration_ms)) => {
                    let tab = att.wire_tab(app, st.host_id)?;
                    Some(Outcome::ok(json!({ "tab": tab, "durationMs": duration_ms }), vec![wf]))
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
        ProfileStage::HeapEnable { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            if let Err(e) = resp {
                att.heap_chunks.remove(&st.host_id);
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)));
            }
            match app.workspace_panes.browser.host.send_typed_cdp(
                st.host_id,
                "HeapProfiler.takeHeapSnapshot",
                json!({ "reportProgress": false }),
                None,
            ) {
                Ok(req) => {
                    st.stage = ProfileStage::HeapTake { cdp_req: req };
                    None
                }
                Err(e) => {
                    att.heap_chunks.remove(&st.host_id);
                    Some(Outcome::fail(failure_code::OPERATION_FAILED, e))
                }
            }
        }
        ProfileStage::HeapTake { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let (raw, overflowed) = att.heap_chunks.remove(&st.host_id).unwrap_or_default();
            let _ = app.workspace_panes.browser.host.send_typed_cdp(st.host_id, "HeapProfiler.disable", json!({}), None);
            if let Err(e) = resp {
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)));
            }
            if overflowed {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Heap snapshot exceeded the 64 MiB uncompressed size limit.",
                ));
            }
            let gz = match gzip_bytes(raw.as_bytes()) {
                Ok(b) => b,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            };
            let file_id = new_file_id();
            let wf = match WireFile::from_bytes(file_id.clone(), "heap.heapsnapshot.gz".into(), "application/gzip".into(), &gz) {
                Ok(w) => w,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            };
            let resources = match att.source_urls(app, st.host_id) {
                Ok(urls) => urls,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            };
            att.record_capture(st.host_id, CapturedFile { id: file_id, name: "heap.heapsnapshot.gz".into(), mime: "application/gzip".into(), bytes: gz, resources });
            let tab = att.wire_tab(app, st.host_id)?;
            Some(Outcome::ok(json!({ "tab": tab }), vec![wf]))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_trace(
    att: &mut SessionAttachment,
    app: &App,
    host_id: u64,
    started_at: Instant,
    renderer_pid: Option<u64>,
    events: Vec<Value>,
    mut resources: Vec<String>,
    data_loss: bool,
    now: Instant,
) -> Result<(WireFile, f64, bool), String> {
    let pid = renderer_pid
        .ok_or_else(|| "Browser engine did not report a renderer process ID for trace isolation.".to_owned())?;
    let duration_ms = now.saturating_duration_since(started_at).as_secs_f64() * 1000.0;
    let payload = serde_json::to_vec(&json!({
        "traceEvents": events,
        "metadata": { "source": "opencode", "scope": "renderer-process", "processId": pid },
    }))
    .unwrap_or_default();
    let gz = gzip_bytes(&payload)?;
    let file_id = new_file_id();
    let wf = WireFile::from_bytes(file_id.clone(), "trace.json.gz".into(), "application/gzip".into(), &gz)?;
    if let Ok(end_urls) = att.source_urls(app, host_id) {
        resources.extend(end_urls);
    }
    resources.sort();
    resources.dedup();
    att.record_capture(host_id, CapturedFile { id: file_id, name: "trace.json.gz".into(), mime: "application/gzip".into(), bytes: gz, resources });
    Ok((wf, duration_ms, data_loss))
}

fn finalize_cpu(
    att: &mut SessionAttachment,
    app: &App,
    host_id: u64,
    started_at: Option<Instant>,
    mut resources: Vec<String>,
    resp: Result<Value, Value>,
    now: Instant,
) -> Result<(WireFile, f64), String> {
    let val = resp.map_err(|e| cdp_err_msg(&e))?;
    let profile = val.get("profile").cloned().unwrap_or(val);
    let bytes = serde_json::to_vec(&profile).unwrap_or_default();
    let file_id = new_file_id();
    let wf = WireFile::from_bytes(file_id.clone(), "profile.cpuprofile".into(), "application/json".into(), &bytes)?;
    if let Ok(end_urls) = att.source_urls(app, host_id) {
        resources.extend(end_urls);
    }
    resources.sort();
    resources.dedup();
    att.record_capture(host_id, CapturedFile { id: file_id, name: "profile.cpuprofile".into(), mime: "application/json".into(), bytes, resources });
    let profile_ms = profile
        .get("startTime")
        .and_then(Value::as_f64)
        .zip(profile.get("endTime").and_then(Value::as_f64))
        .map(|(s, e)| ((e - s) / 1000.0).max(0.0))
        .filter(|d| d.is_finite());
    let duration_ms = profile_ms.unwrap_or_else(|| {
        started_at.map_or(0.0, |t0| now.saturating_duration_since(t0).as_secs_f64() * 1000.0)
    });
    Ok((wf, duration_ms))
}

fn gzip_bytes(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(raw).map_err(|e| format!("Gzip compression failed: {e}"))?;
    let out = enc.finish().map_err(|e| format!("Gzip finish failed: {e}"))?;
    if out.len() > MAX_FILE_BYTES {
        return Err("Compressed capture exceeds the 5 MiB transfer limit.".into());
    }
    Ok(out)
}
