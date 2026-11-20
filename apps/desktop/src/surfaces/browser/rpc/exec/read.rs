//! Target inspection (`inspect: true`), JavaScript `evaluate`, and `wait` executors.

use std::time::Duration;

use browser_pane::policy::{extension_origin, extension_page_refusal, matches_url};
use opencode_kit::browser_rpc::{Command, MAX_WAIT_MS, Outcome, TargetMeta, failure_code};
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::{ExecPhase, StepOutcome, cdp_err_msg, check_page_guards};
use crate::App;

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(120);

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
    match cmd.action_type()? {
        "evaluate" => Some(start_evaluate(att, app, cmd)),
        "wait" => Some(start_wait(att, app, cmd, now)),
        _ => None,
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
