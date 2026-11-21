//! Profiling and audit executors for `experimental.browser` v4 (`trace.start`/`stop`/`analyze`,
//! `cpu.start`/`stop`/`analyze`, `heap.snapshot`/`summary`/`query`/`object`/`compare`, and
//! truthful `lighthouse` `unsupported` refusal).

use std::io::{Read, Write};

use browser_pane::tools::profiling::{HeapSnapshot, analyze_cpu, analyze_trace};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use opencode_kit::browser_rpc::{Command, MAX_FILE_BYTES, Outcome, WireFile, failure_code, new_file_id};
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::session::{CapturedFile, SessionAttachment};
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
/// `agent_files` (current revision with renderer-PID trace isolation).
fn require_profiling_engine(app: &App, name: &str) -> Result<(), Outcome> {
    let host = &app.workspace_panes.browser.host;
    if !host.engine_frame_authority() || !host.engine_agent_files() {
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
    if att.trace_recording.is_some() {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "A performance trace is already recording. Stop it with browser.trace.stop first.",
        ));
    }
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
            att.trace_recording = Some(super::super::session::TraceRecording {
                host_id,
                started_at: now,
                renderer_pid: None,
                events: Vec::new(),
                resources,
                complete: false,
                data_loss: false,
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
    att.cpu_started.insert(host_id, (now, resources));
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
                    let tab = att.wire_tab(app, st.host_id)?;
                    Some(Outcome::ok(json!({ "tab": tab, "recording": true }), Vec::new()))
                }
                Err(e) => {
                    att.trace_recording = None;
                    Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)))
                }
            }
        }
        ProfileStage::TraceStop { cdp_req, ended } => {
            if !*ended {
                let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
                if let Err(e) = resp {
                    att.trace_recording = None;
                    return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)));
                }
                *ended = true;
            }
            let ready = att.trace_recording.as_ref().is_some_and(|r| r.complete);
            if !ready {
                return None;
            }
            let rec = att.trace_recording.take()?;
            let Some(pid) = rec.renderer_pid else {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Browser engine did not report a renderer process ID for trace isolation.",
                ));
            };
            let duration_ms = now.saturating_duration_since(rec.started_at).as_secs_f64() * 1000.0;
            let payload = serde_json::to_vec(&json!({
                "traceEvents": rec.events,
                "metadata": { "source": "opencode", "scope": "renderer-process", "processId": pid },
            }))
            .unwrap_or_default();
            let gz = match gzip_bytes(&payload) {
                Ok(b) => b,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            };
            let file_id = new_file_id();
            let wf = match WireFile::from_bytes(file_id.clone(), "trace.json.gz".into(), "application/gzip".into(), &gz) {
                Ok(w) => w,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            };
            let mut resources = rec.resources;
            if let Ok(end_urls) = att.source_urls(app, st.host_id) {
                resources.extend(end_urls);
            }
            resources.sort();
            resources.dedup();
            att.record_capture(st.host_id, CapturedFile { id: file_id, name: "trace.json.gz".into(), mime: "application/gzip".into(), bytes: gz, resources });
            let tab = att.wire_tab(app, st.host_id)?;
            Some(Outcome::ok(json!({ "tab": tab, "durationMs": duration_ms, "incomplete": rec.data_loss }), vec![wf]))
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
            let val = match resp {
                Ok(v) => v,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e))),
            };
            let profile = val.get("profile").cloned().unwrap_or(val);
            let bytes = serde_json::to_vec(&profile).unwrap_or_default();
            let file_id = new_file_id();
            let wf = match WireFile::from_bytes(file_id.clone(), "profile.cpuprofile".into(), "application/json".into(), &bytes) {
                Ok(w) => w,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            };
            let mut resources = started.as_ref().map(|(_, r)| r.clone()).unwrap_or_default();
            if let Ok(end_urls) = att.source_urls(app, st.host_id) {
                resources.extend(end_urls);
            }
            resources.sort();
            resources.dedup();
            att.record_capture(st.host_id, CapturedFile { id: file_id, name: "profile.cpuprofile".into(), mime: "application/json".into(), bytes, resources });
            let profile_ms = profile
                .get("startTime")
                .and_then(Value::as_f64)
                .zip(profile.get("endTime").and_then(Value::as_f64))
                .map(|(s, e)| ((e - s) / 1000.0).max(0.0))
                .filter(|d| d.is_finite());
            let duration_ms = profile_ms.unwrap_or_else(|| {
                started.map_or(0.0, |(t0, _)| now.saturating_duration_since(t0).as_secs_f64() * 1000.0)
            });
            let tab = att.wire_tab(app, st.host_id)?;
            Some(Outcome::ok(json!({ "tab": tab, "durationMs": duration_ms }), vec![wf]))
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

