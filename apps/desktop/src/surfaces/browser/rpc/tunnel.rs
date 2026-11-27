//! Per-remote-server loopback [`TunnelProxy`] lifecycle synced to `BrowserHost::set_partition_proxy`
//! (WS-1 `#288`/`#292` `ContextKind::RemoteServer` contract + `review-rd-engine.md` point 4 +
//! `opencode#66217` multiplexed binary WebSocket stream transport & `tunnel.grant`).
//!
//! Invariants:
//! - Every remote tunnel binds strictly to IP literal `127.0.0.1` (never a hostname) with fresh
//!   384-bit OS-CSPRNG Basic credentials (`realm = "OpenCode Browser Proxy"`).
//! - When `attached` advertises `tunnel.ws.v1` (and `OPENCODE_BROWSER_TUNNEL_TRANSPORT != legacy`),
//!   `TunnelProxy::activate` prewarms one authenticated binary WebSocket [`ClientMux`] and signals
//!   `WorkerMsg::StreamActivated` / `WorkerMsg::StreamEnded` so the WS owns attachment liveness.
//! - On rotation, the new proxy binds while the prior listener is still open (so the OS cannot
//!   recycle the port), updates `BrowserHost::set_partition_proxy(server_id, Some(new_cfg))`
//!   atomically, and only then closes the prior proxy.
//! - During transient reconnect (`Connecting`/`Backoff` after a prior `Attached` state), the old
//!   proxy is suspended (`503 Service Unavailable`, zero egress) instead of sending
//!   `SetPartitionProxy(None)`. On permanent detach/close/block/unsupported/unavailable,
//!   `BrowserHost::set_partition_proxy(server_id, None)` and `proxy.close()` run immediately.

use std::collections::{BTreeMap, BTreeSet};

use browser_pane::RemoteProxyConfig;
use opencode_kit::browser_rpc::{AttachState, LocationRef, PROXY_REALM, TunnelProxy, TunnelTarget};
use opencode_kit::client::Server;
use opencode_kit::servers::ServerKey;

use super::{BROWSER_RPC, WorkerMsg};
use crate::App;

pub(super) struct ActiveTunnel {
    pub connection_id: String,
    pub proxy: TunnelProxy,
}

#[derive(Default)]
pub(super) struct RemoteTunnelManager {
    tunnels: BTreeMap<ServerKey, ActiveTunnel>,
}

struct DesiredTunnel {
    server: Server,
    location: LocationRef,
    session_id: String,
    attempt: u64,
    connection_id: String,
    capabilities: Vec<String>,
}

/// Sends `experimental.browser/tunnel.grant` on the active remote [`TunnelProxy`] for a trusted
/// desktop navigation URL (`packages/gui-plugins/src/browser/pane.ts:609-644`, opencode#66217).
/// No-op for local servers, non-HTTP(S) URLs, or legacy servers that did not advertise `tunnel.ws.v1`.
pub fn grant_remote_navigation(app: &App, sid: Option<&str>, url: &str) {
    let server_key = app.browser_server_for_session(sid);
    if server_key.is_sidecar() && app.session_server_is_local(sid) {
        return;
    }
    let Some(target) = parse_http_url_target(url) else { return };
    let st = app.kernel.slots.get(&BROWSER_RPC);
    if let Some(active) = st.tunnels.tunnels.get(&server_key) {
        active.proxy.grant_private_target(target, Vec::new());
    }
}

fn parse_http_url_target(raw: &str) -> Option<TunnelTarget> {
    let trimmed = raw.trim();
    let (scheme, rest) = trimmed.split_once("://")?;
    let default_port = if scheme.eq_ignore_ascii_case("https") {
        443u16
    } else if scheme.eq_ignore_ascii_case("http") {
        80u16
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    let (host, port) = if let Some(after) = host_port.strip_prefix('[') {
        let (h, tail) = after.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => p.parse::<u16>().ok()?,
            None if tail.is_empty() => default_port,
            None => return None,
        };
        (h, port)
    } else if let Some((h, p)) = host_port.rsplit_once(':') {
        (h, p.parse::<u16>().ok()?)
    } else {
        (host_port, default_port)
    };
    TunnelTarget::parse(host, port)
}

/// Immediately suspends any active remote [`TunnelProxy`] for `server_key` when `/api/event`
/// drops (`on_sse_disconnected`), shutting down all tracked sockets (including upgraded
/// WebSockets) in 0 ms and clearing the engine partition proxy.
pub(super) fn suspend_server_tunnels(app: &mut App, server_key: &ServerKey) {
    let mgr = std::mem::take(&mut app.kernel.slots.get_mut(&BROWSER_RPC).tunnels);
    if let Some(cur) = mgr.tunnels.get(server_key) {
        cur.proxy.set_suspended(true);
        app.workspace_panes
            .browser
            .host
            .set_partition_proxy(server_key.as_str(), None);
    }
    app.kernel.slots.get_mut(&BROWSER_RPC).tunnels = mgr;
}

