//! Retained trace, CPU profile, and V8 heap snapshot analyzers (`trace.analyze`, `cpu.analyze`,
//! `heap.summary`, `heap.query`, `heap.object`, `heap.compare`).

use std::io::Read;

use browser_pane::tools::profiling::{HeapSnapshot, analyze_cpu, analyze_trace};
use flate2::read::GzDecoder;
use opencode_kit::browser_rpc::{Command, Outcome, failure_code};
use serde_json::Value;

use super::super::session::SessionAttachment;
use super::{StepOutcome, check_page_guards};
use crate::App;

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
