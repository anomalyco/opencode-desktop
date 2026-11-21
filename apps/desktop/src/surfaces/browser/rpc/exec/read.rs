//! Live target inspection (`inspect: true` & pre/post-execution `command.target` verification),
//! fail-closed `frameID` + `contextId`-pinned JavaScript `evaluate`, and deadline-safe `wait`.

use std::time::Duration;

use browser_pane::policy::{extension_origin, extension_page_refusal, matches_url};
use opencode_kit::browser_rpc::{Command, MAX_WAIT_MS, Outcome, TargetMeta, failure_code};
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::frames::EvalTargetError;
use super::super::session::SessionAttachment;
use super::target_url::parse_agent_url;
use super::{ExecPhase, StepOutcome, cdp_err_msg, check_page_guards, send_rpc_cdp, send_rpc_cdp_in_frame};
use crate::App;

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(120);
/// Hard cap on serialized JSON returned by `browser.evaluate` (`chromium.ts:1552`).
const MAX_EVAL_JSON_CHARS: usize = 512_000;

pub(super) struct InspectState {
    pub host_id: u64,
    pub pending: Vec<(u64, Option<String>)>,
    pub cmd: Command,
}

pub(super) struct VerifyFramesState {
    pub host_id: u64,
    pub pending: Vec<(u64, Option<String>)>,
    pub cmd: Command,
}

pub(super) struct EvalState {
    pub host_id: u64,
    pub script: String,
    pub cdp_session: Option<String>,
    pub stage: EvalStage,
}

impl EvalState {
    pub fn cancel_cleanup(&mut self, app: &mut App) {
        match &self.stage {
            EvalStage::Direct { cdp_req } | EvalStage::ResolveRef { cdp_req } => {
                app.workspace_panes.browser.host.cancel_cdp(*cdp_req);
            }
            EvalStage::CallOnRef { object_id, cdp_req } => {
                app.workspace_panes.browser.host.cancel_cdp(*cdp_req);
                let _ = send_rpc_cdp(
                    app,
                    self.host_id,
                    "Runtime.releaseObject",
                    json!({ "objectId": object_id }),
                    self.cdp_session.clone(),
                );
            }
        }
    }
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
    pub context_id: Option<i64>,
    pub cdp_session: Option<String>,
    pub deadline: Instant,
    pub next_poll: Instant,
    pub cdp_req: Option<u64>,
}

impl WaitState {
    pub fn cancel_cleanup(&mut self, app: &mut App) {
        if let Some(req) = self.cdp_req.take() {
            app.workspace_panes.browser.host.cancel_cdp(req);
        }
    }
}

