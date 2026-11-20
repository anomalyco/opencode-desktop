//! Pure-Rust desktop driver for the OpenCode server's `experimental.browser` v4 RPC protocol
//! (`packages/gui-plugins/src/browser/{pane,pane-machine,machine,model}.ts`, `PRE-08` `#63500`).
//!
//! Lives in `StateSlots` (`BROWSER_RPC`, zero fields on `struct App`), wakes zero Luau VMs, and
//! keeps every session's tab namespace (`tab_<uuid>`) isolated by construction.

mod exec;
mod session;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, Sender};

use opencode_kernel::{Feature, KernelBuilder, Slot};
use opencode_kit::browser_rpc::{
    AbortHandle, AttachEffect, AttachEvent, AttachOutcome, Command, LocationRef, Outcome,
    RpcCallError, decode_control, failure_code,
};
use opencode_kit::client::Server;
use opencode_kit::servers::ServerKey;
use rune::time::Instant;
use rune::{Tx, Waker};
use serde_json::Value;

use self::session::SessionAttachment;
use crate::{App, Route, Source};

pub static BROWSER_RPC: Slot<BrowserRpcState> = Slot::new();

pub struct BrowserRpcFeature;

impl Feature<App> for BrowserRpcFeature {
    fn id(&self) -> &'static str {
        "browser-rpc"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&BROWSER_RPC);
        builder.add_setup("browser_rpc.waker", |s, w| {
            s.kernel.slots.get_mut(&BROWSER_RPC).waker = Some(w);
        });
        builder
            .schedule(crate::tick::stage::REACT_PANELS, "browser_rpc.poll", |s, _, tx| {
                poll(s, tx)
            })
            .after("browser.poll");
    }
}

enum WorkerMsg {
    AttachEnded {
        server_key: ServerKey,
        session_id: String,
        attempt: u64,
        outcome: AttachOutcome,
    },
    CommandFetched {
        server_key: ServerKey,
        session_id: String,
        connection_id: String,
        request_id: String,
        result: Result<Command, RpcCallError>,
    },
}

pub struct BrowserRpcState {
    sessions: BTreeMap<(ServerKey, String), SessionAttachment>,
    sse_connected: BTreeMap<ServerKey, bool>,
    worker_tx: Sender<WorkerMsg>,
    worker_rx: Receiver<WorkerMsg>,
    waker: Option<Waker>,
    next_diag_scope: u64,
    ref_counter: u64,
}

impl Default for BrowserRpcState {
    fn default() -> Self {
        let (worker_tx, worker_rx) = mpsc::channel();
        Self {
            sessions: BTreeMap::new(),
            sse_connected: BTreeMap::new(),
            worker_tx,
            worker_rx,
            waker: None,
            next_diag_scope: 1,
            ref_counter: 0,
        }
    }
}

/// Whether `server_key` currently has at least one live `experimental.browser` v4 attachment.
#[allow(dead_code)]
pub fn is_server_attached(app: &App, server_key: &ServerKey) -> bool {
    app.kernel
        .slots
        .get(&BROWSER_RPC)
        .sessions
        .iter()
        .any(|((k, _), att)| k == server_key && att.attach.is_attached())
}

/// Whether `session_id` on `server_key` has settled its attachment attempt for `BuiltinPromptGate`.
#[allow(dead_code)]
pub fn session_prompt_gate_settled(app: &App, server_key: &ServerKey, session_id: &str) -> bool {
    app.kernel
        .slots
        .get(&BROWSER_RPC)
        .sessions
        .get(&(server_key.clone(), session_id.to_owned()))
        .is_none_or(|att| att.attach.prompt_gate_settled())
}

static CONNECTED_BASES: std::sync::LazyLock<std::sync::Mutex<BTreeSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(BTreeSet::new()));

fn mark_base_connected(base: &str) {
    CONNECTED_BASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(base.to_owned());
}

fn is_base_connected(base: &str) -> bool {
    CONNECTED_BASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(base)
}

/// Called when `/api/event` opens or reconnects (`Incoming::Opened` / `Incoming::Reconnected`).
pub fn on_sse_connected(app: &mut App, server_key: &ServerKey) {
    if let Some(srv) = live_server_clone(app) {
        mark_base_connected(&srv.base);
    }
    let now = Instant::now();
    let mut pending_effects = Vec::new();
    {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        st.sse_connected.insert(server_key.clone(), true);
        for ((k, sid), att) in &mut st.sessions {
            if k == server_key {
                let effs = att.attach.step(AttachEvent::ServerConnected, now);
                if !effs.is_empty() {
                    pending_effects.push(((k.clone(), sid.clone()), effs));
                }
            }
        }
    }
    apply_batch_effects(app, pending_effects);
}

