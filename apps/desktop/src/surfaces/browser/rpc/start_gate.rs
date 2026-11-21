//! Fail-open new-session prompt gate: the write worker waits at most PROMPT_GATE_TIMEOUT_MS between
//! POST /api/session and POST /prompt while the new session attaches (machine.ts:507-509).

use std::sync::mpsc::Sender;

use opencode_kit::browser_rpc::{AttachEvent, LocationRef};
use opencode_kit::servers::ServerKey;
use rune::time::Instant;

use super::session::SessionAttachment;
use super::{BROWSER_RPC, apply_batch_effects, is_base_connected, live_server_clone, session_directory};
use crate::{App, Route};

/// Ephemeral one-shot waiter for a new-session start (`POST /api/session` -> `attach` -> `/prompt`).
///
/// Invariant: no prompt is ever held longer than `PROMPT_GATE_TIMEOUT_MS` (1 s), dropped,
/// duplicated, reordered, or sent to another server regardless of window close, tab switch,
/// server switch, app quit, or panic. Dropping this struct ALWAYS wakes the owning `Server`'s
/// write worker immediately (`self.release.send(())`), and the worker itself bounds the wait
/// with `rx.recv_timeout(1 s)`.
pub(super) struct StartWaiter {
    server_key: ServerKey,
    session_id: String,
    release: Sender<()>,
}

impl Drop for StartWaiter {
    fn drop(&mut self) {
        let _ = self.release.send(());
    }
}

/// Releases all pending new-session prompt-gate waiters immediately (called on window close so
/// closing a window mid-hold wakes the worker thread in 0 ms to post the prompt on its own server).
pub fn release_start_waiters(app: &mut App) {
    app.kernel.slots.get_mut(&BROWSER_RPC).start_waiters.clear();
}

/// Handles `Incoming::PromptGate` (new-session start only): begins `experimental.browser` attach
/// right after `POST /api/session` succeeds and holds `release` until attach settles (`<= 1 s`
/// enforced fail-open inside the worker thread). Releases in 0 ms when the window is not showing
/// this server, SSE is down, or attach is already settled (`Attached`/`Backoff`/`Unsupported`/
/// `Unavailable`/`Blocked`).
pub fn hold_start_gate(app: &mut App, session_id: &str, directory: &str, release: Sender<()>) {
    if !app.live() || app.browser_unsupported() || session_id.is_empty() {
        let _ = release.send(());
        return;
    }
    let Some(server) = live_server_clone(app) else {
        let _ = release.send(());
        return;
    };
    let active_server = crate::servers::runtime::active(app).clone();
    // `show_start` promotes the draft's tab to the new session and routes to it, so the start is
    // on this window when its tab (or route) is still there on the active server.
    let on_this_window = matches!(&*app.route, Route::Session { server: rk, id } if *rk == active_server && id == session_id)
        || app.tabs.iter().any(|t| !t.is_draft() && t.server == active_server && t.id == session_id);
    if !on_this_window {
        let _ = release.send(());
        return;
    }
    let dir = if !directory.is_empty() {
        directory.to_owned()
    } else {
        session_directory(app, session_id, "")
    };
    if dir.is_empty() {
        let _ = release.send(());
        return;
    }

    let now = Instant::now();
    let key = (active_server.clone(), session_id.to_owned());
    let same_machine = active_server.is_sidecar() && server.is_local();
    let engine_known = app.workspace_panes.browser.host.engine_caps_known();
    let engine_remote_proxy = app.workspace_panes.browser.host.engine_remote_proxy();
    let mut pending_effects = Vec::new();
    let mut hold = false;
    {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        st.created_starts.insert(key.clone());
        st.routed_sessions.insert(key.clone());
        let sse_ready = st.sse_connected.get(&active_server).copied().unwrap_or(false)
            || is_base_connected(&server.base);
        if sse_ready && (same_machine || !engine_known || engine_remote_proxy) {
            let scope_seed = {
                st.next_diag_scope = st.next_diag_scope.saturating_add(1);
                st.next_diag_scope
            };
            let att = st.sessions.entry(key.clone()).or_insert_with(|| {
                SessionAttachment::new(
                    active_server.clone(),
                    session_id.to_owned(),
                    LocationRef::new(dir.clone(), None),
                    server.clone(),
                    scope_seed,
                )
            });
            if att.location.directory != dir {
                att.location.directory = dir;
            }
            let mut effs = att.attach.step(AttachEvent::SessionCreated, now);
            effs.extend(att.attach.step(AttachEvent::EnsureAttached, now));
            if !effs.is_empty() {
                pending_effects.push((key.clone(), effs));
            }
            hold = !att.attach.prompt_gate_settled();
        }
    }
    apply_batch_effects(app, pending_effects, None);
    if hold {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        let still_waiting = st
            .sessions
            .get(&key)
            .is_some_and(|att| !att.attach.prompt_gate_settled());
        if still_waiting {
            st.start_waiters.push(StartWaiter {
                server_key: active_server,
                session_id: session_id.to_owned(),
                release,
            });
            return;
        }
    }
    let _ = release.send(());
}

pub(super) fn drain_start_waiters(app: &mut App) {
    let active_server = crate::servers::runtime::active(app).clone();
    let srv_base = live_server_clone(app).map(|s| s.base);
    let st = app.kernel.slots.get_mut(&BROWSER_RPC);
    if st.start_waiters.is_empty() {
        return;
    }
    st.start_waiters.retain(|w| {
        if w.server_key != active_server {
            return false;
        }
        let sse_ready = st.sse_connected.get(&w.server_key).copied().unwrap_or(false)
            || srv_base.as_deref().is_some_and(is_base_connected);
        if !sse_ready {
            return false;
        }
        st.sessions
            .get(&(w.server_key.clone(), w.session_id.clone()))
            .is_some_and(|att| !att.attach.prompt_gate_settled())
    });
}