/// Computes metadata-only authorization target (`resources`, `key`) for `cmd`
/// (`chromium.ts:1956-2002`). Shared by `inspect: true` and pre/post-execution enforcement.
pub(super) fn compute_target(
    att: &SessionAttachment,
    app: &App,
    cmd: &Command,
) -> Result<TargetMeta, Outcome> {
    let ty = cmd.action_type().unwrap_or("");
    let Some(wire_id) = cmd.tab_id() else {
        let mut resources = Vec::new();
        if let Some(u) = cmd.action.get("url").and_then(Value::as_str) {
            let trimmed = u.trim();
            if !trimmed.is_empty() {
                let parsed = parse_agent_url(trimmed)
                    .map_err(|msg| Outcome::fail(failure_code::NAVIGATION_FAILED, msg))?;
                resources.push(parsed);
            }
        } else if let Some(p) = cmd.action.get("path").and_then(Value::as_str) {
            resources.push(p.to_owned());
        }
        resources.sort();
        resources.dedup();
        return Ok(TargetMeta { resources, key: ty.to_owned() });
    };
    let host_id = att
        .resolve_tab(app, wire_id)
        .map_err(|msg| Outcome::fail(failure_code::OPERATION_FAILED, msg))?;
    let Some(tab) = app.workspace_panes.browser.host.tab(host_id) else {
        return Err(Outcome::fail(failure_code::OPERATION_FAILED, "Browser tab unavailable."));
    };
    let generation = att.generation_of(host_id);
    let mut target = if ty == "dialog" {
        let resources = att
            .source_urls(app, host_id)
            .map_err(|e| Outcome::fail(failure_code::OPERATION_FAILED, e))?;
        let key = match tab.prompts.dialog.open() {
            Some(open) => format!("dialog:{generation}:{}", open.id),
            None => format!("dialog:{generation}:none"),
        };
        TargetMeta { resources, key }
    } else if let Some(file_id) = cmd.action.get("fileID").and_then(Value::as_str) {
        let resources = match att
            .captures
            .get(&host_id)
            .and_then(|list| list.iter().find(|c| c.id == file_id))
        {
            Some(c) => c.resources.clone(),
            None => att
                .download_resources(app, host_id, file_id)
                .map_err(|e| Outcome::fail(failure_code::OPERATION_FAILED, e))?,
        };
        TargetMeta { resources, key: format!("file:{file_id}") }
    } else if ty == "network.get" {
        let req_id = cmd.action.get("id").and_then(Value::as_str).unwrap_or("");
        let res_url = att
            .diagnostics
            .get(&host_id)
            .and_then(|d| d.network_hop(req_id))
            .map(|h| h.url.clone())
            .unwrap_or_else(|| tab.url.clone());
        TargetMeta { resources: vec![res_url], key: format!("network:{req_id}") }
    } else if ty == "navigate" {
        let norm = match cmd.action.get("url").and_then(Value::as_str) {
            Some(raw) => parse_agent_url(raw)
                .map_err(|msg| Outcome::fail(failure_code::NAVIGATION_FAILED, msg))?,
            None => tab.url.clone(),
        };
        TargetMeta { resources: vec![norm.clone()], key: format!("navigate:{norm}") }
    } else if ty == "navigate.path" {
        let path = cmd
            .action
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        TargetMeta { resources: vec![path.clone()], key: format!("navigate.path:{generation}:{path}") }
    } else if let Some(fid) = cmd.action.get("frameID").and_then(Value::as_str) {
        let Some(frames) = att.frames.get(&host_id) else {
            return Err(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Frame state unavailable for this tab. Call browser.frames({tabID}) and retry.",
            ));
        };
        if frames.frame_cap_exceeded {
            return Err(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Tab exceeded tracked frame cap; cannot verify frame authorization safely.",
            ));
        }
        let Some(frame_url) = frames.frame_urls.get(fid).cloned() else {
            return Err(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Frame ID was not found in tracked frames. Call browser.frames({tabID}) and retry.",
            ));
        };
        let mut resources = vec![frame_url];
        if !tab.url.is_empty() {
            resources.push(tab.url.clone());
        }
        TargetMeta { resources, key: format!("frame:{generation}:{fid}") }
    } else {
        let resources = att
            .source_urls(app, host_id)
            .map_err(|e| Outcome::fail(failure_code::OPERATION_FAILED, e))?;
        TargetMeta { resources, key: format!("tab:{generation}") }
    };
    target.resources.sort();
    target.resources.dedup();
    Ok(target)
}

/// Returns `true` when `cmd` targets or covers page frames and carries a server-authorized
/// `target` that must be verified against a live `Page.getFrameTree` before execution
/// (matching official `packages/gui-plugins/src/browser/chromium.ts:1518`). Commands that touch
/// no frame (`tabs.*`, `navigate`, `navigate.path`, `back`, `forward`, `reload`, `stop`,
/// `preview`, `network.get`, `files.get`) incur zero extra CDP round trips.
pub(super) fn needs_live_frame_verify(cmd: &Command) -> bool {
    if cmd.target.is_none() || cmd.tab_id().is_none() {
        return false;
    }
    let ty = cmd.action_type().unwrap_or("");
    !matches!(
        ty,
        "navigate"
            | "navigate.path"
            | "network.get"
            | "files.get"
            | "tabs.list"
            | "tabs.open"
            | "tabs.focus"
            | "tabs.close"
            | "back"
            | "forward"
            | "reload"
            | "stop"
            | "preview"
    )
}

/// Queues live `Page.getFrameTree` calls on BOTH the top-level target (`None`) and every tracked
/// non-extension OOPIF session (`Some(sid)`) so in-process child iframes inside top-level and
/// OOPIFs are refreshed even when `#274` suppresses `Page.*` events.
pub(super) fn queue_live_frame_refresh(
    att: &SessionAttachment,
    app: &mut App,
    host_id: u64,
) -> Vec<(u64, Option<String>)> {
    let mut pending = Vec::new();
    if let Ok(req) = send_rpc_cdp(app, host_id, "Page.getFrameTree", json!({}), None) {
        pending.push((req, None));
    }
    if app.workspace_panes.browser.host.engine_agent_tools() {
        let oopif_sids = att
            .frames
            .get(&host_id)
            .map(|f| f.oopif_sessions())
            .unwrap_or_default();
        for sid in oopif_sids {
            if let Ok(req) = app.workspace_panes.browser.host.send_typed_cdp(
                host_id,
                "Page.getFrameTree",
                json!({}),
                Some(sid.clone()),
            ) {
                pending.push((req, Some(sid)));
            }
        }
    }
    pending
}