/// Called when the server emits `plugin.updated` (`PRE-08` `#63500`): immediately re-probes any
/// `Unavailable` / `Unsupported` / `Backoff` attachment on `server_key`.
pub fn on_plugin_updated(app: &mut App, server_key: &ServerKey) {
    let now = Instant::now();
    let mut pending_effects = Vec::new();
    {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        for ((k, sid), att) in &mut st.sessions {
            if k == server_key {
                let effs = att.attach.step(AttachEvent::PluginUpdated, now);
                if !effs.is_empty() {
                    pending_effects.push(((k.clone(), sid.clone()), effs));
                }
            }
        }
    }
    apply_batch_effects(app, pending_effects);
}

/// Routes an SSE `rpc.experimental.browser.control` frame to the owning attachment (`connectionID`).
pub fn on_control_event(app: &mut App, server_key: &ServerKey, data: &Value) {
    let Some(conn_id) = data.get("connectionID").and_then(Value::as_str) else { return };
    let now = Instant::now();
    let mut pending_effects = Vec::new();
    {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        for ((k, sid), att) in &mut st.sessions {
            if k == server_key && att.attach.connection_id() == Some(conn_id) {
                let frame = decode_control(conn_id, data);
                let effs = att.attach.step(AttachEvent::Control { frame }, now);
                if !effs.is_empty() {
                    pending_effects.push(((k.clone(), sid.clone()), effs));
                }
                break;
            }
        }
    }
    apply_batch_effects(app, pending_effects);
}

/// Called when `session.created`, `session.moved`, or `session.deleted` arrives over SSE.
pub fn on_session_lifecycle(app: &mut App, server_key: &ServerKey, ty: &str, session_id: &str) {
    let event = match ty {
        "session.created" => AttachEvent::SessionCreated,
        "session.moved" => AttachEvent::SessionMoved,
        "session.deleted" => AttachEvent::Dispose,
        _ => return,
    };
    let now = Instant::now();
    let key = (server_key.clone(), session_id.to_owned());
    let effs = {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        let Some(att) = st.sessions.get_mut(&key) else { return };
        att.attach.step(event, now)
    };
    if !effs.is_empty() {
        apply_batch_effects(app, vec![(key.clone(), effs)]);
    }
    if ty == "session.deleted" {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        if let Some(mut att) = st.sessions.remove(&key) {
            att.stop_outbound();
            if let Some((_, abort)) = att.attach_abort.take() {
                abort.abort();
            }
        }
    }
}

