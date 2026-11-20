//! Navigation, tab-lifecycle, and workspace preview executors for `experimental.browser` v4
//! (`tabs.list`, `tabs.open`, `tabs.focus`, `tabs.close`, `navigate`, `back`, `forward`,
//! `reload`, `stop`, `preview`, `navigate.path`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use browser_pane::policy::{
    extension_origin, extension_page_refusal, file_url_within, local_file_path, refusal, with_scheme,
};
use opencode_kit::browser_rpc::{Command, Outcome, failure_code};
use rune::Tx;
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::{ExecPhase, StepOutcome, check_page_guards};
use crate::App;

const LOAD_TIMEOUT: Duration = Duration::from_secs(25);
const LOAD_SETTLE_GRACE: Duration = Duration::from_millis(1_200);

/// State carried while waiting for a tab navigation/load to settle.
pub(super) struct WaitLoadState {
    pub host_id: u64,
    pub wire_tab_id: String,
    pub preview_path: Option<String>,
    pub close_on_fail: bool,
    pub saw_loading: bool,
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

fn start_tabs_open(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    now: Instant,
) -> StepOutcome {
    let raw_url = cmd.action.get("url").and_then(Value::as_str);
    let focus = cmd.action.get("focus").and_then(Value::as_bool).unwrap_or(true);
    let target_url = if let Some(u) = raw_url {
        let trimmed = u.trim();
        if extension_origin(trimmed) {
            return StepOutcome::Done(Outcome::fail(
                failure_code::NAVIGATION_FAILED,
                extension_page_refusal(trimmed),
            ));
        }
        let candidate = with_scheme(trimmed);
        let roots = app.browser_file_roots(Some(&att.session_id));
        if let Some(why) = refusal(&candidate, &roots) {
            return StepOutcome::Done(Outcome::fail(
                failure_code::NAVIGATION_FAILED,
                format!("Navigation refused ({}): {trimmed}", why.code()),
            ));
        }
        candidate
    } else {
        "about:blank".to_owned()
    };

    let host_id = app.open_new_browser_tab_for_session(Some(&att.session_id), &target_url, None, focus);
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
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
        prev_rejected,
        started_at: now,
        deadline: now + LOAD_TIMEOUT,
    }))
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
    let trimmed = raw_url.trim();
    if extension_origin(trimmed) {
        return StepOutcome::Done(Outcome::fail(
            failure_code::NAVIGATION_FAILED,
            extension_page_refusal(trimmed),
        ));
    }
    let candidate = with_scheme(trimmed);
    let roots = app.browser_file_roots(Some(&att.session_id));
    if let Some(why) = refusal(&candidate, &roots) {
        return StepOutcome::Done(Outcome::fail(
            failure_code::NAVIGATION_FAILED,
            format!("Navigation refused ({}): {trimmed}", why.code()),
        ));
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
    app.navigate_browser_tab(host_id, &candidate);
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
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
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
    let file_url = match resolve_workspace_preview(att, app, path) {
        Ok(u) => u,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::PREVIEW_UNAVAILABLE, msg)),
    };
    // Reuse existing tab showing the same file in this session, matching `pane.ts:1053`.
    let existing = att.host_to_wire.iter().find_map(|(&hid, wid)| {
        let t = app.workspace_panes.browser.host.tab(hid)?;
        let same = t.url.split(['?', '#']).next() == Some(file_url.as_str());
        same.then(|| (hid, wid.clone()))
    });
    let (host_id, wire_tab_id, close_on_fail) = if let Some((hid, wid)) = existing {
        app.navigate_browser_tab(hid, &file_url);
        (hid, wid, false)
    } else {
        let hid = app.open_new_browser_tab_for_session(Some(&att.session_id), &file_url, None, true);
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
    let file_url = match resolve_workspace_preview(att, app, path) {
        Ok(u) => u,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::PREVIEW_UNAVAILABLE, msg)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let prev_rejected = app.workspace_panes.browser.host.tab(host_id).map_or(0, |t| t.rejected);
    app.navigate_browser_tab(host_id, &file_url);
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
                let sid = att.session_id.clone();
                app.close_browser_tab_for_session(&sid, st.host_id, tx);
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
            let sid = att.session_id.clone();
            app.close_browser_tab_for_session(&sid, st.host_id, tx);
        }
        return Some(Outcome::fail(
            failure_code::NAVIGATION_FAILED,
            format!("Navigation was refused by browser URL policy ({reason})."),
        ));
    }
    if tab.loading {
        st.saw_loading = true;
    }
    let settled = (!tab.loading && (st.saw_loading || now >= st.started_at + LOAD_SETTLE_GRACE))
        || now >= st.deadline;
    if !settled {
        return None;
    }
    if let Some(err) = &tab.load_error {
        let msg = format!("Page failed to load ({err}).");
        if st.close_on_fail {
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

fn resolve_workspace_preview(att: &SessionAttachment, app: &App, raw_path: &str) -> Result<String, String> {
    let roots = app.browser_file_roots(Some(&att.session_id));
    let Some(root) = roots.first() else {
        return Err("Workspace preview requires a local session workspace root.".into());
    };
    let abs = if raw_path.starts_with("file:") {
        PathBuf::from(
            local_file_path(raw_path).ok_or_else(|| "Invalid workspace file URL.".to_owned())?,
        )
    } else {
        let p = Path::new(raw_path);
        if p.is_absolute() { p.to_path_buf() } else { PathBuf::from(root).join(p) }
    };
    let url = path_to_file_url(&abs);
    if !file_url_within(&url, &roots) {
        return Err("The workspace file is outside this session's workspace or cannot be read.".into());
    }
    if !abs.is_file() {
        return Err("The workspace file no longer exists or cannot be read by this session.".into());
    }
    Ok(url)
}

fn path_to_file_url(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let prefixed = if raw.starts_with('/') { raw } else { format!("/{raw}") };
    let encoded: String = prefixed
        .split('/')
        .map(|seg| {
            if seg.len() == 2 && seg.as_bytes()[1] == b':' {
                seg.to_owned()
            } else {
                seg.bytes()
                    .map(|b| match b {
                        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                            (b as char).to_string()
                        }
                        _ => format!("%{b:02X}"),
                    })
                    .collect()
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("file://{encoded}")
}
