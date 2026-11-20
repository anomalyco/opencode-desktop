//! Target inspection (`inspect: true` & pre-execution `command.target` verification),
//! `frameID`-aware + `contextId`-pinned JavaScript `evaluate`, and deadline-safe `wait`.

use std::time::Duration;

use browser_pane::policy::{extension_origin, extension_page_refusal, matches_url, with_scheme};
use opencode_kit::browser_rpc::{Command, MAX_WAIT_MS, Outcome, TargetMeta, failure_code};
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::{ExecPhase, StepOutcome, cdp_err_msg, check_page_guards};
use crate::App;

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(120);
/// Hard cap on serialized JSON returned by `browser.evaluate` (`chromium.ts:1552`).
const MAX_EVAL_JSON_CHARS: usize = 512_000;

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
                let _ = app.workspace_panes.browser.host.send_cdp(
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
/// (`chromium.ts:1956-2002`). Shared by `inspect: true` and pre-execution `command.target`
/// enforcement. Never reads page text, headers, bodies, or file bytes.
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
                resources.push(with_scheme(trimmed));
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
        let key = match tab.prompts.dialog.open() {
            Some(open) => format!("dialog:{generation}:{}", open.id),
            None => format!("dialog:{generation}:none"),
        };
        TargetMeta { resources: att.source_urls(app, host_id), key }
    } else if let Some(file_id) = cmd.action.get("fileID").and_then(Value::as_str) {
        let resources = att
            .captures
            .get(&host_id)
            .and_then(|list| list.iter().find(|c| c.id == file_id))
            .map(|c| c.resources.clone())
            .unwrap_or_else(|| att.download_resources(app, host_id, file_id));
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
        let norm = cmd
            .action
            .get("url")
            .and_then(Value::as_str)
            .map(|u| with_scheme(u.trim()))
            .unwrap_or_else(|| tab.url.clone());
        TargetMeta { resources: vec![norm.clone()], key: format!("navigate:{norm}") }
    } else if let Some(fid) = cmd.action.get("frameID").and_then(Value::as_str) {
        let frame_url = att
            .frames
            .get(&host_id)
            .and_then(|f| f.frame_urls.get(fid))
            .cloned()
            .unwrap_or_else(|| tab.url.clone());
        let mut resources = vec![frame_url];
        if !tab.url.is_empty() {
            resources.push(tab.url.clone());
        }
        TargetMeta { resources, key: format!("frame:{generation}:{fid}") }
    } else {
        // Include ALL document + cross-origin iframe URLs (`sourceURLs()`) so server deny rules
        // apply across framed documents for `screenshot`, `snapshot`, `find`, `evaluate`, etc.
        TargetMeta {
            resources: att.source_urls(app, host_id),
            key: format!("tab:{generation}"),
        }
    };
    target.resources.sort();
    target.resources.dedup();
    Ok(target)
}

/// Handles `command.inspect == true` metadata-only authorization probes (`chromium.ts:1956-2002`).
pub(super) fn inspect_target(att: &SessionAttachment, app: &App, cmd: &Command) -> Outcome {
    match compute_target(att, app, cmd) {
        Ok(target) => Outcome::ok(serde_json::to_value(target).unwrap_or(Value::Null), Vec::new()),
        Err(err) => err,
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
    if let Some(cid) = context_id
        && app.workspace_panes.browser.host.engine_agent_tools()
    {
        params["contextId"] = json!(cid);
        return app
            .workspace_panes
            .browser
            .host
            .send_typed_cdp(host_id, "Runtime.evaluate", params, cdp_session);
    }
    if cdp_session.is_some() && app.workspace_panes.browser.host.engine_agent_tools() {
        return app
            .workspace_panes
            .browser
            .host
            .send_typed_cdp(host_id, "Runtime.evaluate", params, cdp_session);
    }
    app.workspace_panes
        .browser
        .host
        .send_cdp(host_id, "Runtime.evaluate", params, cdp_session)
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
                cdp_session: None,
                stage: EvalStage::ResolveRef { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    } else {
        let (context_id, cdp_session) = match att
            .frames
            .get(&host_id)
            .map(|f| f.resolve_eval_target(frame_arg))
            .transpose()
        {
            Ok(Some(v)) => v,
            Ok(None) if frame_arg.is_some() => {
                return StepOutcome::Done(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Frame ID was not found in this tab. Call browser.frames({tabID}) first.",
                ));
            }
            Ok(None) => (None, None),
            Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
        };
        if frame_arg.is_some() && context_id.is_none() && cdp_session.is_none() {
            return StepOutcome::Done(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Frame context is unavailable. Call browser.frames({tabID}) and use a current frameID.",
            ));
        }
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
        match att
            .frames
            .get(&host_id)
            .map(|f| f.resolve_eval_target(frame_arg))
            .transpose()
        {
            Ok(Some(v)) => v,
            Ok(None) if frame_arg.is_some() => {
                return StepOutcome::Done(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Frame ID was not found in this tab. Call browser.frames({tabID}) first.",
                ));
            }
            Ok(None) => (None, None),
            Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
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
            let _ = app.workspace_panes.browser.host.send_cdp(
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
    // Check deadline BEFORE taking any pending CDP response so a stalled page JS loop can NEVER
    // hang past `timeoutMs` or spin the UI thread at 100% CPU.
    if now >= st.deadline {
        st.cancel_cleanup(app);
        return Some(Outcome::fail(
            failure_code::CONDITION_UNMET,
            format!("Wait condition '{}' did not hold before timeout.", st.condition),
        ));
    }
    if let Some(req) = st.cdp_req {
        let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req) else {
            // Keep `next_poll` ahead of `now` while awaiting CDP so `wake_after` does not spin.
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
