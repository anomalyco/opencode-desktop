//! Reading, inspection, diagnostics, wait, screenshot, and file-list/get executors for
//! `experimental.browser` v4 (`frames`, `snapshot`, `find`, `evaluate`, `wait`, `screenshot`,
//! `console`, `network.list`, `network.get`, `files.list`, `files.get`, and `inspect: true`).

use std::collections::HashMap;
use std::io::Cursor;
use std::time::Duration;

use base64::Engine as _;
use browser_pane::policy::{extension_origin, extension_page_refusal, matches_url};
use browser_pane::tools::diagnostics::TabDiagnostics;
use browser_pane::tools::field_value::{DESCRIBE_FIELD_JS, FieldInfo, readable_field};
use browser_pane::tools::frames::parse_frame_tree;
use browser_pane::tools::snapshot::{DEFAULT_MAX_CHARS, PreparedSnapshot, SnapshotOptions, prepare_snapshot};
use opencode_kit::browser_rpc::{
    Command, MAX_FILE_BYTES, MAX_WAIT_MS, Outcome, TargetMeta, WireFile, failure_code, new_file_id,
};
use rune::time::Instant;
use serde_json::{Value, json};

use super::exec::{ExecPhase, StepOutcome, check_page_guards};
use super::session::{CapturedFile, SessionAttachment};
use crate::App;

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(120);
const MAX_FULLPAGE_EDGE: f64 = 16_384.0;
const MAX_FULLPAGE_PIXELS: f64 = 16_000_000.0;

pub(super) struct FramesState {
    pub host_id: u64,
    pub cdp_req: u64,
}

pub(super) struct SnapshotState {
    pub host_id: u64,
    pub frame_id: Option<String>,
    pub opts: SnapshotOptions,
    pub stage: SnapshotStage,
}

pub(super) enum SnapshotStage {
    CheckFrame { cdp_req: u64 },
    FetchAx { cdp_req: u64 },
    ResolveFields {
        prepared: PreparedSnapshot,
        queue: Vec<(i64, String)>,
        readable: HashMap<i64, bool>,
        boxes: HashMap<i64, String>,
        box_queue: Vec<i64>,
        step: FieldResolveStep,
    },
    FetchBoxes {
        prepared: PreparedSnapshot,
        readable: HashMap<i64, bool>,
        boxes: HashMap<i64, String>,
        box_queue: Vec<i64>,
        current_id: i64,
        cdp_req: u64,
    },
}

pub(super) enum FieldResolveStep {
    ResolveNode { id: i64, name: String, cdp_req: u64 },
    CallDescribe { id: i64, name: String, object_id: String, cdp_req: u64 },
}

pub(super) struct EvalState {
    pub host_id: u64,
    pub script: String,
    pub stage: EvalStage,
}

pub(super) enum EvalStage {
    Direct { cdp_req: u64 },
    ResolveRef { cdp_req: u64 },
    CallOnRef { object_id: String, cdp_req: u64 },
}

pub(super) struct WaitState {
    pub host_id: u64,
    pub condition: String,
    pub text: Option<String>,
    pub url_pattern: Option<String>,
    pub deadline: Instant,
    pub next_poll: Instant,
    pub cdp_req: Option<u64>,
}

pub(super) struct ScreenshotState {
    pub host_id: u64,
    pub format: String,
    pub quality: Option<u64>,
    pub max_width: Option<u32>,
    pub stage: ScreenshotStage,
}

pub(super) enum ScreenshotStage {
    GetBox { cdp_req: u64 },
    GetMetrics { cdp_req: u64 },
    Capture { cdp_req: u64 },
}

/// Handles `command.inspect == true` metadata-only authorization probes (`chromium.ts:1956-2002`).
/// Never reads page text, headers, bodies, or file bytes.
pub(super) fn inspect_target(att: &SessionAttachment, app: &App, cmd: &Command) -> Outcome {
    let ty = cmd.action_type().unwrap_or("");
    let Some(wire_id) = cmd.tab_id() else {
        let mut resources = Vec::new();
        if let Some(u) = cmd.action.get("url").and_then(Value::as_str) {
            resources.push(u.to_owned());
        } else if let Some(p) = cmd.action.get("path").and_then(Value::as_str) {
            resources.push(p.to_owned());
        }
        let target = TargetMeta { resources, key: ty.to_owned() };
        return Outcome::ok(serde_json::to_value(target).unwrap_or(Value::Null), Vec::new());
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return Outcome::fail(failure_code::OPERATION_FAILED, msg),
    };
    let Some(tab) = app.workspace_panes.browser.host.tab(host_id) else {
        return Outcome::fail(failure_code::OPERATION_FAILED, "Browser tab unavailable.");
    };
    let generation = att.generation_of(host_id);
    let target = if ty == "dialog" {
        let d = tab.prompts.dialog.open();
        let key = match d {
            Some(open) => format!("dialog:{generation}:{}", open.id),
            None => format!("dialog:{generation}:none"),
        };
        TargetMeta { resources: vec![tab.url.clone()], key }
    } else if let Some(file_id) = cmd.action.get("fileID").and_then(Value::as_str) {
        let mut resources: Vec<String> = att
            .captures
            .get(&host_id)
            .and_then(|list| list.iter().find(|c| c.id == file_id))
            .map(|c| c.resources.clone())
            .unwrap_or_else(|| vec![tab.url.clone()]);
        resources.sort();
        resources.dedup();
        TargetMeta { resources, key: format!("file:{file_id}") }
    } else if ty == "network.get" {
        let req_id = cmd.action.get("id").and_then(Value::as_str).unwrap_or("");
        let res_url = att
            .diagnostics
            .get(&host_id)
            .and_then(|d| d.network_hop(req_id))
            .map(|h| h.url.clone())
            .unwrap_or_else(|| tab.url.clone());
        TargetMeta { resources: vec![res_url], key: req_id.to_owned() }
    } else if ty == "navigate" {
        let url = cmd
            .action
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| tab.url.clone());
        TargetMeta { resources: vec![url], key: format!("nav:{generation}") }
    } else {
        TargetMeta { resources: vec![tab.url.clone()], key: format!("{ty}:{generation}:{wire_id}") }
    };
    Outcome::ok(serde_json::to_value(target).unwrap_or(Value::Null), Vec::new())
}