/// Polls pending `Page.getFrameTree` calls from [`queue_live_frame_refresh`], updating
/// `att.frames` (pruning removed frames & bumping `generation` on reload). Returns `Ok(true)`
/// once all pending replies arrived, `Ok(false)` while waiting, or `Err(msg)` on fatal failure.
pub(super) fn step_live_frame_refresh(
    att: &mut SessionAttachment,
    app: &mut App,
    host_id: u64,
    pending: &mut Vec<(u64, Option<String>)>,
    mut top_tree: Option<&mut Option<Value>>,
) -> Result<bool, String> {
    let mut remaining = Vec::with_capacity(pending.len());
    let mut fatal_err: Option<String> = None;
    for (req_id, sid) in pending.drain(..) {
        match app.workspace_panes.browser.host.take_cdp_response(req_id) {
            Some(Ok(val)) => {
                if att
                    .frames
                    .entry(host_id)
                    .or_default()
                    .ingest_frame_tree(&val, sid.as_deref())
                {
                    att.bump_generation(host_id);
                }
                if sid.is_none()
                    && let Some(slot) = top_tree.as_mut()
                {
                    **slot = Some(val);
                }
            }
            Some(Err(err)) => {
                let msg = cdp_err_msg(&err);
                let still_active_oopif = sid.as_deref().is_some_and(|s| {
                    att.frames
                        .get(&host_id)
                        .is_some_and(|f| f.frame_sessions.values().any(|v| v == s))
                });
                if sid.is_none() || still_active_oopif || msg.to_ascii_lowercase().contains("extension") {
                    fatal_err = Some(msg);
                }
            }
            None => remaining.push((req_id, sid)),
        }
    }
    *pending = remaining;
    if let Some(msg) = fatal_err {
        for (req_id, _) in pending.drain(..) {
            app.workspace_panes.browser.host.cancel_cdp(req_id);
        }
        return Err(msg);
    }
    Ok(pending.is_empty())
}

/// Starts `inspect: true`: refreshes live `Page.getFrameTree` on BOTH the top-level target AND
/// every tracked non-extension OOPIF session so pre-existing in-process child iframes inside
/// OOPIFs are never missed from `resources`.
pub(super) fn start_inspect(att: &mut SessionAttachment, app: &mut App, cmd: Command) -> StepOutcome {
    let ty = cmd.action_type().unwrap_or("");
    let needs_live_frames = cmd.tab_id().is_some()
        && !matches!(ty, "navigate" | "navigate.path" | "network.get" | "files.get");
    if needs_live_frames
        && let Some(wire_id) = cmd.tab_id()
        && let Ok(host_id) = att.resolve_tab(app, wire_id)
        && app.workspace_panes.browser.host.tab_open_in_engine(host_id)
    {
        let pending = queue_live_frame_refresh(att, app, host_id);
        if !pending.is_empty() {
            return StepOutcome::Pending(ExecPhase::Inspect(InspectState { host_id, pending, cmd }));
        }
    }
    match compute_target(att, app, &cmd) {
        Ok(target) => StepOutcome::Done(Outcome::ok(serde_json::to_value(target).unwrap_or(Value::Null), Vec::new())),
        Err(err) => StepOutcome::Done(err),
    }
}

pub(super) fn step_inspect(
    att: &mut SessionAttachment,
    app: &mut App,
    st: &mut InspectState,
) -> Option<Outcome> {
    match step_live_frame_refresh(att, app, st.host_id, &mut st.pending, None) {
        Ok(false) => return None,
        Ok(true) => {}
        Err(msg) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    }
    match compute_target(att, app, &st.cmd) {
        Ok(target) => Some(Outcome::ok(serde_json::to_value(target).unwrap_or(Value::Null), Vec::new())),
        Err(err) => Some(err),
    }
}

pub(super) fn try_start(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
) -> Option<StepOutcome> {
    match cmd.action_type()? {
        "evaluate" => Some(start_evaluate(att, app, cmd)),
        "wait" => Some(start_wait(att, app, cmd, now)),
        _ => None,
    }
}

