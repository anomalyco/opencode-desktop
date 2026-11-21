//! Navigation, tab-lifecycle, and workspace preview executors for `experimental.browser` v4
//! (`tabs.list`, `tabs.open`, `tabs.focus`, `tabs.close`, `navigate`, `back`, `forward`,
//! `reload`, `stop`, `preview`, `navigate.path`).

use std::path::Path;
use std::time::Duration;

use browser_pane::PreviewSpec;
use browser_pane::policy::local_file_path;
use browser_pane::policy::preview::relative_to_root;
use opencode_kit::browser_rpc::{Command, Outcome, failure_code};
use rune::Tx;
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::target_url::parse_agent_url;
use super::{ExecPhase, StepOutcome, check_page_guards};
use crate::App;

const LOAD_TIMEOUT: Duration = Duration::from_secs(15);
const LOAD_SETTLE_GRACE: Duration = Duration::from_millis(1_200);

/// State carried while waiting for a tab navigation/load to settle.
pub(super) struct WaitLoadState {
    pub host_id: u64,
    pub wire_tab_id: String,
    pub preview_path: Option<String>,
    pub close_on_fail: bool,
    pub saw_loading: bool,
    /// Back/forward: the page as it was before (URL, document, history ends). A history entry
    /// restored from the back/forward cache commits without ever reporting loading, so its
    /// commit (any of these changing) settles the wait instead of LOAD_SETTLE_GRACE.
    pub history_from: Option<(String, u64, bool, bool)>,
    pub prev_rejected: u64,
    pub started_at: Instant,
    pub deadline: Instant,
}

/// Dispatches navigation and tab-lifecycle actions. Returns `None` if the action belongs to
/// another family.
pub(super) fn try_start(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
    tx: &mut Tx,
) -> Option<StepOutcome> {
    let ty = cmd.action_type()?;
    match ty {
        "tabs.list" => {
            let (state, _) = att.sync_tabs(app);
            Some(StepOutcome::Done(Outcome::ok(
                serde_json::to_value(state).unwrap_or(Value::Null),
                Vec::new(),
            )))
        }
        "tabs.open" => Some(start_tabs_open(att, app, cmd, now)),
        "tabs.focus" => Some(exec_tabs_focus(att, app, cmd)),
        "tabs.close" => Some(exec_tabs_close(att, app, cmd, tx)),
        "navigate" => Some(start_navigate(att, app, cmd, now)),
        "back" => Some(start_history(att, app, cmd, now, true)),
        "forward" => Some(start_history(att, app, cmd, now, false)),
        "reload" => Some(start_reload(att, app, cmd, now)),
        "stop" => Some(exec_stop(att, app, cmd)),
        "preview" => Some(start_preview(att, app, cmd, now, tx)),
        "navigate.path" => Some(start_navigate_path(att, app, cmd, now)),
        _ => None,
    }
}

fn require_remote_proxy_if_remote(att: &SessionAttachment, app: &App) -> Result<(), Outcome> {
    let same_machine = att.server_key.is_sidecar() && att.server.is_local();
    if same_machine {
        return Ok(());
    }
    if app
        .workspace_panes
        .browser
        .host
        .remote_proxy(att.server_key.as_str())
        .is_none()
    {
        return Err(Outcome::fail(
            failure_code::NAVIGATION_FAILED,
            "Remote-server browser navigation refused (browser.proxy.required): no live tunnel proxy is registered for this server.",
        ));
    }
    Ok(())
}

