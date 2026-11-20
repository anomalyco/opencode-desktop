//! `BuiltinPromptGate` (`<= 1 s` cap) integrated with Rune's durable SQLite outbox
//! (`enqueue_prepared` / `release_prepared`, `packages/gui-plugins/src/browser/{prompt-gate,model}.ts`).
//!
//! Engages `experimental.browser` v4 on demand when a prompt is actually submitted and holds the
//! outgoing prompt for at most 1 second while the attachment is in flight, so the agent's first
//! step never runs before the browser attachment and scope are in place. Resolves in **0 ms** when
//! already attached, unsupported (older servers), blocked, offline, or in mock/fixture mode.

use std::time::Duration;

use opencode_kit::browser_rpc::AttachEvent;
use opencode_kit::servers::ServerKey;
use rune::time::Instant;

use super::{
    BROWSER_RPC, apply_batch_effects, is_base_connected, live_server_clone, reconcile_sessions,
    session_directory,
};
use crate::App;

/// Maximum time a prompt may wait in `Phase::Preparing` for `experimental.browser` to attach.
pub const PROMPT_GATE_CAP: Duration = Duration::from_secs(1);

pub(super) struct PromptGateEntry {
    pub server_key: ServerKey,
    pub session_id: String,
    pub prompt_id: String,
    pub deadline: Instant,
}

/// Returns `true` ONLY when holding the prompt briefly (`<= 1 s`) can let `experimental.browser`
/// finish attaching before step 1 runs; returns `false` (0 ms) when already attached, unsupported,
/// blocked, unavailable, disconnected, remote-unproxied, or without a session location.
pub fn should_gate_prompt(app: &App, session_id: &str) -> bool {
    if !app.live() || app.browser_unsupported() || app.use_mock_browser() {
        return false;
    }
    let Some(server) = live_server_clone(app) else {
        return false;
    };
    let server_key = crate::servers::runtime::active(app).clone();
    // On `dev`, remote servers fail closed (`RequireRemoteProxy`) with zero attach wait.
    if !server_key.is_sidecar() {
        return false;
    }
    let st = app.kernel.slots.get(&BROWSER_RPC);
    let sse_ready = st.sse_connected.get(&server_key).copied().unwrap_or(false)
        || is_base_connected(&server.base);
    if !sse_ready {
        return false;
    }
    if session_directory(app, session_id, "").is_empty() {
        return false;
    }
    let key = (server_key, session_id.to_owned());
    match st.sessions.get(&key) {
        Some(att) => !att.attach.prompt_gate_settled(),
        None => true,
    }
}

/// Engages `(active_server, session_id)` and registers a prompt already journaled in
/// `Phase::Preparing` (`enqueue_prepared`) so it releases the instant the session settles or after
/// [`PROMPT_GATE_CAP`] (1 s).
pub fn register_prompt_gate(app: &mut App, session_id: &str, prompt_id: String) {
    let now = Instant::now();
    let server_key = crate::servers::runtime::active(app).clone();
    let key = (server_key.clone(), session_id.to_owned());
    app.kernel
        .slots
        .get_mut(&BROWSER_RPC)
        .engaged
        .insert(key.clone());
    // Ensure session entry exists and starts attaching immediately now that it is engaged.
    let _ = reconcile_sessions(app, now);
    let wake_effs = {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        st.sessions
            .get_mut(&key)
            .map(|att| att.attach.step(AttachEvent::EnsureAttached, now))
            .unwrap_or_default()
    };
    if !wake_effs.is_empty() {
        apply_batch_effects(app, vec![(key.clone(), wake_effs)]);
    }
    let settled = app
        .kernel
        .slots
        .get(&BROWSER_RPC)
        .sessions
        .get(&key)
        .is_none_or(|att| att.attach.prompt_gate_settled());
    if settled {
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
            prompt_id,
            deadline: now + PROMPT_GATE_CAP,
        });
}

/// Releases any prepared prompts whose session settled or reached its `<= 1 s` cap.
/// Returns `(released_any, earliest_remaining_deadline)`.
pub(super) fn drain_ready_gates(app: &mut App, now: Instant) -> (bool, Option<Instant>) {
    let gates = std::mem::take(&mut app.kernel.slots.get_mut(&BROWSER_RPC).prompt_gates);
    if gates.is_empty() {
        return (false, None);
    }
    let mut release_ids = Vec::new();
    let mut keep = Vec::with_capacity(gates.len());
    let mut earliest: Option<Instant> = None;
    {
        let st = app.kernel.slots.get(&BROWSER_RPC);
        for g in gates {
            let key = (g.server_key.clone(), g.session_id.clone());
            let settled = now >= g.deadline
                || st
                    .sessions
                    .get(&key)
                    .is_none_or(|att| att.attach.prompt_gate_settled())
                || !st.sse_connected.get(&g.server_key).copied().unwrap_or(true);
            if settled {
                release_ids.push(g.prompt_id);
            } else {
                earliest = Some(earliest.map_or(g.deadline, |cur| cur.min(g.deadline)));
                keep.push(g);
            }
        }
    }
    app.kernel.slots.get_mut(&BROWSER_RPC).prompt_gates = keep;
    let released = !release_ids.is_empty();
    for id in release_ids {
        app.release_prepared(&id, None);
    }
    (released, earliest)
}
