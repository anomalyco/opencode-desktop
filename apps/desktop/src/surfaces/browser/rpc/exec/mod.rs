//! Explicit non-blocking command executor state machine and per-tab safeguard guards
//! (server authorization target/generation checks, live inspect refresh, local deadlines,
//! dialogs, HTTP auth, extension-origin isolation, and clean cancellation teardown).

pub(super) mod capture;
pub(super) mod nav;
pub(super) mod read;
pub(super) mod snapshot;
mod target_url;

use std::time::Duration;

use browser_pane::policy::{extension_origin, extension_page_refusal};
use opencode_kit::browser_rpc::{AttachEvent, Command, Outcome, TargetMeta, failure_code};
use rune::time::Instant;
use rune::{Tx, Waker};
use serde_json::Value;

use self::capture::ScreenshotState;
use self::nav::WaitLoadState;
use self::read::{EvalState, InspectState, WaitState};
use self::snapshot::{FramesState, SnapshotState};
use super::session::SessionAttachment;
use crate::App;

const DEFAULT_CMD_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_WAIT_CMD_TIMEOUT: Duration = Duration::from_secs(31);

/// Explicit non-blocking execution state of one in-flight `experimental.browser` command.
enum ExecPhase {
    Inspect(InspectState),
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
            Self::Inspect(s) => s.host_id,
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
                    Some((s.started_at + Duration::from_millis(1_200)).min(s.deadline))
                }
            }
            Self::Wait(s) => Some(s.next_poll.min(s.deadline)),
            _ => None,
        }
    }

    pub fn cancel_cleanup(
        &mut self,
        app: &mut App,
        session_id: &str,
        tx: Option<&mut Tx>,
        pending_close_tabs: &mut Vec<u64>,
    ) {
        match self {
            Self::Inspect(st) => {
                for (req_id, _) in st.pending.drain(..) {
                    app.workspace_panes.browser.host.cancel_cdp(req_id);
                }
            }
            Self::WaitLoad(st) => {
                app.workspace_panes.browser.host.stop(st.host_id);
                if st.close_on_fail {
                    if let Some(tx) = tx {
                        app.close_browser_tab_for_session(session_id, st.host_id, tx);
                    } else {
                        // Queue full session+UI close for the next `poll` tick so `session_tabs`
                        // and the side-panel tab never leak a ghost entry.
                        pending_close_tabs.push(st.host_id);
                    }
                }
            }
            Self::Frames(st) => {
                app.workspace_panes.browser.host.cancel_cdp(st.cdp_req);
            }
            Self::Snapshot(st) => st.cancel_cleanup(app),
            Self::Eval(st) => st.cancel_cleanup(app),
            Self::Wait(st) => st.cancel_cleanup(app),
            Self::Screenshot(st) => st.cancel_cleanup(app),
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
    pub deadline: Instant,
    phase: ExecPhase,
}

impl ActiveCommand {
    pub fn cancel_cleanup(
        &mut self,
        app: &mut App,
        session_id: &str,
        tx: Option<&mut Tx>,
        pending_close_tabs: &mut Vec<u64>,
    ) {
        self.phase
            .cancel_cleanup(app, session_id, tx, pending_close_tabs);
    }
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

/// Verifies server-authorized `command.generation` and `command.target` (`chromium.ts:1518-1531`).
/// Called both right before execution AND right before returning async capture/snapshot outputs.
pub(super) fn verify_authorized_state(
    att: &SessionAttachment,
    app: &App,
    cmd: &Command,
    expected_gen: Option<u64>,
    expected_target: Option<&TargetMeta>,
) -> Result<(), Outcome> {
    if let Some(want_gen) = expected_gen {
        let Some(wire_id) = cmd.tab_id() else {
            return Err(Outcome::fail(
                failure_code::DOCUMENT_CHANGED,
                "Missing tabID for generation check.",
            ));
        };
        let host_id = att
            .resolve_tab(app, wire_id)
            .map_err(|msg| Outcome::fail(failure_code::OPERATION_FAILED, msg))?;
        if att.generation_of(host_id) != want_gen {
            return Err(Outcome::fail(
                failure_code::DOCUMENT_CHANGED,
                "The tab navigated or changed documents after authorization. Refresh state and retry.",
            ));
        }
    }
    if let Some(want_target) = expected_target {
        let actual = read::compute_target(att, app, cmd)?;
        let mut expected_res = want_target.resources.clone();
        expected_res.sort();
        expected_res.dedup();
        if actual.key != want_target.key || actual.resources != expected_res {
            return Err(Outcome::fail(
                failure_code::DOCUMENT_CHANGED,
                "The page or its frames changed after authorization. Retry so permissions can re-check the current URLs.",
            ));
        }
    }
    Ok(())
}

/// Starts one fetched `experimental.browser` command for `att`.
pub(super) fn start_command(
    att: &mut SessionAttachment,
    app: &mut App,
    connection_id: String,
    request_id: String,
    cmd: Command,
    now: Instant,
    waker: Option<Waker>,
    tx: &mut Tx,
) {
    if !att.attach.is_request_in_flight(&connection_id, &request_id) {
        return;
    }
    let _ = att.sync_tabs(app);
    if cmd.is_inspect() {
        match read::start_inspect(att, app, cmd) {
            StepOutcome::Done(outcome) => {
                finish_and_reply(att, app, &connection_id, request_id, outcome, now);
            }
            StepOutcome::Pending(phase) => {
                att.active_commands.push(ActiveCommand {
                    connection_id,
                    request_id,
                    is_dialog: true,
                    deadline: now + DEFAULT_CMD_TIMEOUT,
                    phase,
                });
            }
        }
        return;
    }
    if let Err(outcome) = verify_authorized_state(att, app, &cmd, cmd.generation, cmd.target.as_ref()) {
        finish_and_reply(att, app, &connection_id, request_id, outcome, now);
        return;
    }
    let is_dialog = cmd.action_type() == Some("dialog");
    let step = nav::try_start(att, app, &cmd, now, tx)
        .or_else(|| snapshot::try_start(att, app, &cmd))
        .or_else(|| read::try_start(att, app, &cmd, now))
        .or_else(|| capture::try_start(att, app, &cmd, waker))
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
            let deadline = match &phase {
                ExecPhase::Wait(w) => (w.deadline + Duration::from_secs(1)).min(now + MAX_WAIT_CMD_TIMEOUT),
                ExecPhase::WaitLoad(w) => w.deadline,
                _ => now + DEFAULT_CMD_TIMEOUT,
            };
            att.active_commands.push(ActiveCommand {
                connection_id,
                request_id,
                is_dialog,
                deadline,
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
    waker: Option<Waker>,
    tx: &mut Tx,
) -> (bool, Option<Instant>) {
    let mut changed = false;
    let mut earliest: Option<Instant> = None;
    let mut cmds = std::mem::take(&mut att.active_commands);
    let mut still_running = Vec::with_capacity(cmds.len());
    let sid = att.session_id.clone();

    for mut cmd in cmds.drain(..) {
        if !att.attach.is_request_in_flight(&cmd.connection_id, &cmd.request_id) {
            cmd.cancel_cleanup(app, &sid, Some(tx), &mut att.pending_close_tabs);
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
            cmd.cancel_cleanup(app, &sid, Some(tx), &mut att.pending_close_tabs);
            Some(fail)
        } else if now >= cmd.deadline && !matches!(cmd.phase, ExecPhase::Wait(_) | ExecPhase::WaitLoad(_)) {
            cmd.cancel_cleanup(app, &sid, Some(tx), &mut att.pending_close_tabs);
            Some(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Browser command timed out locally.",
            ))
        } else {
            match &mut cmd.phase {
                ExecPhase::Inspect(st) => read::step_inspect(att, app, st),
                ExecPhase::WaitLoad(st) => nav::step_wait_load(att, app, st, now, tx),
                ExecPhase::Frames(st) => snapshot::step_frames(att, app, st),
                ExecPhase::Snapshot(st) => snapshot::step_snapshot(att, app, st, ref_counter),
                ExecPhase::Eval(st) => read::step_eval(att, app, st),
                ExecPhase::Wait(st) => read::step_wait(att, app, st, now),
                ExecPhase::Screenshot(st) => capture::step_screenshot(att, app, st, waker.clone()),
            }
        };

        if let Some(outcome) = done {
            finish_and_reply(att, app, &cmd.connection_id, cmd.request_id, outcome, now);
            changed = true;
        } else {
            let wake = cmd
                .phase
                .next_wake()
                .map_or(cmd.deadline, |w| w.min(cmd.deadline));
            earliest = Some(match earliest {
                Some(cur) => cur.min(wake),
                None => wake,
            });
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

/// Sends a CDP call using `send_typed_cdp` whenever targeting an OOPIF `session_id` (since
/// `send_cdp` strictly rejects `session_id.is_some()` for untrusted callers).
pub(super) fn send_rpc_cdp(
    app: &mut App,
    host_id: u64,
    method: &str,
    params: Value,
    session_id: Option<String>,
) -> Result<u64, String> {
    if session_id.is_some() {
        app.workspace_panes
            .browser
            .host
            .send_typed_cdp(host_id, method, params, session_id)
    } else {
        app.workspace_panes
            .browser
            .host
            .send_cdp(host_id, method, params, None)
    }
}