pub(super) fn try_start(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
) -> Option<StepOutcome> {
    let ty = cmd.action_type()?;
    match ty {
        "frames" => Some(start_frames(att, app, cmd)),
        "snapshot" | "find" => Some(start_snapshot(att, app, cmd, ty == "find")),
        "evaluate" => Some(start_evaluate(att, app, cmd)),
        "wait" => Some(start_wait(att, app, cmd, now)),
        "screenshot" => Some(start_screenshot(att, app, cmd)),
        "console" => Some(exec_console(att, app, cmd)),
        "network.list" => Some(exec_network_list(att, app, cmd)),
        "network.get" => Some(exec_network_get(att, app, cmd)),
        "files.list" => Some(exec_files_list(att, app, cmd)),
        "files.get" => Some(exec_files_get(att, app, cmd)),
        _ => None,
    }
}

fn start_frames(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "frames") {
        return StepOutcome::Done(o);
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    match app.workspace_panes.browser.host.send_cdp(host_id, "Page.getFrameTree", json!({}), None) {
        Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Frames(FramesState { host_id, cdp_req })),
        Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    }
}

fn start_snapshot(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    is_find: bool,
) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, if is_find { "find" } else { "snapshot" }) {
        return StepOutcome::Done(o);
    }
    let root = if let Some(r) = cmd.action.get("ref").and_then(Value::as_str) {
        match att.refs.get(&host_id).and_then(|t| t.resolve(r)) {
            Some(bid) => Some(bid),
            None => {
                return StepOutcome::Done(Outcome::fail(
                    failure_code::STALE_REF,
                    format!("Element ref {r} is stale or absent. Refresh the snapshot without ref."),
                ));
            }
        }
    } else {
        None
    };
    let frame_id = cmd.action.get("frameID").and_then(Value::as_str).map(str::to_owned);
    let opts = SnapshotOptions {
        root,
        depth: cmd.action.get("depth").and_then(Value::as_u64).map(|v| v as usize),
        text: if is_find {
            cmd.action.get("text").and_then(Value::as_str).map(str::to_owned)
        } else {
            None
        },
        interactive: cmd.action.get("interactive").and_then(Value::as_bool).unwrap_or(false),
        boxes: cmd.action.get("boxes").and_then(Value::as_bool).unwrap_or(false),
        max_lines: cmd.action.get("maxLines").and_then(Value::as_u64).map(|v| v as usize),
        max_chars: cmd
            .action
            .get("maxChars")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_CHARS, |v| (v as usize).clamp(1_000, 100_000)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    if frame_id.is_some() {
        match app.workspace_panes.browser.host.send_cdp(host_id, "Page.getFrameTree", json!({}), None) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Snapshot(SnapshotState {
                host_id,
                frame_id,
                opts,
                stage: SnapshotStage::CheckFrame { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    } else {
        match app.workspace_panes.browser.host.send_cdp(host_id, "Accessibility.getFullAXTree", json!({}), None) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Snapshot(SnapshotState {
                host_id,
                frame_id: None,
                opts,
                stage: SnapshotStage::FetchAx { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    }
}

fn start_evaluate(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let Some(script) = cmd.action.get("script").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing script."));
    };
    let ref_arg = cmd.action.get("ref").and_then(Value::as_str);
    let frame_arg = cmd.action.get("frameID").and_then(Value::as_str);
    if ref_arg.is_some() && frame_arg.is_some() {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Pass either ref or frameID to browser.evaluate, not both.",
        ));
    }
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "evaluate") {
        return StepOutcome::Done(o);
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    if let Some(r) = ref_arg {
        let Some(bid) = att.refs.get(&host_id).and_then(|t| t.resolve(r)) else {
            return StepOutcome::Done(Outcome::fail(
                failure_code::STALE_REF,
                format!("Element ref {r} is stale or absent. Refresh the snapshot first."),
            ));
        };
        match app.workspace_panes.browser.host.send_cdp(
            host_id,
            "DOM.resolveNode",
            json!({ "backendNodeId": bid }),
            None,
        ) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Eval(EvalState {
                host_id,
                script: script.to_owned(),
                stage: EvalStage::ResolveRef { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    } else {
        match app.workspace_panes.browser.host.send_cdp(
            host_id,
            "Runtime.evaluate",
            json!({
                "expression": script,
                "awaitPromise": true,
                "returnByValue": true,
                "userGesture": false,
            }),
            None,
        ) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Eval(EvalState {
                host_id,
                script: script.to_owned(),
                stage: EvalStage::Direct { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    }
}

fn start_wait(att: &mut SessionAttachment, app: &mut App, cmd: &Command, now: Instant) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "wait") {
        return StepOutcome::Done(o);
    }
    let condition = cmd
        .action
        .get("condition")
        .and_then(Value::as_str)
        .unwrap_or("load")
        .to_owned();
    let timeout_ms = cmd
        .action
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(10_000)
        .min(MAX_WAIT_MS);
    let text = cmd.action.get("text").and_then(Value::as_str).map(str::to_owned);
    let url_pattern = cmd.action.get("url").and_then(Value::as_str).map(str::to_owned);
    if matches!(condition.as_str(), "text" | "textGone")
        && text.as_deref().is_none_or(str::is_empty)
    {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "browser.wait requires non-empty text for condition 'text' or 'textGone'.",
        ));
    }
    if condition == "url" && url_pattern.is_none() {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "A URL wait requires a url pattern.",
        ));
    }
    StepOutcome::Pending(ExecPhase::Wait(WaitState {
        host_id,
        condition,
        text,
        url_pattern,
        deadline: now + Duration::from_millis(timeout_ms),
        next_poll: now,
        cdp_req: None,
    }))
}

fn start_screenshot(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let ref_arg = cmd.action.get("ref").and_then(Value::as_str);
    let full_page = cmd.action.get("fullPage").and_then(Value::as_bool).unwrap_or(false);
    if ref_arg.is_some() && full_page {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Choose either ref for an element screenshot or fullPage:true for the whole page.",
        ));
    }
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "screenshot") {
        return StepOutcome::Done(o);
    }
    let format = cmd
        .action
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("png")
        .to_owned();
    let quality = cmd.action.get("quality").and_then(Value::as_u64);
    let max_width = cmd.action.get("maxWidth").and_then(Value::as_u64).map(|v| v as u32);
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);

    if let Some(r) = ref_arg {
        let Some(bid) = att.refs.get(&host_id).and_then(|t| t.resolve(r)) else {
            return StepOutcome::Done(Outcome::fail(
                failure_code::STALE_REF,
                format!("Element ref {r} is stale or absent. Refresh the snapshot first."),
            ));
        };
        match app.workspace_panes.browser.host.send_cdp(
            host_id,
            "DOM.getBoxModel",
            json!({ "backendNodeId": bid }),
            None,
        ) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Screenshot(ScreenshotState {
                host_id,
                format,
                quality,
                max_width,
                stage: ScreenshotStage::GetBox { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    } else if full_page {
        match app.workspace_panes.browser.host.send_cdp(host_id, "Page.getLayoutMetrics", json!({}), None) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Screenshot(ScreenshotState {
                host_id,
                format,
                quality,
                max_width,
                stage: ScreenshotStage::GetMetrics { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    } else {
        let mut params = json!({ "format": format });
        if let Some(q) = quality
            && format != "png"
        {
            params["quality"] = json!(q);
        }
        match app.workspace_panes.browser.host.send_cdp(host_id, "Page.captureScreenshot", params, None) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Screenshot(ScreenshotState {
                host_id,
                format,
                quality,
                max_width,
                stage: ScreenshotStage::Capture { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    }
}

fn exec_console(att: &mut SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "console") {
        return StepOutcome::Done(o);
    }
    let Some(tab) = att.wire_tab(app, host_id) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    let level = cmd.action.get("level").and_then(Value::as_str);
    let limit = cmd.action.get("limit").and_then(Value::as_u64).map(|v| v as usize);
    let mut val = att
        .diagnostics
        .get(&host_id)
        .map(|d| d.query_console(level, limit))
        .unwrap_or_else(|| json!({ "messages": [], "truncated": false, "dropped": 0 }));
    val["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
    StepOutcome::Done(Outcome::ok(val, Vec::new()))
}

fn exec_network_list(att: &mut SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "network.list") {
        return StepOutcome::Done(o);
    }
    let Some(tab) = att.wire_tab(app, host_id) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    let url_contains = cmd.action.get("urlContains").and_then(Value::as_str);
    let rtype = cmd.action.get("resourceType").and_then(Value::as_str);
    let limit = cmd.action.get("limit").and_then(Value::as_u64).map(|v| v as usize);
    let mut val = att
        .diagnostics
        .get(&host_id)
        .map(|d| d.query_network_list(url_contains, rtype, limit))
        .unwrap_or_else(|| json!({ "requests": [], "truncated": false, "dropped": 0 }));
    val["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
    StepOutcome::Done(Outcome::ok(val, Vec::new()))
}

fn exec_network_get(att: &mut SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let Some(req_id) = cmd.action.get("id").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing request id."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "network.get") {
        return StepOutcome::Done(o);
    }
    let Some(tab) = att.wire_tab(app, host_id) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    let Some(hop) = att.diagnostics.get(&host_id).and_then(|d| d.network_hop(req_id)) else {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Request ID is no longer retained in this tab or belongs to another tab. Call browser.network.list({tabID}) and copy a current id.",
        ));
    };
    let include_body = cmd.action.get("includeBody").and_then(Value::as_bool).unwrap_or(false);
    let include_resp = cmd
        .action
        .get("includeResponseBody")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_body = cmd
        .action
        .get("maxBodyChars")
        .and_then(Value::as_u64)
        .map_or(20_000, |v| (v as usize).clamp(1, 20_000));
    let mut val = TabDiagnostics::format_network_get(hop, include_body, include_resp, max_body, None);
    val["tab"] = serde_json::to_value(tab).unwrap_or(Value::Null);
    StepOutcome::Done(Outcome::ok(val, Vec::new()))
}

fn exec_files_list(att: &mut SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "files.list") {
        return StepOutcome::Done(o);
    }
    let Some(tab) = att.wire_tab(app, host_id) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    let mut files: Vec<Value> = att
        .captures
        .get(&host_id)
        .into_iter()
        .flatten()
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.name,
                "mime": c.mime,
                "bytes": c.bytes.len(),
                "state": "completed",
            })
        })
        .collect();
    if let Some(bt) = app.workspace_panes.browser.host.tab(host_id) {
        for d in &bt.files {
            if !opencode_kit::browser_rpc::is_valid_file_id(&d.id) {
                continue;
            }
            let st = match d.state {
                browser_pane::DownloadState::Pending => "pending",
                browser_pane::DownloadState::Completed => "completed",
                browser_pane::DownloadState::Failed => "failed",
            };
            files.push(json!({
                "id": d.id,
                "name": d.name,
                "mime": d.mime,
                "bytes": d.bytes,
                "state": st,
            }));
        }
    }
    StepOutcome::Done(Outcome::ok(json!({ "tab": tab, "files": files }), Vec::new()))
}

