//! Profile cookie messages (`UP-095` Phase 3b): `CefCookieManager` list/import/delete, the startup
//! persistence probe, "Sign out everywhere", and the two-way remote cookie mirror.

use serde_json::json;

use super::EngineManager;
use crate::wire::HostMsg;

impl EngineManager {
    pub(super) fn handle_profile_cookies(&mut self, msg: HostMsg) {
        match msg {
            HostMsg::ProfileCookiesList { request_id, partition } => {
                self.cookies.list(&self.contexts, partition.as_deref(), request_id, self.out_tx.clone());
            }
            HostMsg::ProfileCookiesSet { request_id, partition, cookies } => {
                self.cookies.set_batch(&self.contexts, partition.as_deref(), cookies, request_id, self.out_tx.clone());
            }
            HostMsg::ProfileCookieDelete { request_id, partition, url, name } => {
                self.cookies.delete_one(&self.contexts, partition.as_deref(), &url, &name, request_id, self.out_tx.clone());
            }
            HostMsg::ProfileProbeCookies { request_id } => {
                self.cookies.probe_persistence(&self.profile_cache, request_id, self.out_tx.clone());
            }
            HostMsg::ClearProfileData { request_id } => {
                for t in self.tabs.values() {
                    if !matches!(
                        t.context,
                        crate::wire::ContextKind::Default | crate::wire::ContextKind::RemoteServer { .. }
                    ) {
                        continue;
                    }
                    let url = t.snap.lock().unwrap().url.clone();
                    if let Some(origin) = browser_policy::destination_origin(&url) {
                        self.send_internal_cdp(
                            &t.host,
                            "Storage.clearDataForOrigin",
                            json!({ "origin": origin, "storageTypes": "all" }),
                        );
                    }
                    self.send_internal_cdp(&t.host, "Network.clearBrowserCache", json!({}));
                }
                self.cookies.clear_all(&self.contexts, request_id, self.out_tx.clone());
            }
            HostMsg::SetShareSignIns { enabled } => {
                self.cookies.set_share_sign_ins(enabled, &self.contexts);
            }
            HostMsg::SyncRemoteCookies { request_id, partition, from_remote } => {
                let side = if from_remote {
                    crate::cookies::LinkSide::Other
                } else {
                    crate::cookies::LinkSide::Profile
                };
                self.cookies.sync_partition_now(&self.contexts, &partition, side, request_id, self.out_tx.clone());
            }
            _ => {}
        }
    }

    /// A finished page load may have changed cookies: marks its side of the mirror dirty, then
    /// runs the mirror's periodic reconcile.
    pub(super) fn poll_cookie_mirror(&mut self) {
        let mut profile_dirty = false;
        let mut remote_dirty: Vec<String> = Vec::new();
        for tab in self.tabs.values_mut() {
            let loading = tab.snap.lock().unwrap().loading;
            if tab.last_loading && !loading {
                match &tab.context {
                    crate::wire::ContextKind::Default => profile_dirty = true,
                    crate::wire::ContextKind::RemoteServer { server_id, .. } => remote_dirty.push(server_id.clone()),
                    crate::wire::ContextKind::Preview { .. }
                    | crate::wire::ContextKind::Artifact { .. }
                    | crate::wire::ContextKind::Private => {}
                }
            }
            tab.last_loading = loading;
        }
        if profile_dirty {
            self.cookies.on_profile_dirty(&self.contexts);
        }
        for partition in remote_dirty {
            self.cookies.on_partition_dirty(&self.contexts, &partition, crate::cookies::LinkSide::Other);
        }
        self.cookies.poll(&self.contexts);
    }

    /// A newly opened remote tab gets the profile's sign-ins when sharing is on.
    pub(super) fn seed_remote_partition(&mut self, context: &crate::wire::ContextKind) {
        if let crate::wire::ContextKind::RemoteServer { server_id, .. } = context
            && self.cookies.share_sign_ins()
        {
            self.cookies.on_partition_dirty(&self.contexts, server_id, crate::cookies::LinkSide::Profile);
        }
    }

    /// Releases `tab_id` from its pooled context; when that was a remote partition's last tab, the
    /// cookie mirror for that partition stops.
    pub(super) fn release_tab_context(&mut self, tab_id: u64) {
        if let Some(crate::context::ContextKey::RemoteServer(id)) = self.contexts.release_tab(tab_id) {
            self.cookies.on_partition_dropped(&id);
        }
    }
}
