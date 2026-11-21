//! JavaScript dialog (`alert`, `confirm`, `prompt`, `beforeunload`) executor for
//! `experimental.browser` v4 using WS-1 `#268`'s `DialogGate` (`BrowserHost::answer_dialog`)
//! with a legacy CDP fallback when `Ready.prompts == false`.

use browser_pane::DialogKind;
use opencode_kit::browser_rpc::{Command, Outcome, failure_code};
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::{ExecPhase, StepOutcome, cdp_err_msg, check_page_guards};
use crate::App;

pub(super) struct DialogState {
    pub host_id: u64,
    pub cdp_req: u64,
}

pub(super) fn try_start(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
) -> Option<StepOutcome> {
    if cmd.action_type()? != "dialog" {
        return None;
    }
    Some(start_dialog(att, app, cmd))
}

fn start_dialog(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "dialog") {
        return StepOutcome::Done(o);
    }
    // The agent acting on a tab takes it over, a dialog included (official `pane.ts` `acquire`):
    // passkeys, file choosers and permissions are restricted from here on.
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let action = cmd.action.get("action").and_then(Value::as_str).unwrap_or("get");
    let prompt_text = cmd.action.get("promptText").and_then(Value::as_str).unwrap_or("");
    let engine_prompts = app.workspace_panes.browser.host.engine_prompts();

    if action == "get" {
        let dialog = open_dialog_json(app, host_id);
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
        };
        return StepOutcome::Done(Outcome::ok(json!({ "tab": tab, "dialog": dialog }), Vec::new()));
    }

    let accept = match action {
        "accept" => true,
        "dismiss" => false,
        _ => {
            return StepOutcome::Done(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "browser.dialog action must be 'get', 'accept', or 'dismiss'.",
            ));
        }
    };
    if engine_prompts {
        // Atomic first-answer-wins on UI thread via WS-1 `#268` DialogGate::answer.
        let open_id = app
            .workspace_panes
            .browser
            .host
            .tab(host_id)
            .and_then(|t| t.prompts.dialog.open().map(|d| d.id));
        let Some(dialog_id) = open_id else {
            return StepOutcome::Done(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "This tab has no JavaScript dialog to handle. browser.dialog({tabID,action:\"get\"}) returns null when none is open.",
            ));
        };
        app.workspace_panes
            .browser
            .host
            .answer_dialog(host_id, dialog_id, accept, prompt_text.to_owned(), false);
        let next_dialog = open_dialog_json(app, host_id);
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
        };
        return StepOutcome::Done(Outcome::ok(json!({ "tab": tab, "dialog": next_dialog }), Vec::new()));
    }

    let mut params = json!({ "accept": accept });
    if accept && !prompt_text.is_empty() {
        params["promptText"] = json!(prompt_text);
    }
    match app
        .workspace_panes
        .browser
        .host
        .send_cdp(host_id, "Page.handleJavaScriptDialog", params, None)
    {
        Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Dialog(DialogState { host_id, cdp_req })),
        Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    }
}

pub(super) fn step_dialog(
    att: &SessionAttachment,
    app: &mut App,
    st: &DialogState,
) -> Option<Outcome> {
    let resp = app.workspace_panes.browser.host.take_cdp_response(st.cdp_req)?;
    match resp {
        Ok(_) => {
            let tab = att.wire_tab(app, st.host_id)?;
            Some(Outcome::ok(json!({ "tab": tab, "dialog": null }), Vec::new()))
        }
        Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e))),
    }
}

fn open_dialog_json(app: &App, host_id: u64) -> Value {
    let Some(d) = app
        .workspace_panes
        .browser
        .host
        .tab(host_id)
        .and_then(|t| t.prompts.dialog.open())
    else {
        return Value::Null;
    };
    let kind = match d.kind {
        DialogKind::Alert => "alert",
        DialogKind::Confirm => "confirm",
        DialogKind::Prompt => "prompt",
        DialogKind::Leave => "beforeunload",
    };
    json!({
        "type": kind,
        "message": d.message,
        "defaultValue": d.default_text,
    })
}