fn exec_files_get(att: &mut SessionAttachment, app: &App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let Some(file_id) = cmd.action.get("fileID").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing fileID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "files.get") {
        return StepOutcome::Done(o);
    }
    let Some(tab) = att.wire_tab(app, host_id) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    if let Some(cap) = att
        .captures
        .get(&host_id)
        .and_then(|list| list.iter().find(|c| c.id == file_id))
    {
        return match WireFile::from_bytes(cap.id.clone(), cap.name.clone(), cap.mime.clone(), &cap.bytes) {
            Ok(wf) => StepOutcome::Done(Outcome::ok(json!({ "tab": tab }), vec![wf])),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        };
    }
    if let Some(bt) = app.workspace_panes.browser.host.tab(host_id)
        && let Some(df) = bt.files.iter().find(|f| f.id == file_id)
    {
        if df.state != browser_pane::DownloadState::Completed {
            return StepOutcome::Done(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Download is not completed yet.",
            ));
        }
        let Some(verified) = app.workspace_panes.browser.host.download_path(host_id, df) else {
            return StepOutcome::Done(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Download path failed confinement verification.",
            ));
        };
        let Ok(bytes) = std::fs::read(&verified) else {
            return StepOutcome::Done(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Could not read verified download file.",
            ));
        };
        return match WireFile::from_bytes(df.id.clone(), df.name.clone(), df.mime.clone(), &bytes) {
            Ok(wf) => StepOutcome::Done(Outcome::ok(json!({ "tab": tab }), vec![wf])),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        };
    }
    StepOutcome::Done(Outcome::fail(
        failure_code::OPERATION_FAILED,
        "File ID not found in this tab. Call browser.files.list({tabID}) to see owned files.",
    ))
}