/// Main per-frame poll wired into `stage::REACT_PANELS` right after `browser.poll`.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let now = Instant::now();
    let mut changed = false;

    // 1. Drain worker messages (attach outcomes & fetched commands).
    let msgs: Vec<WorkerMsg> = {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        st.worker_rx.try_iter().collect()
    };
    let mut pending_effects = Vec::new();
    let mut fetched_commands = Vec::new();
    for msg in msgs {
        changed = true;
        match msg {
            WorkerMsg::AttachEnded { server_key, session_id, attempt, outcome } => {
                let key = (server_key, session_id);
                let st = app.kernel.slots.get_mut(&BROWSER_RPC);
                if let Some(att) = st.sessions.get_mut(&key) {
                    let effs = att.attach.step(AttachEvent::AttachEnded { attempt, outcome }, now);
                    if !effs.is_empty() {
                        pending_effects.push((key, effs));
                    }
                }
            }
            WorkerMsg::CommandFetched { server_key, session_id, connection_id, request_id, result } => {
                fetched_commands.push(((server_key, session_id), connection_id, request_id, result));
            }
        }
    }
    apply_batch_effects(app, pending_effects);

    // 2. Reconcile open sessions against the active live server.
    changed |= reconcile_sessions(app, now);

    // 3. Route any CDP events from BrowserHost into the owning session's TabDiagnostics.
    if app.workspace_panes.browser.host.has_cdp_events() {
        let events: Vec<_> = app.workspace_panes.browser.host.drain_cdp_events().collect();
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        for ev in events {
            for att in st.sessions.values_mut() {
                if let Some(diag) = att.diagnostics.get_mut(&ev.tab_id) {
                    diag.ingest_cdp_event(&ev.method, &ev.params, ev.session_id.as_deref());
                }
            }
        }
    }

    // 4. Start newly fetched commands & step in-flight commands without blocking the UI thread.
    // Move `sessions` temporarily out of `Slot` so executors can mutably borrow `App`.
    let (mut sessions, mut ref_counter) = {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        (std::mem::take(&mut st.sessions), st.ref_counter)
    };

    for (key, conn_id, req_id, res) in fetched_commands {
        if let Some(att) = sessions.get_mut(&key) {
            match res {
                Ok(cmd) => exec::start_command(att, app, conn_id, req_id, cmd, now, tx),
                Err(RpcCallError::InvalidInput(msg)) => {
                    if att.attach.is_request_in_flight(&conn_id, &req_id) {
                        att.enqueue_result(
                            &conn_id,
                            req_id.clone(),
                            Outcome::fail(failure_code::UNSUPPORTED, msg),
                        );
                        let _ = att.attach.step(AttachEvent::CommandFinished { request_id: req_id }, now);
                    }
                }
                Err(_) => {
                    let _ = att.attach.step(AttachEvent::CommandFinished { request_id: req_id }, now);
                }
            }
        }
    }

    let mut tick_effects = Vec::new();
    let mut earliest_wake: Option<Instant> = None;
    for (key, att) in &mut sessions {
        let (cmd_changed, cmd_wake) = exec::poll_commands(att, app, now, &mut ref_counter, tx);
        changed |= cmd_changed;
        if let Some(w) = cmd_wake {
            earliest_wake = Some(earliest_wake.map_or(w, |cur| cur.min(w)));
        }
        // Sync tab inventory changes while attached.
        if att.attach.is_attached() {
            let (inv, hash) = att.sync_tabs(app);
            let effs = att.attach.step(AttachEvent::StateChanged { hash }, now);
            if effs.iter().any(|e| matches!(e, AttachEffect::SendState { .. }))
                && let Some(cid) = att.attach.connection_id().map(str::to_owned)
            {
                att.enqueue_state(&cid, inv, hash);
            }
        }
        // Step attachment deadline / backoff timers.
        if let Some(deadline) = att.attach.next_deadline() {
            if now >= deadline {
                let effs = att.attach.step(AttachEvent::Tick, now);
                if !effs.is_empty() {
                    tick_effects.push((key.clone(), effs));
                    changed = true;
                }
            } else {
                earliest_wake = Some(earliest_wake.map_or(deadline, |cur| cur.min(deadline)));
            }
        }
    }

    {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        st.sessions = sessions;
        st.ref_counter = ref_counter;
    }
    apply_batch_effects(app, tick_effects);

    if let Some(at) = earliest_wake {
        tx.wake_after(at.saturating_duration_since(now));
    }
    changed
}

fn reconcile_sessions(app: &mut App, now: Instant) -> bool {
    if !app.live() || app.browser_unsupported() {
        return false;
    }
    let Some(server) = live_server_clone(app) else {
        return false;
    };
    let active_server = crate::servers::runtime::active(app).clone();

    // Collect every open session on the active server (all open tabs + routed session) so two
    // concurrent sessions stay attached simultaneously with independent tab namespaces.
    let mut wanted: BTreeMap<(ServerKey, String), String> = BTreeMap::new();
    for t in &app.tabs {
        if !t.is_draft() && t.server == active_server {
            let dir = session_directory(app, &t.id, &t.directory);
            if !dir.is_empty() {
                wanted.insert((active_server.clone(), t.id.clone()), dir);
            }
        }
    }
    if let Route::Session { server: rk, id } = &*app.route
        && *rk == active_server
    {
        let dir = session_directory(app, id, "");
        if !dir.is_empty() {
            wanted.insert((active_server.clone(), id.clone()), dir);
        }
    }

    let mut changed = false;
    let mut pending_effects = Vec::new();
    {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        let sse_ready = st.sse_connected.get(&active_server).copied().unwrap_or(false)
            || is_base_connected(&server.base);
        let existing_keys: BTreeSet<(ServerKey, String)> = st.sessions.keys().cloned().collect();
        for key in existing_keys {
            if !wanted.contains_key(&key) {
                if let Some(mut old) = st.sessions.remove(&key) {
                    let _ = old.attach.step(AttachEvent::Dispose, now);
                    old.stop_outbound();
                    if let Some((_, abort)) = old.attach_abort.take() {
                        abort.abort();
                    }
                    changed = true;
                }
            }
        }
        for (key, dir) in wanted {
            let same_machine = key.0.is_sidecar();
            let scope_seed = {
                st.next_diag_scope = st.next_diag_scope.saturating_add(1);
                st.next_diag_scope
            };
            let att = st.sessions.entry(key.clone()).or_insert_with(|| {
                changed = true;
                SessionAttachment::new(
                    key.0.clone(),
                    key.1.clone(),
                    LocationRef::new(dir.clone(), None),
                    server.clone(),
                    scope_seed,
                )
            });
            if att.location.directory != dir {
                att.location.directory = dir;
                let effs = att.attach.step(AttachEvent::SessionMoved, now);
                if !effs.is_empty() {
                    pending_effects.push((key.clone(), effs));
                }
            }
            if !same_machine {
                // Fail-closed security rule: never attach to remote servers without engine-enforced
                // `Ready.remote_proxy` isolation (wired with WS-1 Phase 3a in PR 4).
                let effs = att.attach.step(AttachEvent::RequireRemoteProxy, now);
                if !effs.is_empty() {
                    pending_effects.push((key, effs));
                }
            } else if sse_ready {
                let effs = att.attach.step(AttachEvent::EnsureAttached, now);
                if !effs.is_empty() {
                    pending_effects.push((key, effs));
                }
            }
        }
    }
    apply_batch_effects(app, pending_effects);
    changed
}

