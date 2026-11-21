//! Profile cookie messages (`UP-095` Phase 3b): `CefCookieManager` list/import/delete, the startup
//! persistence probe, "Sign out everywhere", and the two-way remote cookie mirror.

use serde_json::json;

use super::EngineManager;
use crate::wire::{EngineMsg, HostMsg};

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
                self.clear_every_site(request_id);
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

    /// "Clear browsing data" for every site, not only the open ones (official `profile/index.ts:523-540`):
    /// the shared profile's stored data of every site (`clear_storage`, through a hidden page when no
    /// tab of it is open), its HTTP cache, each remote server's stored data, and every cookie and HTTP
    /// sign-in (`CookieStore::clear_all`). `ProfileActionDone` comes once all of them have answered,
    /// `ok` only when each did.
    fn clear_every_site(&mut self, request_id: u64) {
        let out = self.out_tx.clone();
        let mut hosts: Vec<cef::BrowserHost> = Vec::new();
        let default_host = self.tabs.values().find(|t| matches!(t.context, crate::wire::ContextKind::Default)).map(|t| t.host.clone());
        for (partition, _) in self.contexts.remote_partitions() {
            let remote = self.tabs.values().find(|t| {
                matches!(&t.context, crate::wire::ContextKind::RemoteServer { server_id, .. } if *server_id == partition)
            });
            if let Some(t) = remote {
                hosts.push(t.host.clone());
            }
        }
        let keep_workers = self.extensions.as_ref().is_some_and(|e| e.installed_count() > 0);
        // Remote parts, the profile's storage, its HTTP cache, and the cookies.
        let join = Join::new(hosts.len() + 3, Box::new(move |ok| {
            let _ = out.send(EngineMsg::ProfileActionDone { request_id, ok });
        }));
        for host in &hosts {
            let id = self.next_cdp_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Remote servers' partitions hold no extension.
            crate::clear_storage::clear_all_sites(host, false, id, join.part());
        }
        let id = self.next_cdp_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match default_host {
            Some(host) => crate::clear_storage::clear_all_sites(&host, keep_workers, id, join.part()),
            None => crate::clear_storage::clear_all_sites_hidden(keep_workers, id, join.part()),
        }
        let cache = join.part();
        match cef::request_context_get_global_context() {
            Some(global) => {
                let mut cb = crate::cookies::completion_cb(move || cache(true));
                global.clear_http_cache(Some(&mut cb));
            }
            None => cache(false),
        }
        self.cookies.clear_all(&self.contexts, join.part());
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

/// Runs `done` once, when the last of its parts ends, with whether every part succeeded.
struct Join {
    left: std::sync::atomic::AtomicUsize,
    ok: std::sync::atomic::AtomicBool,
    done: std::sync::Mutex<Option<Box<dyn FnOnce(bool) + Send>>>,
}

impl Join {
    fn new(parts: usize, done: Box<dyn FnOnce(bool) + Send>) -> std::sync::Arc<Join> {
        std::sync::Arc::new(Join {
            left: std::sync::atomic::AtomicUsize::new(parts),
            ok: std::sync::atomic::AtomicBool::new(true),
            done: std::sync::Mutex::new(Some(done)),
        })
    }
}

trait JoinPart {
    fn part(&self) -> Box<dyn FnOnce(bool) + Send>;
}

impl JoinPart for std::sync::Arc<Join> {
    fn part(&self) -> Box<dyn FnOnce(bool) + Send> {
        let join = self.clone();
        Box::new(move |ok| {
            use std::sync::atomic::Ordering;
            if !ok {
                join.ok.store(false, Ordering::Release);
            }
            if join.left.fetch_sub(1, Ordering::AcqRel) == 1
                && let Some(done) = join.done.lock().unwrap().take()
            {
                done(join.ok.load(Ordering::Acquire));
            }
        })
    }
}