pub(super) fn step_frames(att: &SessionAttachment, app: &mut App, st: &FramesState) -> Option<Outcome> {
    let resp = app.workspace_panes.browser.host.take_cdp_response(st.cdp_req)?;
    match resp {
        Ok(val) => {
            let parsed = parse_frame_tree(&val);
            let Some(tab) = att.wire_tab(app, st.host_id) else {
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
            };
            Some(Outcome::ok(json!({ "tab": tab, "frames": parsed.frames }), Vec::new()))
        }
        Err(err) => Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&err))),
    }
}

pub(super) fn step_snapshot(
    att: &mut SessionAttachment,
    app: &mut App,
    st: &mut SnapshotState,
    ref_counter: &mut u64,
) -> Option<Outcome> {
    match &mut st.stage {
        SnapshotStage::CheckFrame { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let val = match resp {
                Ok(v) => v,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e))),
            };
            let tree = parse_frame_tree(&val);
            let want = st.frame_id.as_deref().unwrap_or("");
            if tree.extension_frame_ids.contains(want) {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Agent browser tools cannot inspect extension frames.",
                ));
            }
            if !tree.frames.iter().any(|f| f.id == want) {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Frame ID not found in this tab. Call browser.frames({tabID}).",
                ));
            }
            match app.workspace_panes.browser.host.send_cdp(
                st.host_id,
                "Accessibility.getFullAXTree",
                json!({ "frameId": want }),
                None,
            ) {
                Ok(req) => {
                    st.stage = SnapshotStage::FetchAx { cdp_req: req };
                    None
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
        SnapshotStage::FetchAx { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let val = match resp {
                Ok(v) => v,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e))),
            };
            let nodes = val.get("nodes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
            let empty_map = HashMap::new();
            let prev = att.refs.get(&st.host_id).map(|r| &r.backend_to_ref).unwrap_or(&empty_map);
            let picked = att.refs.get(&st.host_id).map(|r| &r.picked).unwrap_or(&empty_map);
            let prepared = match prepare_snapshot(nodes, st.opts.clone(), prev, picked) {
                Ok(p) => p,
                Err(e) => return Some(Outcome::fail(failure_code::STALE_REF, e)),
            };
            let mut val_queue = prepared.value_targets();
            let box_queue = prepared.box_targets();
            if let Some((first_id, first_name)) = val_queue.pop() {
                match app.workspace_panes.browser.host.send_cdp(
                    st.host_id,
                    "DOM.resolveNode",
                    json!({ "backendNodeId": first_id }),
                    None,
                ) {
                    Ok(req) => {
                        st.stage = SnapshotStage::ResolveFields {
                            prepared,
                            queue: val_queue,
                            readable: HashMap::new(),
                            boxes: HashMap::new(),
                            box_queue,
                            step: FieldResolveStep::ResolveNode { id: first_id, name: first_name, cdp_req: req },
                        };
                        None
                    }
                    Err(_) => {
                        let readable = HashMap::new();
                        Some(finish_snapshot(att, app, st.host_id, prepared, &readable, &HashMap::new(), ref_counter))
                    }
                }
            } else if !box_queue.is_empty() {
                advance_boxes_or_finish(att, app, st, prepared, HashMap::new(), HashMap::new(), box_queue, ref_counter)
            } else {
                Some(finish_snapshot(att, app, st.host_id, prepared, &HashMap::new(), &HashMap::new(), ref_counter))
            }
        }
        SnapshotStage::ResolveFields { .. } => {
            let SnapshotStage::ResolveFields {
                prepared,
                mut queue,
                mut readable,
                boxes,
                box_queue,
                step,
            } = std::mem::replace(&mut st.stage, SnapshotStage::FetchAx { cdp_req: 0 })
            else {
                return None;
            };
            match step {
                FieldResolveStep::ResolveNode { id, name, cdp_req } => {
                    let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                        st.stage = SnapshotStage::ResolveFields {
                            prepared,
                            queue,
                            readable,
                            boxes,
                            box_queue,
                            step: FieldResolveStep::ResolveNode { id, name, cdp_req },
                        };
                        return None;
                    };
                    let object_id = resp
                        .ok()
                        .and_then(|v| v.get("object").and_then(|o| o.get("objectId")).and_then(Value::as_str).map(str::to_owned));
                    if let Some(oid) = object_id
                        && let Ok(next_req) = app.workspace_panes.browser.host.send_cdp(
                            st.host_id,
                            "Runtime.callFunctionOn",
                            json!({
                                "objectId": oid,
                                "functionDeclaration": DESCRIBE_FIELD_JS,
                                "returnByValue": true,
                                "userGesture": false,
                            }),
                            None,
                        )
                    {
                        st.stage = SnapshotStage::ResolveFields {
                            prepared,
                            queue,
                            readable,
                            boxes,
                            box_queue,
                            step: FieldResolveStep::CallDescribe { id, name, object_id: oid, cdp_req: next_req },
                        };
                        return None;
                    }
                    // Fail closed to hidden on any resolution error.
                    readable.insert(id, false);
                }
                FieldResolveStep::CallDescribe { id, name, object_id, cdp_req } => {
                    let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                        st.stage = SnapshotStage::ResolveFields {
                            prepared,
                            queue,
                            readable,
                            boxes,
                            box_queue,
                            step: FieldResolveStep::CallDescribe { id, name, object_id, cdp_req },
                        };
                        return None;
                    };
                    let _ = app.workspace_panes.browser.host.send_cdp(
                        st.host_id,
                        "Runtime.releaseObject",
                        json!({ "objectId": object_id }),
                        None,
                    );
                    let ok_read = resp
                        .ok()
                        .and_then(|v| v.get("result").and_then(|r| r.get("value")).and_then(FieldInfo::from_value))
                        .is_some_and(|fi| readable_field(&fi, &name));
                    readable.insert(id, ok_read);
                }
            }
            if let Some((next_id, next_name)) = queue.pop()
                && let Ok(req) = app.workspace_panes.browser.host.send_cdp(
                    st.host_id,
                    "DOM.resolveNode",
                    json!({ "backendNodeId": next_id }),
                    None,
                )
            {
                st.stage = SnapshotStage::ResolveFields {
                    prepared,
                    queue,
                    readable,
                    boxes,
                    box_queue,
                    step: FieldResolveStep::ResolveNode { id: next_id, name: next_name, cdp_req: req },
                };
                return None;
            }
            advance_boxes_or_finish(att, app, st, prepared, readable, boxes, box_queue, ref_counter)
        }
        SnapshotStage::FetchBoxes { .. } => {
            let SnapshotStage::FetchBoxes {
                prepared,
                readable,
                mut boxes,
                box_queue,
                current_id,
                cdp_req,
            } = std::mem::replace(&mut st.stage, SnapshotStage::FetchAx { cdp_req: 0 })
            else {
                return None;
            };
            let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                st.stage = SnapshotStage::FetchBoxes {
                    prepared,
                    readable,
                    boxes,
                    box_queue,
                    current_id,
                    cdp_req,
                };
                return None;
            };
            if let Ok(v) = resp
                && let Some(rect_str) = format_quad_box(&v)
            {
                boxes.insert(current_id, rect_str);
            }
            advance_boxes_or_finish(att, app, st, prepared, readable, boxes, box_queue, ref_counter)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn advance_boxes_or_finish(
    att: &mut SessionAttachment,
    app: &mut App,
    st: &mut SnapshotState,
    prepared: PreparedSnapshot,
    readable: HashMap<i64, bool>,
    boxes: HashMap<i64, String>,
    mut box_queue: Vec<i64>,
    ref_counter: &mut u64,
) -> Option<Outcome> {
    if let Some(next_id) = box_queue.pop()
        && let Ok(req) = app.workspace_panes.browser.host.send_cdp(
            st.host_id,
            "DOM.getContentQuads",
            json!({ "backendNodeId": next_id }),
            None,
        )
    {
        st.stage = SnapshotStage::FetchBoxes {
            prepared,
            readable,
            boxes,
            box_queue,
            current_id: next_id,
            cdp_req: req,
        };
        return None;
    }
    Some(finish_snapshot(att, app, st.host_id, prepared, &readable, &boxes, ref_counter))
}

