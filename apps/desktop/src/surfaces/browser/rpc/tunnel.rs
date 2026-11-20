//! Per-remote-server loopback [`TunnelProxy`] lifecycle synced to `BrowserHost::set_partition_proxy`
//! (WS-1 `#288`/`#292` `ContextKind::RemoteServer` contract + `review-rd-engine.md` point 4).
//!
//! Invariants:
//! - Every remote tunnel binds strictly to IP literal `127.0.0.1` (never a hostname) with fresh
//!   384-bit OS-CSPRNG Basic credentials (`realm = "OpenCode Browser Proxy"`).
//! - Every stop, detach, disconnect, or `connection_id` rotation immediately calls
//!   `BrowserHost::set_partition_proxy(server_id, None)` (or the rotated `Some(new_cfg)` after
//!   closing the prior proxy) so a credential is never reused across a recycled port and no remote
//!   tab can ever run unproxied.

use std::collections::BTreeMap;

use browser_pane::RemoteProxyConfig;
use opencode_kit::browser_rpc::{LocationRef, PROXY_REALM, TunnelProxy};
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

/// Reconciles active [`TunnelProxy`] instances against currently attached remote-server sessions
/// and updates `BrowserHost::set_partition_proxy` on every start, rotation, or stop.
pub(super) fn sync_remote_tunnels(app: &mut App) {
    let engine_known = app.workspace_panes.browser.host.engine_caps_known();
    let engine_remote_ok = !engine_known || app.workspace_panes.browser.host.engine_remote_proxy();

    // Pick one live attached session per remote server as the tunnel carrier.
    let mut desired: BTreeMap<ServerKey, DesiredTunnel> = BTreeMap::new();
    if engine_remote_ok {
        let st = app.kernel.slots.get(&BROWSER_RPC);
        for ((key, sid), att) in &st.sessions {
            let same_machine = key.is_sidecar() && att.server.is_local();
            if same_machine {
                continue;
            }
            if let Some(conn_id) = att.attach.connection_id()
                && att.attach.is_attached()
            {
                desired.entry(key.clone()).or_insert_with(|| DesiredTunnel {
                    server: att.server.clone(),
                    location: att.location.clone(),
                    session_id: sid.clone(),
                    connection_id: conn_id.to_owned(),
                });
            }
        }
    }

    // Move manager out temporarily so we can mutate `BrowserHost`.
    let mut mgr = std::mem::take(&mut app.kernel.slots.get_mut(&BROWSER_RPC).tunnels);

    // 1. Stop tunnels for remote servers that detached, disconnected, or lost engine support.
    let stale: Vec<ServerKey> = mgr
        .tunnels
        .keys()
        .filter(|k| !desired.contains_key(*k))
        .cloned()
        .collect();
    for key in stale {
        if let Some(mut old) = mgr.tunnels.remove(&key) {
            app.workspace_panes.browser.host.set_partition_proxy(key.as_str(), None);
            old.proxy.close();
        }
    }

    // 2. Start or rotate tunnels whenever a remote server attaches with a new `connection_id`.
    for (key, want) in desired {
        if mgr
            .tunnels
            .get(&key)
            .is_some_and(|cur| cur.connection_id == want.connection_id)
        {
            continue;
        }
        // Always tear down the previous proxy and notify the engine before binding a new port so
        // credentials are never reused across a recycled loopback port.
        if let Some(mut prev) = mgr.tunnels.remove(&key) {
            app.workspace_panes.browser.host.set_partition_proxy(key.as_str(), None);
            prev.proxy.close();
        }
        match TunnelProxy::start(
            want.server,
            want.location,
            want.session_id,
            want.connection_id.clone(),
        ) {
            Ok(proxy) => {
                // Register strictly with IP literal `127.0.0.1` + high-entropy Basic credentials
                // (required by WS-1 `#292` `validate_proxy`).
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
                mgr.tunnels.insert(
                    key,
                    ActiveTunnel { connection_id: want.connection_id, proxy },
                );
            }
            Err(e) => {
                eprintln!("browser-rpc: failed to start remote tunnel proxy: {e}");
                app.workspace_panes.browser.host.set_partition_proxy(key.as_str(), None);
            }
        }
    }

    app.kernel.slots.get_mut(&BROWSER_RPC).tunnels = mgr;
}