/// Reconciles active [`TunnelProxy`] instances against currently attached remote-server sessions
/// and updates `BrowserHost::set_partition_proxy` on every start, rotation, or stop.
pub(super) fn sync_remote_tunnels(app: &mut App) {
    let engine_known = app.workspace_panes.browser.host.engine_caps_known();
    let engine_remote_ok = !engine_known || app.workspace_panes.browser.host.engine_remote_proxy();

    let (worker_tx, waker) = {
        let st = app.kernel.slots.get(&BROWSER_RPC);
        (
            st.worker_tx.clone(),
            app.workspace_panes.browser.host.message_waker().or_else(|| st.waker.clone()),
        )
    };

    let mut desired: BTreeMap<ServerKey, DesiredTunnel> = BTreeMap::new();
    let mut reconnecting: BTreeSet<ServerKey> = BTreeSet::new();
    if engine_remote_ok {
        let st = app.kernel.slots.get(&BROWSER_RPC);
        for ((key, sid), att) in &st.sessions {
            let same_machine = key.is_sidecar() && att.server.is_local();
            if same_machine {
                continue;
            }
            let sse_up = st.sse_connected.get(key).copied().unwrap_or(true);
            if sse_up
                && let Some(conn_id) = att.attach.connection_id()
                && let Some(attempt) = att.attach.attempt()
                && att.attach.is_attached()
            {
                desired.entry(key.clone()).or_insert_with(|| DesiredTunnel {
                    server: att.server.clone(),
                    location: att.location.clone(),
                    session_id: sid.clone(),
                    attempt,
                    connection_id: conn_id.to_owned(),
                    capabilities: att.attach.capabilities().to_vec(),
                });
            } else if !sse_up
                || matches!(
                    att.attach.state,
                    AttachState::Connecting { .. } | AttachState::Loading { .. } | AttachState::Backoff { .. }
                )
            {
                reconnecting.insert(key.clone());
            }
        }
    }

    let mut mgr = std::mem::take(&mut app.kernel.slots.get_mut(&BROWSER_RPC).tunnels);

    let mut stale: Vec<ServerKey> = Vec::new();
    for (key, cur) in &mgr.tunnels {
        if desired.contains_key(key) {
            continue;
        }
        if reconnecting.contains(key) {
            cur.proxy.set_suspended(true);
        } else {
            stale.push(key.clone());
        }
    }
    for key in stale {
        if let Some(mut old) = mgr.tunnels.remove(&key) {
            app.workspace_panes.browser.host.set_partition_proxy(key.as_str(), None);
            old.proxy.close();
        }
    }

    for (key, want) in desired {
        if let Some(cur) = mgr.tunnels.get(&key)
            && cur.connection_id == want.connection_id
        {
            cur.proxy.set_suspended(false);
            if app.workspace_panes.browser.host.remote_proxy(key.as_str()).is_none() {
                let cfg = RemoteProxyConfig {
                    host: "127.0.0.1".to_owned(),
                    port: cur.proxy.port,
                    username: Some(cur.proxy.credentials.username.clone()),
                    password: Some(cur.proxy.credentials.password.clone()),
                    realm: Some(PROXY_REALM.to_owned()),
                };
                app.workspace_panes
                    .browser
                    .host
                    .set_partition_proxy(key.as_str(), Some(cfg));
            }
            continue;
        }
        match TunnelProxy::start(
            want.server,
            want.location,
            want.session_id.clone(),
            want.connection_id.clone(),
        ) {
            Ok(proxy) => {
                let tx_ready = worker_tx.clone();
                let wake_ready = waker.clone();
                let key_ready = key.clone();
                let sid_ready = want.session_id.clone();
                let attempt = want.attempt;
                let tx_closed = worker_tx.clone();
                let wake_closed = waker.clone();
                let key_closed = key.clone();
                let sid_closed = want.session_id;
                proxy.activate(
                    &want.capabilities,
                    Box::new(move || {
                        let _ = tx_ready.send(WorkerMsg::StreamActivated {
                            server_key: key_ready,
                            session_id: sid_ready,
                            attempt,
                        });
                        if let Some(w) = wake_ready {
                            w();
                        }
                    }),
                    Box::new(move |info| {
                        let _ = tx_closed.send(WorkerMsg::StreamEnded {
                            server_key: key_closed,
                            session_id: sid_closed,
                            attempt,
                            replaced: info.replaced,
                            reason: info.reason,
                        });
                        if let Some(w) = wake_closed {
                            w();
                        }
                    }),
                );
                let cfg = RemoteProxyConfig {
                    host: "127.0.0.1".to_owned(),
                    port: proxy.port,
                    username: Some(proxy.credentials.username.clone()),
                    password: Some(proxy.credentials.password.clone()),
                    realm: Some(PROXY_REALM.to_owned()),
                };
                app.workspace_panes
                    .browser
                    .host
                    .set_partition_proxy(key.as_str(), Some(cfg));
                if let Some(mut prev) = mgr.tunnels.insert(
                    key,
                    ActiveTunnel { connection_id: want.connection_id, proxy },
                ) {
                    prev.proxy.close();
                }
            }
            Err(e) => {
                eprintln!("browser-rpc: failed to start remote tunnel proxy: {e}");
                app.workspace_panes.browser.host.set_partition_proxy(key.as_str(), None);
                if let Some(mut prev) = mgr.tunnels.remove(&key) {
                    prev.proxy.close();
                }
            }
        }
    }

    app.kernel.slots.get_mut(&BROWSER_RPC).tunnels = mgr;
}