fn finish_snapshot(
    att: &mut SessionAttachment,
    app: &App,
    host_id: u64,
    prepared: PreparedSnapshot,
    readable: &HashMap<i64, bool>,
    boxes: &HashMap<i64, String>,
    ref_counter: &mut u64,
) -> Outcome {
    let empty_map = HashMap::new();
    let prev = att.refs.get(&host_id).map(|r| &r.backend_to_ref).unwrap_or(&empty_map);
    let picked = att.refs.get(&host_id).map(|r| &r.picked).unwrap_or(&empty_map);
    let out = prepared.finish(readable, boxes, prev, picked, &mut || {
        *ref_counter = ref_counter.saturating_add(1);
        format!("e{ref_counter}")
    });
    att.refs.entry(host_id).or_default().update_from_snapshot(out.refs);
    let Some(tab) = att.wire_tab(app, host_id) else {
        return Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable.");
    };
    Outcome::ok(json!({ "tab": tab, "content": out.content, "truncated": out.truncated }), Vec::new())
}

pub(super) fn step_eval(att: &SessionAttachment, app: &mut App, st: &mut EvalState) -> Option<Outcome> {
    match &st.stage {
        EvalStage::Direct { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            Some(eval_result_to_outcome(att, app, st.host_id, resp, false))
        }
        EvalStage::ResolveRef { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let val = match resp {
                Ok(v) => v,
                Err(_) => {
                    return Some(Outcome::fail(
                        failure_code::STALE_REF,
                        "Element ref no longer exists. Refresh the snapshot first.",
                    ));
                }
            };
            let Some(oid) = val.get("object").and_then(|o| o.get("objectId")).and_then(Value::as_str) else {
                return Some(Outcome::fail(
                    failure_code::STALE_REF,
                    "Element ref could not be resolved. Refresh the snapshot first.",
                ));
            };
            let decl = format!(
                "function(...args) {{ if (!this.isConnected) throw new Error('browser.detached_ref'); return ({}\n).apply(this, args); }}",
                st.script
            );
            match app.workspace_panes.browser.host.send_cdp(
                st.host_id,
                "Runtime.callFunctionOn",
                json!({
                    "objectId": oid,
                    "functionDeclaration": decl,
                    "arguments": [{ "objectId": oid }],
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": false,
                }),
                None,
            ) {
                Ok(req) => {
                    st.stage = EvalStage::CallOnRef { object_id: oid.to_owned(), cdp_req: req };
                    None
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
        EvalStage::CallOnRef { object_id, cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let _ = app.workspace_panes.browser.host.send_cdp(
                st.host_id,
                "Runtime.releaseObject",
                json!({ "objectId": object_id }),
                None,
            );
            Some(eval_result_to_outcome(att, app, st.host_id, resp, true))
        }
    }
}

fn eval_result_to_outcome(
    att: &SessionAttachment,
    app: &App,
    host_id: u64,
    resp: Result<Value, Value>,
    is_ref: bool,
) -> Outcome {
    let val = match resp {
        Ok(v) => v,
        Err(e) => return Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)),
    };
    if let Some(ex) = val.get("exceptionDetails") {
        let desc = ex
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(Value::as_str)
            .or_else(|| ex.get("text").and_then(Value::as_str))
            .unwrap_or("JavaScript exception");
        if is_ref && desc.starts_with("Error: browser.detached_ref") {
            return Outcome::fail(
                failure_code::STALE_REF,
                "Element is no longer attached to the page. Refresh the snapshot first.",
            );
        }
        let clipped: String = desc.chars().take(800).collect();
        return Outcome::fail(
            failure_code::OPERATION_FAILED,
            format!("Page JavaScript threw an exception: {clipped}"),
        );
    }
    let Some(tab) = att.wire_tab(app, host_id) else {
        return Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable.");
    };
    let result_val = val
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null);
    Outcome::ok(json!({ "tab": tab, "value": result_val }), Vec::new())
}