fn start_tabs_open(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
) -> StepOutcome {
    if let Err(o) = require_remote_proxy_if_remote(att, app) {
        return StepOutcome::Done(o);
    }
    let raw_url = cmd
        .action
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|u| !u.is_empty());
    let focus = cmd.action.get("focus").and_then(Value::as_bool).unwrap_or(true);
    let target_url = if let Some(u) = raw_url {
        let parsed = match parse_agent_url(u) {
            Ok(parsed) => parsed,
            Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::NAVIGATION_FAILED, msg)),
        };
        match app.normalize_browser_url_for_session(Some(&att.session_id), &parsed, true) {
            Ok(normalized) => normalized,
            Err(err) => {
                return StepOutcome::Done(Outcome::fail(failure_code::NAVIGATION_FAILED, err));
            }
        }
    } else {
        "about:blank".to_owned()
    };

    let sid = att.session_id.clone();
    let open = |app: &mut App| match app.resolve_preview_target(Some(&sid), &target_url, true) {
        crate::browser::preview::PreviewTarget::Preview { display_url, spec } => Some(app.open_resolved_preview_tab(
            Some(&sid),
            &display_url,
            spec,
            None,
            focus,
            Some(browser_pane::TabDriver::Agent),
        )),
        crate::browser::preview::PreviewTarget::Refused(_) => None,
        crate::browser::preview::PreviewTarget::NotPreview => {
            Some(app.open_new_agent_browser_tab_for_session(Some(&sid), &target_url, None, focus))
        }
    };
    let opened = if focus { open(app) } else { open_in_background(app, &sid, open) };
    let Some(host_id) = opened else {
        return StepOutcome::Done(Outcome::fail(
            failure_code::NAVIGATION_FAILED,
            "The workspace file is outside this session's workspace or cannot be read.",
        ));
    };
    att.ensure_tab_cdp(app, host_id);
    let _ = att.sync_tabs(app);
    let Some(wire_tab_id) = att.host_to_wire.get(&host_id).cloned() else {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Opened browser tab was not registered in this session.",
        ));
    };
    if focus {
        att.agent_tab_id = Some(wire_tab_id.clone());
    }
    if target_url == "about:blank" || app.use_mock_browser() {
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab disappeared."));
        };
        return StepOutcome::Done(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()));
    }
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
    StepOutcome::Pending(ExecPhase::WaitLoad(WaitLoadState {
        host_id,
        wire_tab_id,
        preview_path: None,
        close_on_fail: true,
        saw_loading: app.workspace_panes.browser.host.tab(host_id).is_some_and(|t| t.loading),
        history_from: None,
        prev_rejected,
        started_at: now,
        deadline: now + LOAD_TIMEOUT,
    }))
}

/// `tabs.open` with `focus: false` keeps the new tab in the background (`agentTabShow`,
/// `packages/gui-plugins/src/browser/policy.ts:146-150` at `b9a60d9d`): it joins the session's
/// strip, while the pane's open state, the tab it shows and the focused page stay as they were.
fn open_in_background(app: &mut App, sid: &str, open: impl FnOnce(&mut App) -> Option<u64>) -> Option<u64> {
    let focused = app.workspace_panes.browser.host.focused_tab_id();
    let (before, shared_open, pane_focus) = {
        let side = app.kernel.slots.get(&crate::panels::SIDE);
        (side.sessions.get(sid).cloned(), side.shared_open, side.active_pane_focus)
    };
    // `focus: false` already opens it keeping the keyboard (`attach_browser_tab_to_session`).
    let id = open(app)?;
    let key = crate::panels::panel_key(crate::surfaces::browser::PLUGIN, &id.to_string());
    let side = app.kernel.slots.get_mut(&crate::panels::SIDE);
    side.shared_open = shared_open;
    side.active_pane_focus = pane_focus;
    let mut kept = before.unwrap_or_default();
    if !kept.tab_state.tabs.all.contains(&key) {
        kept.tab_state.tabs.all.push(key);
    }
    side.sessions.insert(sid.to_owned(), kept);
    if let Some(prev) = focused.filter(|prev| *prev != id) {
        app.workspace_panes.browser.host.focus_tab(prev);
    }
    app.mark_browser_tab_unseen(id);
    Some(id)
}

fn exec_tabs_focus(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    app.workspace_panes.browser.host.focus_tab(host_id);
    let key = crate::panels::panel_key(crate::surfaces::browser::PLUGIN, &host_id.to_string());
    let sid = att.session_id.clone();
    app.keeping_keyboard(|s| s.open_side_tab_for_session(&sid, &key, None, false, false));
    att.agent_tab_id = Some(wire_id.to_owned());
    let _ = att.sync_tabs(app);
    let Some(tab) = att.wire_tab(app, host_id) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    StepOutcome::Done(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()))
}