fn load_capture_json(att: &SessionAttachment, host_id: u64, file_id: &str) -> Result<Value, String> {
    let cap = att
        .captures
        .get(&host_id)
        .and_then(|list| list.iter().find(|c| c.id == file_id))
        .ok_or_else(|| "File ID not found in this tab. Call browser.files.list({tabID}).".to_owned())?;
    let bytes = if cap.mime == "application/gzip" || cap.name.ends_with(".gz") {
        let dec = GzDecoder::new(cap.bytes.as_slice());
        let mut out = Vec::new();
        dec.take(64 * 1024 * 1024)
            .read_to_end(&mut out)
            .map_err(|e| format!("Could not decompress capture file: {e}"))?;
        out
    } else {
        cap.bytes.clone()
    };
    serde_json::from_slice(&bytes).map_err(|e| format!("Capture file is not valid JSON: {e}"))
}

fn exec_trace_analyze(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let (host_id, tab, file_id, limit) = match parse_artifact_cmd(att, app, cmd, "trace.analyze") {
        Ok(v) => v,
        Err(o) => return StepOutcome::Done(o),
    };
    let val = match load_capture_json(att, host_id, &file_id).and_then(|j| analyze_trace(&j, limit)) {
        Ok(mut v) => {
            v["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
            v
        }
        Err(e) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    };
    StepOutcome::Done(Outcome::ok(val, Vec::new()))
}

fn exec_cpu_analyze(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let (host_id, tab, file_id, limit) = match parse_artifact_cmd(att, app, cmd, "cpu.analyze") {
        Ok(v) => v,
        Err(o) => return StepOutcome::Done(o),
    };
    let val = match load_capture_json(att, host_id, &file_id).and_then(|j| analyze_cpu(&j, limit)) {
        Ok(mut v) => {
            v["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
            v
        }
        Err(e) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    };
    StepOutcome::Done(Outcome::ok(val, Vec::new()))
}

fn exec_heap_summary(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let (host_id, tab, file_id, limit) = match parse_artifact_cmd(att, app, cmd, "heap.summary") {
        Ok(v) => v,
        Err(o) => return StepOutcome::Done(o),
    };
    let val = match load_capture_json(att, host_id, &file_id).and_then(|j| HeapSnapshot::parse(&j)) {
        Ok(h) => {
            let mut v = h.summary(limit);
            v["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
            v
        }
        Err(e) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    };
    StepOutcome::Done(Outcome::ok(val, Vec::new()))
}

fn exec_heap_query(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let (host_id, tab, file_id, limit) = match parse_artifact_cmd(att, app, cmd, "heap.query") {
        Ok(v) => v,
        Err(o) => return StepOutcome::Done(o),
    };
    let name = cmd.action.get("name").and_then(Value::as_str).unwrap_or("");
    let val = match load_capture_json(att, host_id, &file_id).and_then(|j| HeapSnapshot::parse(&j)) {
        Ok(h) => {
            let mut v = h.query(name, limit);
            v["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
            v
        }
        Err(e) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    };
    StepOutcome::Done(Outcome::ok(val, Vec::new()))
}

fn exec_heap_object(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let (host_id, tab, file_id, limit) = match parse_artifact_cmd(att, app, cmd, "heap.object") {
        Ok(v) => v,
        Err(o) => return StepOutcome::Done(o),
    };
    let Some(obj_id) = cmd.action.get("id").and_then(Value::as_i64) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing object id."));
    };
    let val = match load_capture_json(att, host_id, &file_id)
        .and_then(|j| HeapSnapshot::parse(&j))
        .and_then(|h| h.object(obj_id, limit))
    {
        Ok(mut v) => {
            v["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
            v
        }
        Err(e) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    };
    StepOutcome::Done(Outcome::ok(val, Vec::new()))
}

fn exec_heap_compare(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "heap.compare") {
        return StepOutcome::Done(o);
    }
    let Some(before_id) = cmd.action.get("before").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing before fileID."));
    };
    let Some(after_id) = cmd.action.get("after").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing after fileID."));
    };
    let limit = cmd.action.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
    let Some(tab) = att.wire_tab(app, host_id) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    let res = (|| -> Result<Value, String> {
        let before = HeapSnapshot::parse(&load_capture_json(att, host_id, before_id)?)?;
        let after = HeapSnapshot::parse(&load_capture_json(att, host_id, after_id)?)?;
        let mut v = HeapSnapshot::compare(&before, &after, limit);
        v["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
        Ok(v)
    })();
    match res {
        Ok(v) => StepOutcome::Done(Outcome::ok(v, Vec::new())),
        Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    }
}

fn parse_artifact_cmd(
    att: &SessionAttachment,
    app: &App,
    cmd: &Command,
    name: &str,
) -> Result<(u64, opencode_kit::browser_rpc::Tab, String, usize), Outcome> {
    let wire_id = cmd
        .tab_id()
        .ok_or_else(|| Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."))?;
    let host_id = att
        .resolve_tab(app, wire_id)
        .map_err(|msg| Outcome::fail(failure_code::OPERATION_FAILED, msg))?;
    check_page_guards(app, host_id, name)?;
    let file_id = cmd
        .action
        .get("fileID")
        .and_then(Value::as_str)
        .ok_or_else(|| Outcome::fail(failure_code::OPERATION_FAILED, "Missing fileID."))?
        .to_owned();
    let limit = cmd.action.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
    let tab = att
        .wire_tab(app, host_id)
        .ok_or_else(|| Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."))?;
    Ok((host_id, tab, file_id, limit))
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
