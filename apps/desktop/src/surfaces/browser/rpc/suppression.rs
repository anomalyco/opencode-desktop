//! Legacy `rune_mcp` `browser_*` suppression and one-shot local RPC capability probing.

use std::collections::BTreeMap;

use opencode_kit::browser_rpc::{AttachState, BlockReason, LocationRef};
use opencode_kit::client::Server;
use opencode_kit::servers::ServerKey;

use super::{BROWSER_RPC, BrowserRpcState, ServerRpcVerdict, WorkerMsg, live_server_clone};
use crate::App;

pub(super) fn server_suppressed(st: &BrowserRpcState, server_key: &ServerKey) -> bool {
    let mut any_proven_legacy = false;
    for ((k, _), att) in &st.sessions {
        if k != server_key {
            continue;
        }
        if matches!(
            att.attach.state,
            AttachState::Attached { .. }
                | AttachState::Blocked {
                    reason: BlockReason::Replaced | BlockReason::Missing,
                }
        ) {
            return true;
        }
        if matches!(att.attach.state, AttachState::Unsupported | AttachState::Unavailable) {
            any_proven_legacy = true;
        }
    }
    if any_proven_legacy {
        return false;
    }
    !matches!(
        st.server_verdicts.get(server_key),
        Some((_, _, ServerRpcVerdict::LegacyAllowed))
    )
}

/// Runs a one-shot non-mutating capability probe (`POST /api/rpc/experimental.browser/state {}`)
/// on the active same-machine local server when there is no cached verdict yet (Home, Draft,
/// restored/background tabs, or slow SSE before any session attaches).
pub(super) fn maybe_start_capability_probe(app: &mut App) {
    if !app.live() || app.browser_unsupported() {
        return;
    }
    let Some(server) = live_server_clone(app) else {
        return;
    };
    let active_server = crate::servers::runtime::active(app).clone();
    if !(active_server.is_sidecar() && server.is_local()) {
        return;
    }
    let probe_dir = app
        .projects
        .first()
        .map(|p| p.directory.as_str())
        .filter(|d| !d.is_empty())
        .unwrap_or(&app.default_dir)
        .to_owned();
    let (worker_tx, waker, generation) = {
        let st = app.kernel.slots.get_mut(&BROWSER_RPC);
        let already_have = st
            .server_verdicts
            .get(&active_server)
            .is_some_and(|(prev, _, _)| prev.endpoint_key() == server.endpoint_key());
        if already_have {
            return;
        }
        let any_session_active = st.sessions.iter().any(|((k, _), att)| {
            *k == active_server
                && !matches!(att.attach.state, AttachState::Idle | AttachState::Closed)
        });
        if any_session_active {
            return;
        }
        st.probe_gen = st.probe_gen.saturating_add(1);
        let generation = st.probe_gen;
        st.server_verdicts
            .insert(active_server.clone(), (server.clone(), generation, ServerRpcVerdict::Probing));
        (st.worker_tx.clone(), st.waker.clone(), generation)
    };
    let location = LocationRef::new(probe_dir, None);
    std::thread::Builder::new()
        .name("browser-rpc-probe".into())
        .spawn(move || {
            let supported = server.browser_rpc_probe(&location);
            let _ = worker_tx.send(WorkerMsg::ProbeEnded {
                server_key: active_server,
                server,
                generation,
                supported,
            });
            if let Some(w) = waker {
                w();
            }
        })
        .ok();
}

pub(super) fn sync_mcp_suppression(app: &mut App) {
    let window_id = app.kernel.slots.get(&crate::rune_mcp::RUNE_MCP).window_id();
    let st = app.kernel.slots.get_mut(&BROWSER_RPC);
    let mut session_proofs: BTreeMap<ServerKey, (Server, bool)> = BTreeMap::new();
    for ((k, _), att) in &st.sessions {
        match att.attach.state {
            AttachState::Attached { .. }
            | AttachState::Blocked {
                reason: BlockReason::Replaced | BlockReason::Missing,
            } => {
                session_proofs.insert(k.clone(), (att.server.clone(), true));
            }
            AttachState::Unsupported | AttachState::Unavailable => {
                session_proofs
                    .entry(k.clone())
                    .or_insert_with(|| (att.server.clone(), false));
            }
            _ => {}
        }
    }
    for (k, (srv, supported)) in session_proofs {
        let next = if supported { ServerRpcVerdict::Supported } else { ServerRpcVerdict::LegacyAllowed };
        let needs_update = st
            .server_verdicts
            .get(&k)
            .is_none_or(|(prev, _, v)| *v != next || prev.endpoint_key() != srv.endpoint_key());
        if needs_update {
            st.probe_gen = st.probe_gen.saturating_add(1);
            let g = st.probe_gen;
            st.server_verdicts.insert(k, (srv, g, next));
        }
    }

    let mut keys: BTreeMap<ServerKey, Server> = BTreeMap::new();
    for (k, (srv, _, _)) in &st.server_verdicts {
        keys.insert(k.clone(), srv.clone());
    }
    for ((k, _), att) in &st.sessions {
        keys.entry(k.clone()).or_insert_with(|| att.server.clone());
    }
    let mut current: BTreeMap<ServerKey, (Server, bool)> = BTreeMap::new();
    for (k, srv) in keys {
        let suppressed = server_suppressed(st, &k);
        current.insert(k, (srv, suppressed));
    }
    let mut updates: Vec<(Server, Option<bool>)> = Vec::new();
    for (k, (srv, suppressed)) in &current {
        match st.mcp_suppressed.get(k) {
            Some((prev_srv, _)) if prev_srv.endpoint_key() != srv.endpoint_key() => {
                updates.push((prev_srv.clone(), None));
                updates.push((srv.clone(), Some(*suppressed)));
            }
            Some((_, prev_s)) if *prev_s == *suppressed => {}
            _ => {
                updates.push((srv.clone(), Some(*suppressed)));
            }
        }
    }
    for (k, (srv, _)) in &st.mcp_suppressed {
        if !current.contains_key(k) {
            updates.push((srv.clone(), None));
        }
    }
    st.mcp_suppressed = current;
    for (srv, suppressed) in updates {
        crate::rune_mcp::set_browser_rpc_attached(window_id, &srv, suppressed);
    }
}