fn exec_tabs_close(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    tx: &mut Tx,
) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    let sid = att.session_id.clone();
    app.close_browser_tab_for_session(&sid, host_id, tx);
    let (state, _) = att.sync_tabs(app);
    StepOutcome::Done(Outcome::ok(serde_json::to_value(state).unwrap_or(Value::Null), Vec::new()))
}

fn start_navigate(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
) -> StepOutcome {
    if let Err(o) = require_remote_proxy_if_remote(att, app) {
        return StepOutcome::Done(o);
    }
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let Some(raw_url) = cmd.action.get("url").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing url."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(outcome) = check_page_guards(app, host_id, "navigate") {
        return StepOutcome::Done(outcome);
    }
    let parsed = match parse_agent_url(raw_url) {
        Ok(parsed) => parsed,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::NAVIGATION_FAILED, msg)),
    };
    let candidate = match app.normalize_browser_url_for_session(Some(&att.session_id), &parsed, true) {
        Ok(normalized) => normalized,
        Err(err) => {
            return StepOutcome::Done(Outcome::fail(failure_code::NAVIGATION_FAILED, err));
        }
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    app.mark_browser_tab_unseen(host_id);
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
    match app.resolve_preview_target(Some(&att.session_id), &candidate, true) {
        crate::browser::preview::PreviewTarget::Preview { display_url, spec } => {
            app.reopen_resolved_preview_tab(host_id, Some(&att.session_id), &display_url, spec);
        }
        crate::browser::preview::PreviewTarget::Refused(_) => {
            return StepOutcome::Done(Outcome::fail(
                failure_code::NAVIGATION_FAILED,
                "The workspace file is outside this session's workspace or cannot be read.",
            ));
        }
        crate::browser::preview::PreviewTarget::NotPreview => {
            app.navigate_browser_tab(host_id, &candidate);
        }
    }
    if app.use_mock_browser() {
        let _ = att.sync_tabs(app);
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
        };
        return StepOutcome::Done(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()));
    }
    StepOutcome::Pending(ExecPhase::WaitLoad(WaitLoadState {
        host_id,
        wire_tab_id: wire_id.to_owned(),
        preview_path: None,
        close_on_fail: false,
        saw_loading: app.workspace_panes.browser.host.tab(host_id).is_some_and(|t| t.loading),
        history_from: None,
        prev_rejected,
        started_at: now,
        deadline: now + LOAD_TIMEOUT,
    }))
}

fn start_history(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
    is_back: bool,
) -> StepOutcome {
    let action_name = if is_back { "back" } else { "forward" };
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(outcome) = check_page_guards(app, host_id, action_name) {
        return StepOutcome::Done(outcome);
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    app.mark_browser_tab_unseen(host_id);
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
    let history_from = app
        .workspace_panes
        .browser
        .host
        .tab(host_id)
        .map(|t| (t.url.clone(), t.prompts.document, t.can_go_back, t.can_go_forward));
    if is_back {
        app.workspace_panes.browser.host.back(host_id);
    } else {
        app.workspace_panes.browser.host.forward(host_id);
    }
    if app.use_mock_browser() {
        let _ = att.sync_tabs(app);
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
        };
        return StepOutcome::Done(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()));
    }
    StepOutcome::Pending(ExecPhase::WaitLoad(WaitLoadState {
        host_id,
        wire_tab_id: wire_id.to_owned(),
        preview_path: None,
        close_on_fail: false,
        saw_loading: false,
        history_from,
        prev_rejected,
        started_at: now,
        deadline: now + LOAD_TIMEOUT,
    }))
}

