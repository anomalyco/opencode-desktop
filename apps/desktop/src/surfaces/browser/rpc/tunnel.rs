//! Per-remote-server loopback [`TunnelProxy`] lifecycle synced to `BrowserHost::set_partition_proxy`
//! (WS-1 `#288`/`#292` `ContextKind::RemoteServer` contract + `review-rd-engine.md` point 4).
//!
//! Invariants:
//! - Every remote tunnel binds strictly to IP literal `127.0.0.1` (never a hostname) with fresh
//!   384-bit OS-CSPRNG Basic credentials (`realm = "OpenCode Browser Proxy"`).
//! - On rotation, the new proxy binds while the prior listener is still open (so the OS cannot
//!   recycle the port), updates `BrowserHost::set_partition_proxy(server_id, Some(new_cfg))`
//!   atomically, and only then closes the prior proxy — never reusing credentials across ports and
//!   never force-closing live remote tabs during normal reconnect.
//! - During transient reconnect (`Connecting`/`Backoff` after a prior `Attached` state), the old
//!   proxy is suspended (`503 Service Unavailable`, zero egress) instead of sending
//!   `SetPartitionProxy(None)`. On permanent detach/close/block/unsupported/unavailable,
//!   `BrowserHost::set_partition_proxy(server_id, None)` and `proxy.close()` run immediately.

use std::collections::{BTreeMap, BTreeSet};

use browser_pane::RemoteProxyConfig;
use opencode_kit::browser_rpc::{AttachState, LocationRef, PROXY_REALM, TunnelProxy};
use opencode_kit::client::Server;
use opencode_kit::servers::ServerKey;

use super::BROWSER_RPC;
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
    connection_id: String,
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

    // Pick one live attached session per remote server as the tunnel carrier, and note remote
    // servers currently in transient reconnect (`Connecting`/`Backoff` after prior attach).
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
                && att.attach.is_attached()
            {
                desired.entry(key.clone()).or_insert_with(|| DesiredTunnel {
                    server: att.server.clone(),
                    location: att.location.clone(),
                    session_id: sid.clone(),
                    connection_id: conn_id.to_owned(),
                });
            } else if !sse_up
                || matches!(
                    att.attach.state,
                    AttachState::Connecting { .. } | AttachState::Backoff { .. }
                )
            {
                reconnecting.insert(key.clone());
            }
        }
    }

    // Move manager out temporarily so we can mutate `BrowserHost`.
    let mut mgr = std::mem::take(&mut app.kernel.slots.get_mut(&BROWSER_RPC).tunnels);

    // 1. Suspend tunnels in transient reconnect (`503`, zero egress, keeps tabs alive), and stop
    //    tunnels for remote servers that permanently detached, closed, blocked, or lost engine support.
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

    // 2. Start or rotate tunnels whenever a remote server attaches with a new `connection_id`.
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
        // Start the replacement BEFORE closing `prev` so the OS cannot recycle the prior port
        // and the engine switches atomically from `Some(old)` to `Some(new)`.
        match TunnelProxy::start(
            want.server,
            want.location,
            want.session_id,
            want.connection_id.clone(),
        ) {
            Ok(proxy) => {
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
