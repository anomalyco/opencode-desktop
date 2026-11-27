//! Retained trace, CPU profile, and V8 heap snapshot analyzers (`trace.analyze`, `cpu.analyze`,
//! `heap.summary`, `heap.query`, `heap.object`, `heap.compare`).

use std::io::{Read, Write};

use browser_pane::tools::profiling::{HeapSnapshot, analyze_cpu, analyze_trace};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use opencode_kit::browser_rpc::{Command, MAX_FILE_BYTES, Outcome, TargetMeta, WireFile, failure_code, new_file_id};
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::session::{CapturedFile, CpuRecording, SessionAttachment, TraceRecording};
use super::{StepOutcome, cdp_err_msg, check_page_guards};
use crate::App;

pub(super) const RESOURCES_OVERFLOW_MSG: &str = "Recording visited too many distinct URLs to verify authorization safely.";

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

pub(super) fn exec_trace_analyze(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
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

pub(super) fn exec_cpu_analyze(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
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

pub(super) fn exec_heap_summary(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
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

pub(super) fn exec_heap_query(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
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

pub(super) fn exec_heap_object(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
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

pub(super) fn exec_heap_compare(att: &SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
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

/// Computes the server-authorization target (`resources` + unique recording `key`) for `trace.stop`
/// and `cpu.stop` (`chromium.ts:2108-2109`, `profiling-machine.ts:413-418`).
pub(super) fn stop_target(
    att: &SessionAttachment,
    app: &App,
    host_id: u64,
    is_trace: bool,
) -> Result<TargetMeta, Outcome> {
    let (key, mut resources, overflow, stopped) = if is_trace {
        let Some(r) = att.trace_recording.as_ref().filter(|r| r.host_id == host_id) else {
            return Ok(TargetMeta { resources: Vec::new(), key: String::new() });
        };
        (r.key.clone(), r.resources.clone(), r.resources_overflow, r.auto_stopped.is_some())
    } else {
        let Some(r) = att.cpu_started.get(&host_id) else {
            return Ok(TargetMeta { resources: Vec::new(), key: String::new() });
        };
        (r.key.clone(), r.resources.clone(), r.resources_overflow, r.auto_stopped.is_some())
    };
    if overflow {
        return Err(Outcome::fail(failure_code::OPERATION_FAILED, RESOURCES_OVERFLOW_MSG));
    }
    if !stopped {
        let live = att
            .source_urls(app, host_id)
            .map_err(|e| Outcome::fail(failure_code::OPERATION_FAILED, e))?;
        resources.extend(live);
    }
    resources.sort();
    resources.dedup();
    Ok(TargetMeta { resources, key })
}

pub(super) fn authorized_matches(expected: Option<&TargetMeta>, key: &str, resources: &[String]) -> bool {
    let Some(want) = expected else {
        return true;
    };
    let mut want_res = want.resources.clone();
    want_res.sort();
    want_res.dedup();
    let mut have_res = resources.to_vec();
    have_res.sort();
    have_res.dedup();
    want.key == key && want_res == have_res
}

pub(super) fn wire_file_from_capture(att: &SessionAttachment, host_id: u64, file_id: &str) -> Result<WireFile, String> {
    let cap = att
        .captures
        .get(&host_id)
        .and_then(|list| list.iter().find(|c| c.id == file_id))
        .ok_or_else(|| "Capture expired before stop; record a shorter profile.".to_owned())?;
    WireFile::from_bytes(cap.id.clone(), cap.name.clone(), cap.mime.clone(), &cap.bytes)
}

pub(super) fn finalize_trace(
    att: &mut SessionAttachment,
    app: &App,
    rec: &mut TraceRecording,
    events: Vec<Value>,
    now: Instant,
) -> Result<(String, f64, bool), String> {
    if rec.resources_overflow {
        return Err(RESOURCES_OVERFLOW_MSG.to_owned());
    }
    let pid = rec
        .renderer_pid
        .ok_or_else(|| "Browser engine did not report a renderer process ID for trace isolation.".to_owned())?;
    let end_urls = att.source_urls(app, rec.host_id)?;
    rec.resources.extend(end_urls);
    rec.resources.sort();
    rec.resources.dedup();
    let duration_ms = now.saturating_duration_since(rec.started_at).as_secs_f64() * 1000.0;
    let payload = serde_json::to_vec(&json!({
        "traceEvents": events,
        "metadata": { "source": "opencode", "scope": "renderer-process", "processId": pid },
    }))
    .unwrap_or_default();
    let gz = gzip_bytes(&payload)?;
    let file_id = new_file_id();
    att.record_capture(rec.host_id, CapturedFile {
        id: file_id.clone(),
        name: "trace.json.gz".into(),
        mime: "application/gzip".into(),
        bytes: gz,
        resources: rec.resources.clone(),
    });
    Ok((file_id, duration_ms, rec.data_loss))
}

pub(super) fn finalize_cpu(
    att: &mut SessionAttachment,
    app: &App,
    host_id: u64,
    rec: &mut CpuRecording,
    resp: Result<Value, Value>,
    now: Instant,
) -> Result<(String, f64), String> {
    if rec.resources_overflow {
        return Err(RESOURCES_OVERFLOW_MSG.to_owned());
    }
    let val = resp.map_err(|e| cdp_err_msg(&e))?;
    let end_urls = att.source_urls(app, host_id)?;
    rec.resources.extend(end_urls);
    rec.resources.sort();
    rec.resources.dedup();
    let profile = val.get("profile").cloned().unwrap_or(val);
    let bytes = serde_json::to_vec(&profile).unwrap_or_default();
    if bytes.len() > MAX_FILE_BYTES {
        return Err("CPU profile exceeds the 5 MiB transfer limit.".into());
    }
    let file_id = new_file_id();
    att.record_capture(host_id, CapturedFile {
        id: file_id.clone(),
        name: "profile.cpuprofile".into(),
        mime: "application/json".into(),
        bytes,
        resources: rec.resources.clone(),
    });
    let profile_ms = profile
        .get("startTime")
        .and_then(Value::as_f64)
        .zip(profile.get("endTime").and_then(Value::as_f64))
        .map(|(s, e)| ((e - s) / 1000.0).max(0.0))
        .filter(|d| d.is_finite());
    let duration_ms = profile_ms.unwrap_or_else(|| now.saturating_duration_since(rec.started_at).as_secs_f64() * 1000.0);
    Ok((file_id, duration_ms))
}

pub(super) fn gzip_bytes(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(raw).map_err(|e| format!("Gzip compression failed: {e}"))?;
    let out = enc.finish().map_err(|e| format!("Gzip finish failed: {e}"))?;
    if out.len() > MAX_FILE_BYTES {
        return Err("Compressed capture exceeds the 5 MiB transfer limit.".into());
    }
    Ok(out)
}