pub(super) fn step_wait(
    att: &SessionAttachment,
    app: &mut App,
    st: &mut WaitState,
    now: Instant,
) -> Option<Outcome> {
    let Some(tab) = app.workspace_panes.browser.host.tab(st.host_id) else {
        return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Browser tab closed while waiting."));
    };
    if extension_origin(&tab.url) {
        return Some(Outcome::fail(
            failure_code::OPERATION_FAILED,
            extension_page_refusal(&tab.url),
        ));
    }
    if st.condition == "delay" {
        if now >= st.deadline {
            let wire = att.wire_tab(app, st.host_id)?;
            return Some(Outcome::ok(serde_json::to_value(wire).unwrap_or(Value::Null), Vec::new()));
        }
        return None;
    }
    if st.condition == "load" {
        if !tab.loading {
            let wire = att.wire_tab(app, st.host_id)?;
            return Some(Outcome::ok(serde_json::to_value(wire).unwrap_or(Value::Null), Vec::new()));
        }
        if now >= st.deadline {
            return Some(Outcome::fail(failure_code::CONDITION_UNMET, "Page was still loading."));
        }
        return None;
    }
    if st.condition == "url" {
        let pat = st.url_pattern.as_deref().unwrap_or("");
        if matches_url(&tab.url, pat) {
            let wire = att.wire_tab(app, st.host_id)?;
            return Some(Outcome::ok(serde_json::to_value(wire).unwrap_or(Value::Null), Vec::new()));
        }
        if now >= st.deadline {
            return Some(Outcome::fail(
                failure_code::CONDITION_UNMET,
                format!("URL did not match '{pat}' (current: {}).", tab.url),
            ));
        }
        return None;
    }
    if let Some(req) = st.cdp_req {
        let resp = app.workspace_panes.browser.host.take_cdp_response(req)?;
        st.cdp_req = None;
        st.next_poll = now + WAIT_POLL_INTERVAL;
        if let Ok(v) = resp {
            let present = v
                .get("result")
                .and_then(|r| r.get("value"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let met = if st.condition == "text" { present } else { !present };
            if met {
                let wire = att.wire_tab(app, st.host_id)?;
                return Some(Outcome::ok(serde_json::to_value(wire).unwrap_or(Value::Null), Vec::new()));
            }
        }
    }
    if now >= st.deadline {
        return Some(Outcome::fail(
            failure_code::CONDITION_UNMET,
            format!("Wait condition '{}' did not hold before timeout.", st.condition),
        ));
    }
    if now >= st.next_poll {
        let needle = serde_json::to_string(st.text.as_deref().unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
        let expr = format!("document.body?.innerText.includes({needle}) ?? false");
        if let Ok(req) = app.workspace_panes.browser.host.send_cdp(
            st.host_id,
            "Runtime.evaluate",
            json!({ "expression": expr, "returnByValue": true, "userGesture": false }),
            None,
        ) {
            st.cdp_req = Some(req);
        } else {
            st.next_poll = now + WAIT_POLL_INTERVAL;
        }
    }
    None
}

pub(super) fn step_screenshot(
    att: &mut SessionAttachment,
    app: &mut App,
    st: &mut ScreenshotState,
) -> Option<Outcome> {
    match &st.stage {
        ScreenshotStage::GetBox { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let val = match resp {
                Ok(v) => v,
                Err(_) => {
                    return Some(Outcome::fail(
                        failure_code::STALE_REF,
                        "Could not read bounding box for element ref.",
                    ));
                }
            };
            let Some((x, y, w, h)) = box_model_rect(&val) else {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Element has zero visible size.",
                ));
            };
            let mut params = json!({
                "format": st.format,
                "clip": { "x": x, "y": y, "width": w, "height": h, "scale": 1.0 }
            });
            if let Some(q) = st.quality
                && st.format != "png"
            {
                params["quality"] = json!(q);
            }
            match app.workspace_panes.browser.host.send_cdp(st.host_id, "Page.captureScreenshot", params, None) {
                Ok(req) => {
                    st.stage = ScreenshotStage::Capture { cdp_req: req };
                    None
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
        ScreenshotStage::GetMetrics { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let val = match resp {
                Ok(v) => v,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e))),
            };
            let rect = val.get("cssContentSize").or_else(|| val.get("contentSize"));
            let w = rect.and_then(|r| r.get("width")).and_then(Value::as_f64).unwrap_or(1280.0).max(1.0);
            let h = rect.and_then(|r| r.get("height")).and_then(Value::as_f64).unwrap_or(800.0).max(1.0);
            let mut scale = 1.0f64;
            if w > MAX_FULLPAGE_EDGE {
                scale = scale.min(MAX_FULLPAGE_EDGE / w);
            }
            if h > MAX_FULLPAGE_EDGE {
                scale = scale.min(MAX_FULLPAGE_EDGE / h);
            }
            if (w * scale) * (h * scale) > MAX_FULLPAGE_PIXELS {
                scale *= (MAX_FULLPAGE_PIXELS / ((w * scale) * (h * scale))).sqrt();
            }
            let mut params = json!({
                "format": st.format,
                "captureBeyondViewport": true,
                "clip": { "x": 0.0, "y": 0.0, "width": w, "height": h, "scale": scale }
            });
            if let Some(q) = st.quality
                && st.format != "png"
            {
                params["quality"] = json!(q);
            }
            match app.workspace_panes.browser.host.send_cdp(st.host_id, "Page.captureScreenshot", params, None) {
                Ok(req) => {
                    st.stage = ScreenshotStage::Capture { cdp_req: req };
                    None
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
        ScreenshotStage::Capture { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let val = match resp {
                Ok(v) => v,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e))),
            };
            let Some(b64) = val.get("data").and_then(Value::as_str) else {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Screenshot response omitted image data.",
                ));
            };
            let Ok(mut bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Screenshot image data was not valid base64.",
                ));
            };
            if let Some(max_w) = st.max_width {
                bytes = maybe_downscale(&bytes, &st.format, max_w);
            }
            if bytes.len() > MAX_FILE_BYTES {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Captured screenshot exceeds the 5 MiB transfer limit. Try format='jpeg' or a smaller maxWidth.",
                ));
            }
            let file_id = new_file_id();
            let ext = if st.format == "jpeg" { "jpg" } else { st.format.as_str() };
            let name = format!("screenshot.{ext}");
            let mime = format!("image/{}", st.format);
            let wf = match WireFile::from_bytes(file_id.clone(), name.clone(), mime.clone(), &bytes) {
                Ok(w) => w,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            };
            let tab_url = app
                .workspace_panes
                .browser
                .host
                .tab(st.host_id)
                .map(|t| t.url.clone())
                .unwrap_or_default();
            att.record_capture(
                st.host_id,
                CapturedFile { id: file_id, name, mime, bytes, resources: vec![tab_url] },
            );
            let Some(tab) = att.wire_tab(app, st.host_id) else {
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
            };
            Some(Outcome::ok(json!({ "tab": tab }), vec![wf]))
        }
    }
}

