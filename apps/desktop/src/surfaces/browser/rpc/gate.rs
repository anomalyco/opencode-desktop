//! `BuiltinPromptGate` (`<= 1 s` cap) integrated with Rune's durable SQLite outbox
//! (`enqueue_prepared` / `release_prepared`), direct prompt writes (`App::write`), and
//! post-create new-session prompts (`Incoming::PromptGate`).
//!
//! Engages `experimental.browser` v4 on demand when a prompt or `/command` is actually submitted
//! and holds the outgoing prompt for at most 1 second while the attachment is in flight, so the
//! agent's first step never runs before the browser attachment and scope are in place. Resolves
//! in **0 ms** when already attached, in backoff, unsupported, blocked, offline, or mock/fixture.

use std::sync::mpsc::Sender;
use std::time::Duration;

use opencode_kit::browser_rpc::AttachEvent;
use opencode_kit::client::Write;
use opencode_kit::servers::ServerKey;
use rune::time::Instant;

use super::{
    BROWSER_RPC, apply_batch_effects, is_base_connected, live_server_clone, reconcile_sessions,
    session_directory,
};
use crate::App;

/// Maximum time a prompt may wait for `experimental.browser` to attach.
pub const PROMPT_GATE_CAP: Duration = Duration::from_secs(1);

pub(super) enum PromptGateTarget {
    /// Journaled outbox row held in `Phase::Preparing`.
    Prepared(String),
    /// Direct prompt write (`/command`, busy follow-up `admit_with`, or non-journaled send).
    Write(Write),
    /// Post-create new-session first prompt (`Write::Start` / `StartAs` / `StartInNewWorktree`).
    Notify(Sender<()>),
}

pub(super) struct PromptGateEntry {
    pub server_key: ServerKey,
    pub session_id: String,
    pub directory: Option<String>,
    pub target: PromptGateTarget,
    pub deadline: Instant,
}

/// Returns `true` ONLY when holding the prompt briefly (`<= 1 s`) can let `experimental.browser`
/// finish attaching before step 1 runs; returns `false` (0 ms) when already attached, in backoff,
/// unsupported, blocked, unavailable, disconnected, remote-unproxied, or without a location.
pub fn should_gate_prompt(app: &App, session_id: &str) -> bool {
    let dir = session_directory(app, session_id, "");
    should_gate_for_dir(app, session_id, &dir)
}

fn should_gate_for_dir(app: &App, session_id: &str, directory: &str) -> bool {
    if !app.live() || app.browser_unsupported() || app.use_mock_browser() || directory.is_empty() {
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
    let key = (server_key, session_id.to_owned());
    match st.sessions.get(&key) {
        Some(att) => !att.attach.prompt_gate_settled(),
        None => true,
    }
}

/// Engages `(active_server, session_id)` and registers a journaled `Phase::Preparing` prompt so
/// it releases the instant the session settles or after [`PROMPT_GATE_CAP`] (1 s).
pub fn register_prompt_gate(app: &mut App, session_id: &str, prompt_id: String) {
    enqueue_gate(app, session_id, None, PromptGateTarget::Prepared(prompt_id));
}

/// Engages `(active_server, session_id)` and holds a direct prompt write (`/command`, busy
/// follow-up `admit_with`, or non-journaled send) until the session settles or [`PROMPT_GATE_CAP`].
pub fn hold_write(app: &mut App, session_id: &str, write: Write) {
    enqueue_gate(app, session_id, None, PromptGateTarget::Write(write));
}

/// Engages a newly created session right after `POST /api/session` succeeds and holds its first
/// prompt (`release`) until the session settles or [`PROMPT_GATE_CAP`].
pub fn wait_after_create(app: &mut App, session_id: &str, directory: &str, release: Sender<()>) {
    if !should_gate_for_dir(app, session_id, directory) {
        let _ = release.send(());
        return;
    }
    enqueue_gate(
        app,
        session_id,
        (!directory.is_empty()).then(|| directory.to_owned()),
        PromptGateTarget::Notify(release),
    );
}

fn enqueue_gate(
    app: &mut App,
    session_id: &str,
    directory: Option<String>,
    target: PromptGateTarget,
) {
    let now = Instant::now();
    let server_key = crate::servers::runtime::active(app).clone();
    let key = (server_key.clone(), session_id.to_owned());
    {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        st.engaged.insert(key.clone());
        st.prompt_gates.push(PromptGateEntry {
            server_key,
            session_id: session_id.to_owned(),
            directory,
            target,
            deadline: now + PROMPT_GATE_CAP,
        });
    }
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
        apply_batch_effects(app, vec![(key, wake_effs)]);
    }
    let _ = drain_ready_gates(app, now);
}

/// Releases any gated prompts whose session settled or reached its `<= 1 s` cap.
/// Returns `(released_any, earliest_remaining_deadline)`.
pub(super) fn drain_ready_gates(app: &mut App, now: Instant) -> (bool, Option<Instant>) {
    let gates = std::mem::take(&mut app.kernel.slots.get_mut(&BROWSER_RPC).prompt_gates);
    if gates.is_empty() {
        return (false, None);
    }
    let mut release = Vec::new();
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
                release.push(g.target);
            } else {
                earliest = Some(earliest.map_or(g.deadline, |cur| cur.min(g.deadline)));
                keep.push(g);
            }
        }
    }
    app.kernel.slots.get_mut(&BROWSER_RPC).prompt_gates = keep;
    let released = !release.is_empty();
    for target in release {
        match target {
            PromptGateTarget::Prepared(id) => app.release_prepared(&id, None),
            PromptGateTarget::Write(w) => {
                let _ = app.write_direct(w);
            }
            PromptGateTarget::Notify(tx) => {
                let _ = tx.send(());
            }
        }
    }
    (released, earliest)
}
