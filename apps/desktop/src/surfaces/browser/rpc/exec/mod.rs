//! Explicit non-blocking command executor state machine and per-tab safeguard guards
//! (dialogs, HTTP auth, extension-origin isolation, cancellation).

pub(super) mod capture;
pub(super) mod nav;
pub(super) mod read;
pub(super) mod snapshot;

use browser_pane::policy::{extension_origin, extension_page_refusal};
use opencode_kit::browser_rpc::{AttachEvent, Command, Outcome, failure_code};
use rune::Tx;
use rune::time::Instant;
use serde_json::Value;

use self::capture::ScreenshotState;
use self::nav::WaitLoadState;
use self::read::{EvalState, WaitState};
use self::snapshot::{FramesState, SnapshotState};
use super::session::SessionAttachment;
use crate::App;

/// Explicit non-blocking execution state of one in-flight `experimental.browser` command.
enum ExecPhase {
    WaitLoad(WaitLoadState),
    Frames(FramesState),
    Snapshot(SnapshotState),
    Eval(EvalState),
    Wait(WaitState),
    Screenshot(ScreenshotState),
}

impl ExecPhase {
    pub fn target_host_tab(&self) -> u64 {
        match self {
            Self::WaitLoad(s) => s.host_id,
            Self::Frames(s) => s.host_id,
            Self::Snapshot(s) => s.host_id,
            Self::Eval(s) => s.host_id,
            Self::Wait(s) => s.host_id,
            Self::Screenshot(s) => s.host_id,
        }
    }

    pub fn next_wake(&self) -> Option<Instant> {
        match self {
            Self::WaitLoad(s) => {
                if s.saw_loading {
                    Some(s.deadline)
                } else {
                    Some((s.started_at + std::time::Duration::from_millis(1_200)).min(s.deadline))
                }
            }
            Self::Wait(s) => Some(s.next_poll.min(s.deadline)),
            _ => None,
        }
    }
}

enum StepOutcome {
    Done(Outcome),
    Pending(ExecPhase),
}

pub(super) struct ActiveCommand {
    pub connection_id: String,
    pub request_id: String,
    pub is_dialog: bool,
    phase: ExecPhase,
}

/// Checks non-negotiable per-tab guards before running an agent page action:
/// 1. Refuses extension pages (`chrome-extension://`, etc.) except leaving navigations.
/// 2. Refuses when an HTTP 401 auth challenge requires human sign-in (`sign_in_required`).
/// 3. Refuses when a JS dialog blocks the tab (`[browser.dialog_open]`), except for `dialog`.
pub(super) fn check_page_guards(app: &App, host_id: u64, action_type: &str) -> Result<(), Outcome> {
    let Some(tab) = app.workspace_panes.browser.host.tab(host_id) else {
        return Err(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Browser tab is unavailable.",
        ));
    };
    let leaving = matches!(action_type, "navigate" | "back" | "forward" | "stop");
    if !leaving && extension_origin(&tab.url) {
        return Err(Outcome::fail(
            failure_code::OPERATION_FAILED,
            extension_page_refusal(&tab.url),
        ));
    }
    if action_type != "dialog" && tab.prompts.dialog.open().is_some() {
        return Err(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "[browser.dialog_open] A JavaScript dialog is open in this tab. Inspect or answer it with browser.dialog before continuing.",
        ));
    }
    if !leaving && let Some(origin) = &tab.prompts.agent_auth_required {
        return Err(Outcome::fail(
            failure_code::SIGN_IN_REQUIRED,
            format!("The site {origin} requires HTTP sign-in from the user in the browser pane."),
        ));
    }
    Ok(())
}

/// Starts one fetched `experimental.browser` command for `att`.
/// Immediately replies on synchronous actions or registers a non-blocking [`ActiveCommand`].
pub(super) fn start_command(
    att: &mut SessionAttachment,
    app: &mut App,
    connection_id: String,
    request_id: String,
    cmd: Command,
    now: Instant,
    tx: &mut Tx,
) {
    if !att.attach.is_request_in_flight(&connection_id, &request_id) {
        return;
    }
    if cmd.is_inspect() {
        let outcome = read::inspect_target(att, app, &cmd);
        finish_and_reply(att, app, &connection_id, request_id, outcome, now);
        return;
    }
    let is_dialog = cmd.action_type() == Some("dialog");
    let step = nav::try_start(att, app, &cmd, now, tx)
        .or_else(|| snapshot::try_start(att, app, &cmd))
        .or_else(|| read::try_start(att, app, &cmd, now))
        .or_else(|| capture::try_start(att, app, &cmd))
        .unwrap_or_else(|| {
            let name = cmd.action_type().unwrap_or("unknown");
            StepOutcome::Done(Outcome::fail(
                failure_code::UNSUPPORTED,
                format!("browser.{name} is not supported yet by this desktop build."),
            ))
        });
    match step {
        StepOutcome::Done(outcome) => {
            finish_and_reply(att, app, &connection_id, request_id, outcome, now);
        }
        StepOutcome::Pending(phase) => {
            att.active_commands.push(ActiveCommand {
                connection_id,
                request_id,
                is_dialog,
                phase,
            });
        }
    }
}