fn eval_target_outcome(err: EvalTargetError) -> Outcome {
    match err {
        EvalTargetError::DocumentChanged(msg) => Outcome::fail(failure_code::DOCUMENT_CHANGED, msg),
        EvalTargetError::Failed(msg) => Outcome::fail(failure_code::OPERATION_FAILED, msg),
    }
}

fn send_eval_cdp(
    app: &mut App,
    host_id: u64,
    expr: &str,
    await_promise: bool,
    context_id: Option<i64>,
    cdp_session: Option<String>,
) -> Result<u64, String> {
    let mut params = json!({
        "expression": expr,
        "awaitPromise": await_promise,
        "returnByValue": true,
        "userGesture": false,
    });
    if app.workspace_panes.browser.host.engine_agent_tools() {
        let Some(cid) = context_id else {
            return Err("Refusing unpinned Runtime.evaluate without a verified contextId.".into());
        };
        params["contextId"] = json!(cid);
        return app
            .workspace_panes
            .browser
            .host
            .send_typed_cdp(host_id, "Runtime.evaluate", params, cdp_session);
    }
    app.workspace_panes
        .browser
        .host
        .send_cdp(host_id, "Runtime.evaluate", params, None)
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
    att.ensure_tab_cdp(app, host_id);

    if let Some(r) = ref_arg {
        let Some((bid, ref_session)) = att.refs.get(&host_id).and_then(|t| t.resolve(r)) else {
            return StepOutcome::Done(Outcome::fail(
                failure_code::STALE_REF,
                format!("Element ref {r} is stale or absent. Refresh the snapshot first."),
            ));
        };
        let ref_frame = att.refs.get(&host_id).and_then(|t| t.frame_of(r));
        match send_rpc_cdp_in_frame(
            app,
            host_id,
            "DOM.resolveNode",
            json!({ "backendNodeId": bid }),
            ref_session.clone(),
            ref_frame,
        ) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Eval(EvalState {
                host_id,
                script: script.to_owned(),
                cdp_session: ref_session,
                stage: EvalStage::ResolveRef { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    } else {
        let require_pinned = app.workspace_panes.browser.host.engine_agent_tools();
        let (context_id, cdp_session) = match att
            .frames
            .get(&host_id)
            .map(|f| f.resolve_eval_target(frame_arg, require_pinned))
        {
            Some(Ok(v)) => v,
            Some(Err(err)) => return StepOutcome::Done(eval_target_outcome(err)),
            None if require_pinned => {
                return StepOutcome::Done(Outcome::fail(
                    failure_code::DOCUMENT_CHANGED,
                    "Frame context is not ready yet. Refresh state and retry.",
                ));
            }
            None if frame_arg.is_some() => {
                return StepOutcome::Done(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Frame ID was not found in this tab. Call browser.frames({tabID}) first.",
                ));
            }
            None => (None, None),
        };
        match send_eval_cdp(app, host_id, script, true, context_id, cdp_session.clone()) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Eval(EvalState {
                host_id,
                script: script.to_owned(),
                cdp_session,
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
    if !matches!(condition.as_str(), "load" | "delay" | "url" | "text" | "textGone") {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            format!("Unsupported wait condition '{condition}'."),
        ));
    }
    let timeout_ms = cmd
        .action
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(10_000)
        .min(MAX_WAIT_MS);
    let text = cmd.action.get("text").and_then(Value::as_str).map(str::to_owned);
    let url_pattern = cmd.action.get("url").and_then(Value::as_str).map(str::to_owned);
    let frame_arg = cmd.action.get("frameID").and_then(Value::as_str);
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
    let (context_id, cdp_session) = if matches!(condition.as_str(), "text" | "textGone") {
        let require_pinned = app.workspace_panes.browser.host.engine_agent_tools();
        match att
            .frames
            .get(&host_id)
            .map(|f| f.resolve_eval_target(frame_arg, require_pinned))
        {
            Some(Ok(v)) => v,
            Some(Err(err)) => return StepOutcome::Done(eval_target_outcome(err)),
            None if require_pinned => {
                return StepOutcome::Done(Outcome::fail(
                    failure_code::DOCUMENT_CHANGED,
                    "Frame context is not ready yet. Refresh state and retry.",
                ));
            }
            None if frame_arg.is_some() => {
                return StepOutcome::Done(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Frame ID was not found in this tab. Call browser.frames({tabID}) first.",
                ));
            }
            None => (None, None),
        }
    } else {
        (None, None)
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    att.ensure_tab_cdp(app, host_id);
    StepOutcome::Pending(ExecPhase::Wait(WaitState {
        host_id,
        condition,
        text,
        url_pattern,
        context_id,
        cdp_session,
        deadline: now + Duration::from_millis(timeout_ms),
        next_poll: now,
        cdp_req: None,
    }))
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
            match send_rpc_cdp(
                app,
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
                st.cdp_session.clone(),
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
            let _ = send_rpc_cdp(
                app,
                st.host_id,
                "Runtime.releaseObject",
                json!({ "objectId": object_id }),
                st.cdp_session.clone(),
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
    let encoded_len = serde_json::to_string(&result_val).map_or(usize::MAX, |s| s.len());
    if encoded_len > MAX_EVAL_JSON_CHARS {
        return Outcome::fail(
            failure_code::OPERATION_FAILED,
            format!(
                "browser.evaluate result ({encoded_len} chars) exceeds the {MAX_EVAL_JSON_CHARS} character limit. Return a smaller summary or slice."
            ),
        );
    }
    Outcome::ok(json!({ "tab": tab, "value": result_val }), Vec::new())
}

fn wire_tab_or_fail(att: &SessionAttachment, app: &App, host_id: u64) -> Outcome {
    match att.wire_tab(app, host_id) {
        Some(wire) => Outcome::ok(serde_json::to_value(wire).unwrap_or(Value::Null), Vec::new()),
        None => Outcome::fail(failure_code::OPERATION_FAILED, "Browser tab closed while waiting."),
    }
}

pub(super) fn step_wait(
    att: &SessionAttachment,
    app: &mut App,
    st: &mut WaitState,
    now: Instant,
) -> Option<Outcome> {
    let Some((tab_url, tab_loading)) = app
        .workspace_panes
        .browser
        .host
        .tab(st.host_id)
        .map(|t| (t.url.clone(), t.loading))
    else {
        st.cancel_cleanup(app);
        return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Browser tab closed while waiting."));
    };
    if extension_origin(&tab_url) {
        st.cancel_cleanup(app);
        return Some(Outcome::fail(
            failure_code::OPERATION_FAILED,
            extension_page_refusal(&tab_url),
        ));
    }
    if st.condition == "delay" {
        if now >= st.deadline {
            return Some(wire_tab_or_fail(att, app, st.host_id));
        }
        return None;
    }
    if st.condition == "load" {
        if !tab_loading {
            return Some(wire_tab_or_fail(att, app, st.host_id));
        }
        if now >= st.deadline {
            return Some(Outcome::fail(failure_code::CONDITION_UNMET, "Page was still loading."));
        }
        return None;
    }
    if st.condition == "url" {
        let pat = st.url_pattern.as_deref().unwrap_or("");
        if matches_url(&tab_url, pat) {
            return Some(wire_tab_or_fail(att, app, st.host_id));
        }
        if now >= st.deadline {
            return Some(Outcome::fail(
                failure_code::CONDITION_UNMET,
                format!("URL did not match '{pat}' (current: {tab_url})."),
            ));
        }
        return None;
    }
    if now >= st.deadline {
        st.cancel_cleanup(app);
        return Some(Outcome::fail(
            failure_code::CONDITION_UNMET,
            format!("Wait condition '{}' did not hold before timeout.", st.condition),
        ));
    }
    if let Some(req) = st.cdp_req {
        let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req) else {
            st.next_poll = (now + WAIT_POLL_INTERVAL).min(st.deadline);
            return None;
        };
        st.cdp_req = None;
        st.next_poll = (now + WAIT_POLL_INTERVAL).min(st.deadline);
        if let Ok(v) = resp {
            let present = v
                .get("result")
                .and_then(|r| r.get("value"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let met = if st.condition == "text" { present } else { !present };
            if met {
                return Some(wire_tab_or_fail(att, app, st.host_id));
            }
        }
    }
    if now >= st.next_poll {
        let needle = serde_json::to_string(st.text.as_deref().unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
        let expr = format!("document.body?.innerText.includes({needle}) ?? false");
        match send_eval_cdp(app, st.host_id, &expr, false, st.context_id, st.cdp_session.clone()) {
            Ok(req) => {
                st.cdp_req = Some(req);
                st.next_poll = (now + WAIT_POLL_INTERVAL).min(st.deadline);
            }
            Err(_) => {
                st.next_poll = (now + WAIT_POLL_INTERVAL).min(st.deadline);
            }
        }
    }
    None
}