fn start_reload(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let ignore_cache = cmd.action.get("ignoreCache").and_then(Value::as_bool).unwrap_or(false);
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(outcome) = check_page_guards(app, host_id, "reload") {
        return StepOutcome::Done(outcome);
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    app.mark_browser_tab_unseen(host_id);
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
    if ignore_cache {
        app.workspace_panes.browser.host.reload_ignore_cache(host_id);
    } else {
        app.workspace_panes.browser.host.reload(host_id);
    }
    if app.use_mock_browser() {
        let _ = att.sync_tabs(app);
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
        };
        return StepOutcome::Done(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()));
    }
    StepOutcome::Pending(ExecPhase::WaitLoad(WaitLoadState {
        host_id,
        wire_tab_id: wire_id.to_owned(),
        preview_path: None,
        close_on_fail: false,
        saw_loading: false,
        history_from: None,
        prev_rejected,
        started_at: now,
        deadline: now + LOAD_TIMEOUT,
    }))
}

fn exec_stop(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    app.workspace_panes.browser.host.stop(host_id);
    let _ = att.sync_tabs(app);
    let Some(tab) = att.wire_tab(app, host_id) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    StepOutcome::Done(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()))
}

fn start_preview(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
    tx: &mut Tx,
) -> StepOutcome {
    let Some(path) = cmd.action.get("path").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::PREVIEW_UNAVAILABLE, "Missing path."));
    };
    if !is_html_path(path) {
        if app.active_session_id() == Some(att.session_id.as_str()) {
            app.open_workspace_file(path, false, None, tx);
        }
        return StepOutcome::Done(Outcome::ok(json!({ "path": path }), Vec::new()));
    }
    let (display_url, spec) = match resolve_workspace_preview(att, app, path) {
        Ok(v) => v,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::PREVIEW_UNAVAILABLE, msg)),
    };
    // Reuse existing tab showing the same file in this session, matching `pane.ts:1053`.
    let existing = att.host_to_wire.iter().find_map(|(&hid, wid)| {
        let t = app.workspace_panes.browser.host.tab(hid)?;
        let same = t.url.split(['?', '#']).next() == Some(display_url.as_str());
        same.then(|| (hid, wid.clone()))
    });
    let (host_id, wire_tab_id, close_on_fail) = if let Some((hid, wid)) = existing {
        if let Err(o) = check_page_guards(app, hid, "navigate") {
            return StepOutcome::Done(o);
        }
        app.workspace_panes.browser.host.set_agent_driven(hid, true);
        app.reopen_resolved_preview_tab(hid, Some(&att.session_id), &display_url, spec);
        (hid, wid, false)
    } else {
        let hid = app.open_resolved_preview_tab(
            Some(&att.session_id),
            &display_url,
            spec,
            None,
            true,
            Some(browser_pane::TabDriver::Agent),
        );
        att.ensure_tab_cdp(app, hid);
        let _ = att.sync_tabs(app);
        let Some(wid) = att.host_to_wire.get(&hid).cloned() else {
            return StepOutcome::Done(Outcome::fail(
                failure_code::PREVIEW_UNAVAILABLE,
                "Preview tab could not be opened.",
            ));
        };
        (hid, wid, true)
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    att.agent_tab_id = Some(wire_tab_id.clone());
    if app.use_mock_browser() {
        let _ = att.sync_tabs(app);
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::PREVIEW_UNAVAILABLE, "Preview tab unavailable."));
        };
        return StepOutcome::Done(Outcome::ok(
            json!({ "path": path, "tabID": wire_tab_id, "url": tab.url, "tab": tab }),
            Vec::new(),
        ));
    }
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
    StepOutcome::Pending(ExecPhase::WaitLoad(WaitLoadState {
        host_id,
        wire_tab_id,
        preview_path: Some(path.to_owned()),
        close_on_fail,
        saw_loading: false,
        history_from: None,
        prev_rejected,
        started_at: now,
        deadline: now + LOAD_TIMEOUT,
    }))
}

