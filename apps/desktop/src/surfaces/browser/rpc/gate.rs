//! `BuiltinPromptGate` (`<= 1 s` cap) covering every prompt, command, busy follow-up, retry,
//! outbox flush, new-session start, and queue rewrite path (`packages/gui-plugins/src/browser/{prompt-gate,model}.ts`,
//! `packages/app/src/runtime/plugin/{host-apis.tsx:1246-1251, prompt-gate.ts:4-36}`,
//! `packages/app/src/composer/submit.ts:135-169, 193, 251, 307-309, 518, 643-668`).
//!
//! - Every gate release runs bound to the gate's own `server_key` via `runtime::with_server`
//!   (matching official `plugins.ready(server.key, sessionID)` -> `sendPrompt(session, ...)`).
//! - Resolves in **0 ms** when already attached, in `Backoff`, `Unsupported`, `Unavailable`,
//!   `Blocked`, offline, or in mock/fixture mode.

use std::sync::mpsc::Sender;
use std::time::Duration;

use opencode_feature_inbox::queue::RewriteTask;
use opencode_kit::browser_rpc::{AttachEvent, LocationRef};
use opencode_kit::client::Write;
use opencode_kit::servers::ServerKey;
use rune::time::Instant;

use super::session::SessionAttachment;
use super::{BROWSER_RPC, apply_batch_effects, is_base_connected, live_server_clone, session_directory};
use crate::App;

/// Maximum time a prompt/command may wait for `experimental.browser` to attach (`PROMPT_GATE_TIMEOUT_MS`).
pub const PROMPT_GATE_CAP: Duration = Duration::from_secs(1);

pub(super) enum PromptGateTarget {
    Prepared(String),
    Outbox,
    Write(Write),
    Rewrite(RewriteTask),
    StartGate(Sender<()>),
}

pub(super) struct PromptGateEntry {
    pub server_key: ServerKey,
    pub session_id: String,
    pub target: PromptGateTarget,
    pub deadline: Instant,
}

fn should_gate_for(app: &App, session_id: &str, fallback_dir: &str) -> bool {
    if !app.live() || app.browser_unsupported() || app.use_mock_browser() {
        return false;
    }
    let Some(server) = live_server_clone(app) else {
        return false;
    };
    let server_key = crate::servers::runtime::active(app).clone();
    let st = app.kernel.slots.get(&BROWSER_RPC);
    let sse_ready = st
        .sse_connected
        .get(&server_key)
        .copied()
        .unwrap_or(false)
        || is_base_connected(&server.base);
    if !sse_ready {
        return false;
    }
    if session_directory(app, session_id, fallback_dir).is_empty() {
        return false;
    }
    let key = (server_key, session_id.to_owned());
    match st.sessions.get(&key) {
        Some(att) => {
            if att.attach.prompt_gate_settled() {
                return false;
            }
            let cur_attempt = att.attach.connecting_attempt().unwrap_or(0);
            st.gate_timed_out_attempt.get(&key) != Some(&cur_attempt)
        }
        None => {
            let same_machine = key.0.is_sidecar() && server.is_local();
            if !same_machine
                && app.workspace_panes.browser.host.engine_caps_known()
                && !app.workspace_panes.browser.host.engine_remote_proxy()
            {
                return false;
            }
            st.gate_timed_out_attempt.get(&key) != Some(&0)
        }
    }
}

/// Returns `true` ONLY when holding the prompt briefly (`<= 1 s`) can let `experimental.browser`
/// finish attaching before step 1 runs; returns `false` (0 ms) when already attached, in backoff,
/// unsupported, blocked, unavailable, disconnected, or without a session location.
pub fn should_gate_prompt(app: &App, session_id: &str) -> bool {
    should_gate_for(app, session_id, "")
}

