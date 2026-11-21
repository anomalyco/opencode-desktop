//! `BrowserHost` profile cookie store, persistence probe, "Sign out everywhere", and two-way
//! remote cookie sharing hooks (`Phase 3b`).

use crate::client::BrowserHost;
use crate::cookies::ProfileEvent;
use crate::machine::EngineCaps;
use crate::protocol::{HostMsg, WireCookie};

impl BrowserHost {
    /// Whether the connected engine supports `CefCookieManager` profile cookie operations,
    /// persistence probing, "Sign out everywhere", and two-way remote sign-in mirroring.
    pub fn engine_cookies(&self) -> bool {
        matches!(self.caps, EngineCaps::Known { cookies: true, .. })
    }

    /// Whether two-way cookie sharing between the default profile and remote server partitions
    /// is currently enabled (off by default).
    pub fn share_sign_ins(&self) -> bool {
        self.share_sign_ins
    }

    /// Toggles two-way cookie sharing between the default profile and remote server partitions.
    /// Re-sent automatically across engine reconnects when enabled.
    pub fn set_share_sign_ins(&mut self, enabled: bool) {
        self.share_sign_ins = enabled;
        if self.engine_cookies() {
            self.send(HostMsg::SetShareSignIns { enabled });
        }
    }

    /// Requests a listing of all cookies in the default profile (or `Some(partition)` for a
    /// `RemoteServer` partition), excluding the internal persistence probe cookie.
    pub fn request_profile_cookies(&mut self, partition: Option<String>) -> Option<u64> {
        if !self.engine_cookies() {
            return None;
        }
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::ProfileCookiesList {
            request_id,
            partition,
        });
        Some(request_id)
    }

    /// Writes `cookies` into the default profile (or `Some(partition)`) and flushes the store.
    pub fn import_profile_cookies(
        &mut self,
        partition: Option<String>,
        cookies: Vec<WireCookie>,
    ) -> Option<u64> {
        if !self.engine_cookies() {
            return None;
        }
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::ProfileCookiesSet {
            request_id,
            partition,
            cookies,
        });
        Some(request_id)
    }

    /// Deletes a single cookie by URL and name and flushes the store.
    pub fn delete_profile_cookie(
        &mut self,
        partition: Option<String>,
        url: String,
        name: String,
    ) -> Option<u64> {
        if !self.engine_cookies() {
            return None;
        }
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::ProfileCookieDelete {
            request_id,
            partition,
            url,
            name,
        });
        Some(request_id)
    }

    /// Writes the short-lived persistence probe cookie and flushes the default cookie store so the
    /// host can call `cookies::database::probe_disk_cookies` on the returned `cache_dir` and then
    /// remove the probe cookie via `delete_profile_cookie`.
    pub fn probe_profile_cookies(&mut self) -> Option<u64> {
        if !self.engine_cookies() {
            return None;
        }
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::ProfileProbeCookies { request_id });
        Some(request_id)
    }

    /// "Sign out everywhere": clears all cookies, HTTP auth credentials, certificate exceptions,
    /// and site storage/caches across the default profile and every active remote server partition.
    pub fn clear_profile_data(&mut self) -> Option<u64> {
        if !self.engine_cookies() {
            return None;
        }
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::ClearProfileData { request_id });
        Some(request_id)
    }

    /// Explicitly reconciles a `RemoteServer` partition's cookies with the default profile.
    pub fn sync_remote_cookies(&mut self, partition: String, from_remote: bool) -> Option<u64> {
        if !self.engine_cookies() || !self.share_sign_ins {
            return None;
        }
        let request_id = self.next_req_id;
        self.next_req_id += 1;
        self.send(HostMsg::SyncRemoteCookies {
            request_id,
            partition,
            from_remote,
        });
        Some(request_id)
    }

    /// Drains pending profile/cookie engine responses queued since the last call.
    pub fn take_profile_events(&mut self) -> Vec<ProfileEvent> {
        self.profile_events.drain(..).collect()
    }
}