fn start_navigate_path(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let Some(path) = cmd.action.get("path").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::PREVIEW_UNAVAILABLE, "Missing path."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(outcome) = check_page_guards(app, host_id, "navigate") {
        return StepOutcome::Done(outcome);
    }
    let (display_url, spec) = match resolve_workspace_preview(att, app, path) {
        Ok(v) => v,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::PREVIEW_UNAVAILABLE, msg)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
    app.reopen_resolved_preview_tab(host_id, Some(&att.session_id), &display_url, spec);
    att.agent_tab_id = Some(wire_id.to_owned());
    if app.use_mock_browser() {
        let _ = att.sync_tabs(app);
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::PREVIEW_UNAVAILABLE, "Tab unavailable."));
        };
        return StepOutcome::Done(Outcome::ok(
            json!({ "path": path, "tabID": wire_id, "url": tab.url, "tab": tab }),
            Vec::new(),
        ));
    }
    StepOutcome::Pending(ExecPhase::WaitLoad(WaitLoadState {
        host_id,
        wire_tab_id: wire_id.to_owned(),
        preview_path: Some(path.to_owned()),
        close_on_fail: false,
        saw_loading: false,
        history_from: None,
        prev_rejected,
        started_at: now,
        deadline: now + LOAD_TIMEOUT,
    }))
}

pub(super) fn step_wait_load(
    att: &mut SessionAttachment,
    app: &mut App,
    st: &mut WaitLoadState,
    now: Instant,
    tx: &mut Tx,
) -> Option<Outcome> {
    match app.workspace_panes.browser.host.status() {
        browser_pane::EngineStatus::Ready => {}
        browser_pane::EngineStatus::Failed(why) => {
            if st.close_on_fail {
                eprintln!(
                    "browser tab closed: its open failed (session={}, tab={}, reason=engine_failed)",
                    att.session_id, st.host_id
                );
                let sid = att.session_id.clone();
                app.close_browser_tab_for_session(&sid, st.host_id, tx);
            }
            if matches!(why.fault, browser_pane::Fault::NotPublished) {
                return Some(Outcome::fail(
                    failure_code::UNSUPPORTED,
                    "Embedded browser engine is unavailable in this build.",
                ));
            }
            return Some(Outcome::fail(
                failure_code::NAVIGATION_FAILED,
                format!("Browser engine failed to start: {}", why.fault),
            ));
        }
        browser_pane::EngineStatus::Unsupported => {
            return Some(Outcome::fail(
                failure_code::UNSUPPORTED,
                "Embedded browser engine is unavailable in this build.",
            ));
        }
        _ => {
            if now >= st.deadline {
                return Some(Outcome::fail(
                    failure_code::NAVIGATION_FAILED,
                    "Timed out waiting for the browser engine to become ready.",
                ));
            }
            // Don't consume LOAD_SETTLE_GRACE while the CEF engine is still cold-starting.
            st.started_at = now;
            return None;
        }
    }
    if let Some(site) = app.workspace_panes.browser.host.take_agent_passkey_refused(st.host_id) {
        return Some(Outcome::fail(
            failure_code::SIGN_IN_REQUIRED,
            browser_pane::prompts::agent_passkey_refusal_message((!site.is_empty()).then_some(site.as_str())),
        ));
    }
    let Some(tab) = app.workspace_panes.browser.host.tab(st.host_id) else {
        return Some(Outcome::fail(
            failure_code::NAVIGATION_FAILED,
            "Browser tab closed while loading.",
        ));
    };
    if let Some(origin) = &tab.prompts.agent_auth_required {
        let msg = format!(
            "The site {origin} asked for HTTP authentication. Only the user can sign in from the browser pane."
        );
        return Some(Outcome::fail(failure_code::SIGN_IN_REQUIRED, msg));
    }
    if tab.prompts.dialog.open().is_some() {
        return Some(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "[browser.dialog_interrupted] A JavaScript dialog opened while the page was loading. Answer or dismiss it with browser.dialog before continuing.",
        ));
    }
    if tab.rejected > st.prev_rejected {
        let reason = tab.refusal.clone().unwrap_or_else(|| "blocked".into());
        if st.close_on_fail {
            eprintln!(
                "browser tab closed: its open failed (session={}, tab={}, reason={reason})",
                att.session_id, st.host_id
            );
            let sid = att.session_id.clone();
            app.close_browser_tab_for_session(&sid, st.host_id, tx);
        }
        return Some(Outcome::fail(
            failure_code::NAVIGATION_FAILED,
            format!("Navigation was refused by browser URL policy ({reason})."),
        ));
    }
    if tab.loading
        || st.history_from.as_ref().is_some_and(|(url, document, back, forward)| {
            (url, *document, *back, *forward) != (&tab.url, tab.prompts.document, tab.can_go_back, tab.can_go_forward)
        })
    {
        st.saw_loading = true;
    }
    if now >= st.deadline && tab.loading && tab.load_error.is_none() {
        // `UP-147` (`#65313`, `keepsOpenedTab`): a page still loading when the navigation deadline
        // passes keeps its tab open so the person or agent can wait, reload, or close it.
        let _ = att.sync_tabs(app);
        let msg = if st.close_on_fail {
            format!(
                "Navigation timed out after {}s. The tab stays open as {}: use that tabID with browser.wait, browser.reload or browser.tabs.close, and do not open the address again.",
                LOAD_TIMEOUT.as_secs(),
                st.wire_tab_id,
            )
        } else {
            format!("Navigation timed out after {}s. The page may still be loading.", LOAD_TIMEOUT.as_secs())
        };
        return Some(Outcome::fail(failure_code::NAVIGATION_FAILED, msg));
    }
    let settled = (!tab.loading && (st.saw_loading || now >= st.started_at + LOAD_SETTLE_GRACE))
        || now >= st.deadline;
    if !settled {
        return None;
    }
    if let Some(err) = &tab.load_error {
        let msg = format!("Page failed to load ({err}).");
        if st.close_on_fail {
            eprintln!(
                "browser tab closed: its open failed (session={}, tab={}, reason={err})",
                att.session_id, st.host_id
            );
            let sid = att.session_id.clone();
            app.close_browser_tab_for_session(&sid, st.host_id, tx);
        }
        return Some(Outcome::fail(failure_code::NAVIGATION_FAILED, msg));
    }
    let _ = att.sync_tabs(app);
    let Some(wire) = att.wire_tab(app, st.host_id) else {
        return Some(Outcome::fail(failure_code::NAVIGATION_FAILED, "Tab unavailable after load."));
    };
    if let Some(path) = &st.preview_path {
        Some(Outcome::ok(
            json!({ "path": path, "tabID": st.wire_tab_id, "url": wire.url, "tab": wire }),
            Vec::new(),
        ))
    } else {
        Some(Outcome::ok(serde_json::to_value(wire).unwrap_or(Value::Null), Vec::new()))
    }
}