fn maybe_downscale(bytes: &[u8], format: &str, max_w: u32) -> Vec<u8> {
    if !matches!(format, "png" | "jpeg") {
        return bytes.to_vec();
    }
    let Ok(img) = image::load_from_memory(bytes) else {
        return bytes.to_vec();
    };
    if img.width() <= max_w {
        return bytes.to_vec();
    }
    let new_h = ((img.height() as u64 * max_w as u64) / img.width().max(1) as u64).max(1) as u32;
    let resized = img.resize(max_w, new_h, image::imageops::FilterType::Triangle);
    let mut out = Cursor::new(Vec::new());
    let fmt = if format == "jpeg" {
        image::ImageFormat::Jpeg
    } else {
        image::ImageFormat::Png
    };
    if resized.write_to(&mut out, fmt).is_ok() {
        out.into_inner()
    } else {
        bytes.to_vec()
    }
}

fn format_quad_box(v: &Value) -> Option<String> {
    let quad = v.get("quads")?.as_array()?.first()?.as_array()?;
    if quad.len() < 8 {
        return None;
    }
    let xs = [0, 2, 4, 6].map(|i| quad[i].as_f64().unwrap_or(0.0));
    let ys = [1, 3, 5, 7].map(|i| quad[i].as_f64().unwrap_or(0.0));
    let min_x = xs.into_iter().fold(f64::INFINITY, f64::min);
    let max_x = xs.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.into_iter().fold(f64::INFINITY, f64::min);
    let max_y = ys.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let w = (max_x - min_x).round() as i64;
    let h = (max_y - min_y).round() as i64;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some(format!(" [{},{},{w},{h}]", min_x.round() as i64, min_y.round() as i64))
}

fn box_model_rect(v: &Value) -> Option<(f64, f64, f64, f64)> {
    let border = v.get("model")?.get("border")?.as_array()?;
    if border.len() < 8 {
        return None;
    }
    let xs = [0, 2, 4, 6].map(|i| border[i].as_f64().unwrap_or(0.0));
    let ys = [1, 3, 5, 7].map(|i| border[i].as_f64().unwrap_or(0.0));
    let min_x = xs.into_iter().fold(f64::INFINITY, f64::min);
    let max_x = xs.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.into_iter().fold(f64::INFINITY, f64::min);
    let max_y = ys.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let w = max_x - min_x;
    let h = max_y - min_y;
    (w > 0.0 && h > 0.0).then_some((min_x, min_y, w, h))
}

pub(super) fn cdp_err_msg(err: &Value) -> String {
    err.get("message")
        .and_then(Value::as_str)
        .or_else(|| err.get("error").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| err.to_string())
}