fn apply_batch_effects(
    app: &mut App,
    batch: Vec<((ServerKey, String), Vec<AttachEffect>)>,
) {
    if batch.is_empty() {
        return;
    }
    let (worker_tx, waker) = {
        let st = app.kernel.slots.get(&BROWSER_RPC);
        (st.worker_tx.clone(), st.waker.clone())
    };
    let mut sessions = std::mem::take(&mut app.kernel.slots.get_mut(&BROWSER_RPC).sessions);
    for (key, effects) in batch {
        let Some(att) = sessions.get_mut(&key) else { continue };
        for effect in effects {
            match effect {
                AttachEffect::StartAttach { attempt, connection_id } => {
                    if let Some((_, prev_abort)) = att.attach_abort.take() {
                        prev_abort.abort();
                    }
                    let abort = AbortHandle::new();
                    att.attach_abort = Some((attempt, abort.clone()));
                    let server = att.server.clone();
                    let location = att.location.clone();
                    let sid = att.session_id.clone();
                    let skey = att.server_key.clone();
                    let tx = worker_tx.clone();
                    let wake = waker.clone();
                    std::thread::Builder::new()
                        .name("browser-rpc-attach".into())
                        .spawn(move || {
                            let outcome = server.browser_rpc_attach(&location, &sid, &connection_id, &abort);
                            let _ = tx.send(WorkerMsg::AttachEnded {
                                server_key: skey,
                                session_id: sid,
                                attempt,
                                outcome,
                            });
                            if let Some(w) = wake {
                                w();
                            }
                        })
                        .ok();
                }
                AttachEffect::AbortAttach { attempt } => {
                    if att.attach_abort.as_ref().is_some_and(|(a, _)| *a == attempt)
                        && let Some((_, abort)) = att.attach_abort.take()
                    {
                        abort.abort();
                    }
                    att.stop_outbound();
                }
                AttachEffect::SendState { connection_id } => {
                    let (inv, hash) = att.sync_tabs(app);
                    att.enqueue_state(&connection_id, inv, hash);
                }
                AttachEffect::FetchCommand { connection_id, request_id } => {
                    let server = att.server.clone();
                    let location = att.location.clone();
                    let sid = att.session_id.clone();
                    let skey = att.server_key.clone();
                    let tx = worker_tx.clone();
                    let wake = waker.clone();
                    std::thread::Builder::new()
                        .name("browser-rpc-cmd".into())
                        .spawn(move || {
                            let result =
                                server.browser_rpc_command(&location, &sid, &connection_id, &request_id);
                            let _ = tx.send(WorkerMsg::CommandFetched {
                                server_key: skey,
                                session_id: sid,
                                connection_id,
                                request_id,
                                result,
                            });
                            if let Some(w) = wake {
                                w();
                            }
                        })
                        .ok();
                }
                AttachEffect::CancelCommands { request_ids } => {
                    att.active_commands
                        .retain(|c| !request_ids.iter().any(|id| id == &c.request_id));
                }
                AttachEffect::ScheduleTimer { .. }
                | AttachEffect::SettlePromptGates
                | AttachEffect::SetMcpSuppressed { .. } => {}
            }
        }
    }
    app.kernel.slots.get_mut(&BROWSER_RPC).sessions = sessions;
}

fn live_server_clone(app: &App) -> Option<Server> {
    match &*app.source {
        Source::Live { server, .. } => Some(server.clone()),
        Source::Fixture => None,
    }
}

fn session_directory(app: &App, sid: &str, fallback: &str) -> String {
    if let Some(info) = app.session(sid)
        && !info.directory.is_empty()
    {
        return info.directory.clone();
    }
    // Wait until the session's real directory is known (`prompt-gate.ts:10`: `session.location !== undefined`)
    // instead of attaching under `$HOME` before `Incoming::Sessions` arrives.
    fallback.to_owned()
}