fn is_html_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

fn resolve_workspace_preview(
    att: &SessionAttachment,
    app: &App,
    raw_path: &str,
) -> Result<(String, PreviewSpec), String> {
    if !app.browser_preview_supported() {
        return Err("Previews require an updated browser engine.".into());
    }
    let rel = if raw_path.trim().get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")) {
        let local = local_file_path(raw_path)
            .ok_or_else(|| "Invalid workspace file URL.".to_owned())?;
        let roots = app.browser_file_roots(Some(&att.session_id));
        roots
            .iter()
            .find_map(|r| relative_to_root(&local, r))
            .ok_or_else(|| {
                "The workspace file is outside this session's workspace or cannot be read."
                    .to_owned()
            })?
    } else if Path::new(raw_path).is_absolute() {
        let roots = app.browser_file_roots(Some(&att.session_id));
        let norm = raw_path.replace('\\', "/");
        roots
            .iter()
            .find_map(|r| relative_to_root(&norm, r))
            .ok_or_else(|| {
                "The workspace file is outside this session's workspace or cannot be read."
                    .to_owned()
            })?
    } else {
        raw_path.to_owned()
    };
    // Agent previews strictly refuse hidden/dotfile and secret entry documents (`agent = true`).
    match app.resolve_workspace_file_target(&att.session_id, &rel, true) {
        crate::browser::preview::PreviewTarget::Preview { display_url, spec } => {
            if let PreviewSpec::Local { ref root, ref document } = spec
                && !Path::new(root).join(document).is_file()
            {
                return Err(
                    "The workspace file no longer exists or cannot be read by this session.".into(),
                );
            }
            Ok((display_url, spec))
        }
        _ => Err("The workspace file is outside this session's workspace or cannot be read.".into()),
    }
}