/// Ensures `(active_server, session_id)` has a live `SessionAttachment` and starts attaching
/// immediately (`model.ts:766`: *"the session attaches now, routed or not"*). Returns `true` if
/// the session is already settled (caller may dispatch immediately in 0 ms).
fn engage_and_check_settled(app: &mut App, session_id: &str, fallback_dir: &str, now: Instant) -> bool {
    let Some(server) = live_server_clone(app) else {
        return true;
    };
    let server_key = crate::servers::runtime::active(app).clone();
    let dir = session_directory(app, session_id, fallback_dir);
    if dir.is_empty() {
        return true;
    }
    let same_machine = server_key.is_sidecar() && server.is_local();
    let engine_remote_ok = !app.workspace_panes.browser.host.engine_caps_known()
        || app.workspace_panes.browser.host.engine_remote_proxy();
    let key = (server_key.clone(), session_id.to_owned());
    let effs = {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        st.routed_sessions.insert(key.clone());
        let scope_seed = {
            st.next_diag_scope = st.next_diag_scope.saturating_add(1);
            st.next_diag_scope
        };
        let att = st.sessions.entry(key.clone()).or_insert_with(|| {
            SessionAttachment::new(
                server_key.clone(),
                session_id.to_owned(),
                LocationRef::new(dir.clone(), None),
                server.clone(),
                scope_seed,
            )
        });
        if !same_machine && !engine_remote_ok {
            att.attach.step(AttachEvent::RequireRemoteProxy, now)
        } else {
            att.attach.step(AttachEvent::EnsureAttached, now)
        }
    };
    if !effs.is_empty() {
        apply_batch_effects(app, vec![(key.clone(), effs)], None);
    }
    let st = app.kernel.slots.get(&BROWSER_RPC);
    match st.sessions.get(&key) {
        None => true,
        Some(att) => {
            if att.attach.prompt_gate_settled() {
                true
            } else {
                let cur_attempt = att.attach.connecting_attempt().unwrap_or(0);
                st.gate_timed_out_attempt.get(&key) == Some(&cur_attempt)
            }
        }
    }
}

/// Registers a prompt already journaled in `Phase::Preparing` (`enqueue_prepared`) so it releases
/// bound to its own `server_key` once `(server_key, session_id)` settles or after `1 s`.
pub fn register_prompt_gate(app: &mut App, session_id: &str, prompt_id: String) {
    let now = Instant::now();
    let server_key = crate::servers::runtime::active(app).clone();
    if engage_and_check_settled(app, session_id, "", now) {
        app.release_prepared(&prompt_id, None);
        return;
    }
    app.kernel
        .slots
        .get_mut(&BROWSER_RPC)
        .prompt_gates
        .push(PromptGateEntry {
            server_key,
            session_id: session_id.to_owned(),
            target: PromptGateTarget::Prepared(prompt_id),
            deadline: now + PROMPT_GATE_CAP,
        });
}

/// Single choke point called by `pump_outbox` (`features/inbox/src/send_failed/outbox.rs`) covering
/// ALL journaled sends: idle prompts, busy follow-ups (`admit_with`: steer/queue/Alt+Enter first),
/// manual retries, and reopen/reconnect/multi-window flushes.
/// Returns `true` if `pump_outbox` may dispatch immediately; `false` if held for `<= 1 s`.
pub fn ensure_outbox_gate(app: &mut App, session_id: &str) -> bool {
    if !should_gate_for(app, session_id, "") {
        return true;
    }
    let now = Instant::now();
    let server_key = crate::servers::runtime::active(app).clone();
    if engage_and_check_settled(app, session_id, "", now) {
        return true;
    }
    let st = app.kernel.slots.get_mut(&BROWSER_RPC);
    let already_waiting = st.prompt_gates.iter().any(|g| {
        g.server_key == server_key
            && g.session_id == session_id
            && matches!(g.target, PromptGateTarget::Outbox)
    });
    if !already_waiting {
        st.prompt_gates.push(PromptGateEntry {
            server_key,
            session_id: session_id.to_owned(),
            target: PromptGateTarget::Outbox,
            deadline: now + PROMPT_GATE_CAP,
        });
    }
    false
}

/// Holds a direct live `Write` (`/command`, non-journaled prompt/deliver) until `experimental.browser`
/// settles (`<= 1 s`), then dispatches it on its own `server_key`. Returns `true` if held.
pub fn hold_write(app: &mut App, session_id: &str, write: Write) -> Result<(), Write> {
    if !should_gate_for(app, session_id, "") {
        return Err(write);
    }
    let now = Instant::now();
    let server_key = crate::servers::runtime::active(app).clone();
    if engage_and_check_settled(app, session_id, "", now) {
        return Err(write);
    }
    app.kernel
        .slots
        .get_mut(&BROWSER_RPC)
        .prompt_gates
        .push(PromptGateEntry {
            server_key,
            session_id: session_id.to_owned(),
            target: PromptGateTarget::Write(write),
            deadline: now + PROMPT_GATE_CAP,
        });
    Ok(())
}