/// Advances all in-flight non-blocking commands for `att`. Returns `(changed, earliest_wake)`.
pub(super) fn poll_commands(
    att: &mut SessionAttachment,
    app: &mut App,
    now: Instant,
    ref_counter: &mut u64,
    tx: &mut Tx,
) -> (bool, Option<Instant>) {
    let mut changed = false;
    let mut earliest: Option<Instant> = None;
    let mut cmds = std::mem::take(&mut att.active_commands);
    let mut still_running = Vec::with_capacity(cmds.len());

    for mut cmd in cmds.drain(..) {
        // Hard cancellation invariant: never post a late result after cancel or disconnect.
        if !att.attach.is_request_in_flight(&cmd.connection_id, &cmd.request_id) {
            changed = true;
            continue;
        }
        let host_id = cmd.phase.target_host_tab();
        let mid_guard_fail = if !cmd.is_dialog {
            if let Some(tab) = app.workspace_panes.browser.host.tab(host_id) {
                if tab.prompts.dialog.open().is_some() {
                    Some(Outcome::fail(
                        failure_code::OPERATION_FAILED,
                        "[browser.dialog_interrupted] A JavaScript dialog opened while the action was running. Answer or dismiss it with browser.dialog before continuing.",
                    ))
                } else {
                    tab.prompts.agent_auth_required.as_ref().map(|origin| {
                        Outcome::fail(
                            failure_code::SIGN_IN_REQUIRED,
                            format!("The site {origin} requires HTTP sign-in from the user in the browser pane."),
                        )
                    })
                }
            } else {
                Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Browser tab closed while the action was running.",
                ))
            }
        } else {
            None
        };

        let done = if let Some(fail) = mid_guard_fail {
            Some(fail)
        } else {
            match &mut cmd.phase {
                ExecPhase::WaitLoad(st) => nav::step_wait_load(att, app, st, now, tx),
                ExecPhase::Frames(st) => snapshot::step_frames(att, app, st),
                ExecPhase::Snapshot(st) => snapshot::step_snapshot(att, app, st, ref_counter),
                ExecPhase::Eval(st) => read::step_eval(att, app, st),
                ExecPhase::Wait(st) => read::step_wait(att, app, st, now),
                ExecPhase::Screenshot(st) => capture::step_screenshot(att, app, st),
            }
        };

        if let Some(outcome) = done {
            finish_and_reply(att, app, &cmd.connection_id, cmd.request_id, outcome, now);
            changed = true;
        } else {
            if let Some(w) = cmd.phase.next_wake() {
                earliest = Some(match earliest {
                    Some(cur) => cur.min(w),
                    None => w,
                });
            }
            still_running.push(cmd);
        }
    }

    att.active_commands = still_running;
    (changed, earliest)
}

fn finish_and_reply(
    att: &mut SessionAttachment,
    app: &App,
    connection_id: &str,
    request_id: String,
    outcome: Outcome,
    now: Instant,
) {
    if !att.attach.is_request_in_flight(connection_id, &request_id) {
        return;
    }
    // Sync tab state BEFORE posting the result on the ordered FIFO queue so the server knows
    // every tabID / generation named by the result.
    let (state, hash) = att.sync_tabs(app);
    let _ = att.attach.step(AttachEvent::StateChanged { hash }, now);
    att.enqueue_state(connection_id, state, hash);
    att.enqueue_result(connection_id, request_id.clone(), outcome);
    let _ = att.attach.step(AttachEvent::CommandFinished { request_id }, now);
}

pub(super) fn cdp_err_msg(err: &Value) -> String {
    err.get("message")
        .and_then(Value::as_str)
        .or_else(|| err.get("error").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| err.to_string())
}