/// Holds a queue rewrite chain (`QueueJob::Rewrite`, which re-admits prompts via `rewrite_queue`)
/// until `experimental.browser` settles (`<= 1 s`), then spawns it on its own `server_key`.
pub fn hold_rewrite(app: &mut App, task: RewriteTask) -> Result<(), RewriteTask> {
    if !should_gate_for(app, task.session(), "") {
        return Err(task);
    }
    let now = Instant::now();
    let server_key = crate::servers::runtime::active(app).clone();
    let session_id = task.session().to_owned();
    if engage_and_check_settled(app, &session_id, "", now) {
        return Err(task);
    }
    app.kernel
        .slots
        .get_mut(&BROWSER_RPC)
        .prompt_gates
        .push(PromptGateEntry {
            server_key,
            session_id,
            target: PromptGateTarget::Rewrite(task),
            deadline: now + PROMPT_GATE_CAP,
        });
    Ok(())
}

/// Holds a brand-new session (`Write::Start` / `StartAs` / `StartInNewWorktree` via
/// `Incoming::PromptGate`) right after `POST /api/session` and before `POST /prompt` (`<= 1 s`).
pub fn hold_start_gate(app: &mut App, session_id: &str, directory: &str, release: Sender<()>) {
    if !should_gate_for(app, session_id, directory) {
        let _ = release.send(());
        return;
    }
    let now = Instant::now();
    let server_key = crate::servers::runtime::active(app).clone();
    if engage_and_check_settled(app, session_id, directory, now) {
        let _ = release.send(());
        return;
    }
    app.kernel
        .slots
        .get_mut(&BROWSER_RPC)
        .prompt_gates
        .push(PromptGateEntry {
            server_key,
            session_id: session_id.to_owned(),
            target: PromptGateTarget::StartGate(release),
            deadline: now + PROMPT_GATE_CAP,
        });
}

/// Releases any waiting prompt/command/outbox/rewrite/start gates whose session settled, lost SSE,
/// or reached its `<= 1 s` cap — binding every release strictly to `g.server_key` via
/// `crate::servers::runtime::with_server`.
pub(super) fn drain_ready_gates(app: &mut App, now: Instant) -> (bool, Option<Instant>) {
    let gates = std::mem::take(&mut app.kernel.slots.get_mut(&BROWSER_RPC).prompt_gates);
    if gates.is_empty() {
        return (false, None);
    }
    let mut ready = Vec::new();
    let mut keep = Vec::with_capacity(gates.len());
    let mut earliest: Option<Instant> = None;
    {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        for g in gates {
            let key = (g.server_key.clone(), g.session_id.clone());
            let timed_out = now >= g.deadline;
            let settled = timed_out
                || st
                    .sessions
                    .get(&key)
                    .is_none_or(|att| att.attach.prompt_gate_settled())
                || !st.sse_connected.get(&g.server_key).copied().unwrap_or(true);
            if settled {
                if timed_out {
                    let attempt = st
                        .sessions
                        .get(&key)
                        .and_then(|att| att.attach.connecting_attempt())
                        .unwrap_or(0);
                    st.gate_timed_out_attempt.insert(key, attempt);
                }
                ready.push(g);
            } else {
                earliest = Some(earliest.map_or(g.deadline, |cur| cur.min(g.deadline)));
                keep.push(g);
            }
        }
    }
    app.kernel.slots.get_mut(&BROWSER_RPC).prompt_gates = keep;
    let released = !ready.is_empty();
    for g in ready {
        match g.target {
            PromptGateTarget::Prepared(id) => {
                crate::servers::runtime::with_server(app, &g.server_key, |s| {
                    s.release_prepared(&id, None);
                });
            }
            PromptGateTarget::Outbox => {
                crate::servers::runtime::with_server(app, &g.server_key, |s| {
                    s.pump_outbox();
                });
            }
            PromptGateTarget::Write(w) => {
                crate::servers::runtime::with_server(app, &g.server_key, |s| {
                    s.send_live_write(w);
                });
            }
            PromptGateTarget::Rewrite(task) => {
                crate::servers::runtime::with_server(app, &g.server_key, |s| {
                    s.spawn_rewrite_now(task);
                });
            }
            PromptGateTarget::StartGate(tx) => {
                let _ = tx.send(());
            }
        }
    }
    (released, earliest)
}
