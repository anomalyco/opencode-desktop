//! Preview grant registry, random origin minting, display URL translation, and async remote /
//! artifact fetch coordination for `oc-preview://` tabs.

pub mod context;
pub mod handle_path;
pub mod handler;
pub mod scheme;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use browser_policy::preview::{
    MAX_ARTIFACT_BYTES, MAX_PREVIEW_FILE_BYTES, artifact_resource, is_preview_scheme,
    local_display_url, parse_preview_url, preview_allowed, preview_url,
};
use cef::{Callback, ImplCallback, ImplResourceReadCallback, ResourceReadCallback};

use crate::output::Output;
use crate::wire::{EngineMsg, PreviewKindTag, PreviewSpec};
use handle_path::RootPin;

/// Maximum pending initiator entries remembered per grant (FIFO eviction prevents a fetch flood
/// from wiping in-flight initiator records).
const MAX_INITIATOR_CACHE: usize = 4096;

#[derive(Clone, Debug)]
pub enum PreviewKind {
    Local {
        root: String,
        pin: RootPin,
        document: String,
        home: Option<String>,
    },
    Remote {
        root: String,
        session_prefix: String,
        document: String,
    },
    Artifact {
        artifact_id: String,
    },
}

impl PreviewKind {
    pub fn tag(&self) -> PreviewKindTag {
        match self {
            Self::Local { .. } => PreviewKindTag::Local,
            Self::Remote { .. } => PreviewKindTag::Remote,
            Self::Artifact { .. } => PreviewKindTag::Artifact,
        }
    }

    pub fn max_bytes(&self) -> u64 {
        match self {
            Self::Artifact { .. } => MAX_ARTIFACT_BYTES,
            Self::Local { .. } | Self::Remote { .. } => MAX_PREVIEW_FILE_BYTES,
        }
    }
}

/// One live preview tab's isolated grant. Revoked synchronously when the tab closes or navigates
/// away; every local open and async remote/artifact response re-checks `is_revoked()`.
pub struct PreviewGrant {
    pub tab_id: u64,
    pub grant_host: String,
    pub kind: PreviewKind,
    revoked: AtomicBool,
    entry_taken: AtomicBool,
    notified_denied: AtomicBool,
    last_display: Mutex<String>,
    /// Request identifier -> whether Chromium's non-spoofable `request_initiator` was empty
    /// (browser-initiated top-level navigation, eligible for the one-shot entry token).
    initiators: Mutex<InitiatorCache>,
}

#[derive(Default)]
struct InitiatorCache {
    map: HashMap<u64, InitiatorRecord>,
    order: VecDeque<u64>,
}

#[derive(Clone, Copy, Debug)]
pub struct InitiatorRecord {
    pub browser_initiated: bool,
    pub allowed: bool,
}

impl PreviewGrant {
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }

    pub fn is_artifact(&self) -> bool {
        matches!(self.kind, PreviewKind::Artifact { .. })
    }

    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }

    /// Consumes the one-shot entry token allowing `.well-known/index.html` to load itself once
    /// without opening sibling dotfiles (`workspace-pages.ts:61-66`).
    pub fn take_entry(&self) -> bool {
        !self.entry_taken.swap(true, Ordering::AcqRel)
    }

    pub fn mark_denied(&self, out_tx: &Output) {
        if !self.is_revoked() && !self.notified_denied.swap(true, Ordering::AcqRel) {
            let _ = out_tx.send(EngineMsg::PreviewDenied { tab_id: self.tab_id });
        }
    }

    pub fn record_initiator(&self, req_id: u64, browser_initiated: bool, allowed: bool) {
        let mut cache = self.initiators.lock().unwrap();
        while cache.map.len() >= MAX_INITIATOR_CACHE {
            let Some(oldest) = cache.order.pop_front() else { break };
            cache.map.remove(&oldest);
        }
        if cache.map.insert(req_id, InitiatorRecord { browser_initiated, allowed }).is_none() {
            cache.order.push_back(req_id);
        }
    }

    pub fn take_initiator(&self, req_id: u64) -> Option<InitiatorRecord> {
        self.initiators.lock().unwrap().map.remove(&req_id)
    }

    pub fn last_display_url(&self) -> String {
        self.last_display.lock().unwrap().clone()
    }

    /// Pure read-only translation of an internal `oc-preview://<grant_host>/...` URL into its
    /// clean display URL, or `None` if it fails scope/grant validation.
    fn compute_display_url(&self, raw_url: &str) -> Option<String> {
        if self.is_revoked() {
            return None;
        }
        let out = match &self.kind {
            PreviewKind::Local { root, document, home, .. } => {
                let (rel, suffix) = parse_preview_url(raw_url, &self.grant_host, cfg!(windows))?;
                let same = rel == *document;
                if !preview_allowed(root, document, &rel, same, home.as_deref()) {
                    return None;
                }
                local_display_url(root, &rel, &suffix)
            }
            PreviewKind::Remote { root, session_prefix, document } => {
                let (rel, suffix) = parse_preview_url(raw_url, &self.grant_host, false)?;
                let same = rel == *document;
                if !preview_allowed(root, document, &rel, same, None) {
                    return None;
                }
                format!("{session_prefix}{}{suffix}", browser_policy::preview::encode_rel_path(&rel))
            }
            PreviewKind::Artifact { artifact_id } => {
                let (display, _) = artifact_resource(raw_url, &self.grant_host, artifact_id)?;
                display
            }
        };
        Some(out)
    }

    /// Translates an internal `oc-preview://<grant_host>/...` URL into its clean display URL
    /// (`file:///...`, `workspace://...`, or `artifact://...`) and caches it as `last_display`.
    /// Never returns `oc-preview://`.
    pub fn display_url(&self, raw_url: &str) -> Option<String> {
        let out = self.compute_display_url(raw_url)?;
        *self.last_display.lock().unwrap() = out.clone();
        Some(out)
    }

    /// Whether a main-frame navigation to `raw_url` is allowed to stay inside this preview tab.
    pub fn allows_main_navigation(&self, raw_url: &str) -> bool {
        if self.is_revoked() {
            return false;
        }
        if raw_url.trim().eq_ignore_ascii_case("about:blank") {
            return true;
        }
        self.compute_display_url(raw_url).is_some()
    }

    /// Whether a subframe inside this preview tab may navigate to `raw_url`.
    /// Artifacts forbid `data:`/`blob:` subframes (matching `ARTIFACT_CSP`).
    pub fn allows_subframe_navigation(&self, raw_url: &str) -> bool {
        if self.is_revoked() {
            return false;
        }
        if self.is_artifact() {
            let t = raw_url.trim();
            if t.eq_ignore_ascii_case("about:blank") {
                return true;
            }
            return self.compute_display_url(t).is_some();
        }
        browser_policy::preview::preview_subresource_allowed(raw_url, &self.grant_host)
    }

    /// Sanitizes any URL reported by CEF (`on_address_change`, `on_load_error`) so the internal
    /// random `oc-preview://<uuid>` origin never escapes into `TabSnapshot`.
    pub fn sanitize_reported_url(&self, raw_url: &str) -> String {
        if let Some(display) = self.display_url(raw_url) {
            return display;
        }
        let own = format!("{}://{}/", scheme::PREVIEW_SCHEME, self.grant_host);
        if raw_url.len() >= own.len() && raw_url.as_bytes()[..own.len()].eq_ignore_ascii_case(own.as_bytes()) {
            let last = self.last_display_url();
            if !last.is_empty() {
                return last;
            }
        }
        if is_preview_scheme(raw_url) || browser_policy::is_file_scheme(raw_url) {
            return "about:blank".into();
        }
        raw_url.to_owned()
    }
}

/// Shared state between an in-flight async `ProxyResourceHandler` (CEF IO thread) and the
/// engine main thread receiving `HostMsg::PreviewResponse*` frames.
pub struct PendingProxy {
    pub tab_id: u64,
    pub grant: Arc<PreviewGrant>,
    pub head_only: bool,
    pub state: Mutex<ProxyStreamState>,
}

pub struct ProxyStreamState {
    pub canceled: bool,
    pub head_ready: bool,
    pub head_ok: bool,
    pub status: u16,
    pub mime: String,
    pub charset: Option<String>,
    pub content_length: Option<u64>,
    pub content_range: Option<String>,
    pub open_callback: Option<Callback>,
    pub chunks: VecDeque<Vec<u8>>,
    pub total_received: u64,
    pub ended: bool,
    pub end_ok: bool,
    pub waiting_read: Option<WaitingRead>,
}

pub struct WaitingRead {
    pub data_out: usize,
    pub max_len: usize,
    pub callback: ResourceReadCallback,
}

impl PendingProxy {
    pub fn new(tab_id: u64, grant: Arc<PreviewGrant>, head_only: bool) -> Arc<Self> {
        Arc::new(Self {
            tab_id,
            grant,
            head_only,
            state: Mutex::new(ProxyStreamState {
                canceled: false,
                head_ready: false,
                head_ok: false,
                status: 500,
                mime: "application/octet-stream".into(),
                charset: None,
                content_length: None,
                content_range: None,
                open_callback: None,
                chunks: VecDeque::new(),
                total_received: 0,
                ended: false,
                end_ok: false,
                waiting_read: None,
            }),
        })
    }

    pub fn cancel(&self) {
        let (open_cb, read_cb) = {
            let mut st = self.state.lock().unwrap();
            st.canceled = true;
            st.ended = true;
            st.end_ok = false;
            st.chunks.clear();
            (st.open_callback.take(), st.waiting_read.take())
        };
        if let Some(cb) = open_cb {
            cb.cancel();
        }
        if let Some(wait) = read_cb {
            wait.callback.cont(-2);
        }
    }

    pub fn deliver_head(
        &self,
        ok: bool,
        status: u16,
        mime: &str,
        content_length: Option<u64>,
        content_range: Option<String>,
    ) {
        let open_cb = {
            let mut st = self.state.lock().unwrap();
            if st.canceled || self.grant.is_revoked() {
                st.canceled = true;
                st.open_callback.take()
            } else {
                let over_cap = content_length.is_some_and(|n| n > self.grant.kind.max_bytes());
                let valid_status = matches!(status, 200 | 206 | 416);
                st.head_ready = true;
                st.head_ok = ok && valid_status && !over_cap;
                st.status = status;
                let (clean_mime, charset) = browser_policy::preview::sanitize_content_type(mime);
                st.mime = clean_mime;
                st.charset = charset;
                st.content_length = content_length;
                st.content_range = content_range;
                st.open_callback.take()
            }
        };
        if let Some(cb) = open_cb {
            let ok_now = {
                let st = self.state.lock().unwrap();
                st.head_ok && !st.canceled && !self.grant.is_revoked()
            };
            if ok_now {
                cb.cont();
            } else {
                cb.cancel();
            }
        }
    }

    pub fn deliver_chunk(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let wake = {
            let mut st = self.state.lock().unwrap();
            if st.canceled || st.ended || self.grant.is_revoked() {
                return;
            }
            st.total_received = st.total_received.saturating_add(bytes.len() as u64);
            if st.total_received > self.grant.kind.max_bytes() {
                st.canceled = true;
                st.ended = true;
                st.end_ok = false;
                st.chunks.clear();
                st.waiting_read.take().map(|w| (w.callback, -2i32))
            } else if let Some(wait) = st.waiting_read.take() {
                let n = bytes.len().min(wait.max_len);
                // SAFETY: CEF guarantees `data_out` remains valid until `wait.callback.cont` runs.
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), wait.data_out as *mut u8, n);
                }
                if n < bytes.len() {
                    st.chunks.push_front(bytes[n..].to_vec());
                }
                Some((wait.callback, n as i32))
            } else {
                st.chunks.push_back(bytes);
                None
            }
        };
        if let Some((cb, n)) = wake {
            cb.cont(n);
        }
    }

    pub fn deliver_end(&self, ok: bool) {
        let wake = {
            let mut st = self.state.lock().unwrap();
            st.ended = true;
            st.end_ok = ok && !st.canceled && !self.grant.is_revoked();
            if !st.end_ok {
                st.chunks.clear();
            }
            if st.chunks.is_empty() {
                st.waiting_read.take().map(|w| (w.callback, if st.end_ok { 0i32 } else { -2i32 }))
            } else {
                None
            }
        };
        if let Some((cb, code)) = wake {
            cb.cont(code);
        }
    }
}

/// Engine-wide preview grant & async proxy fetch registry.
pub struct PreviewRegistry {
    next_req_id: AtomicU64,
    proxies: Mutex<HashMap<u64, Arc<PendingProxy>>>,
}

impl PreviewRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_req_id: AtomicU64::new(1),
            proxies: Mutex::new(HashMap::new()),
        })
    }

    /// Creates a new isolated grant from `spec`, pinning the local workspace root immediately.
    /// Returns `(grant, internal_oc_preview_url, initial_display_url)`.
    pub fn create_grant(
        &self,
        tab_id: u64,
        spec: PreviewSpec,
        home: Option<&str>,
    ) -> Result<(Arc<PreviewGrant>, String, String), &'static str> {
        let grant_host = random_grant_host();
        match spec {
            PreviewSpec::Local { root, document } => {
                let doc = document.replace('\\', "/");
                let doc = doc.trim_start_matches('/').to_owned();
                if !browser_policy::preview::preview_path_valid(&doc, cfg!(windows))
                    || !preview_allowed(&root, &doc, &doc, true, home)
                {
                    return Err("browser.address.workspace");
                }
                let Some(pin) = handle_path::pin_root(&root, home) else {
                    return Err("browser.address.workspace");
                };
                let internal = preview_url(&grant_host, &doc, "");
                let display = local_display_url(&root, &doc, "");
                let grant = Arc::new(PreviewGrant {
                    tab_id,
                    grant_host,
                    kind: PreviewKind::Local { root, pin, document: doc, home: home.map(str::to_owned) },
                    revoked: AtomicBool::new(false),
                    entry_taken: AtomicBool::new(false),
                    notified_denied: AtomicBool::new(false),
                    last_display: Mutex::new(display.clone()),
                    initiators: Mutex::new(InitiatorCache::default()),
                });
                Ok((grant, internal, display))
            }
            PreviewSpec::Remote { root, session_prefix, document } => {
                let doc = document.replace('\\', "/");
                let doc = doc.trim_start_matches('/').to_owned();
                let prefix = session_prefix.to_ascii_lowercase();
                if !prefix.starts_with("workspace://") || !prefix.ends_with('/') {
                    return Err("browser.address.workspace");
                }
                if !browser_policy::preview::preview_path_valid(&doc, false)
                    || !preview_allowed(&root, &doc, &doc, true, None)
                {
                    return Err("browser.address.workspace");
                }
                let internal = preview_url(&grant_host, &doc, "");
                let display = format!("{prefix}{}", browser_policy::preview::encode_rel_path(&doc));
                let grant = Arc::new(PreviewGrant {
                    tab_id,
                    grant_host,
                    kind: PreviewKind::Remote { root, session_prefix: prefix, document: doc },
                    revoked: AtomicBool::new(false),
                    entry_taken: AtomicBool::new(false),
                    notified_denied: AtomicBool::new(false),
                    last_display: Mutex::new(display.clone()),
                    initiators: Mutex::new(InitiatorCache::default()),
                });
                Ok((grant, internal, display))
            }
            PreviewSpec::Artifact { artifact_id, path } => {
                if !browser_policy::preview::is_valid_artifact_id(&artifact_id) {
                    return Err("browser.address.web");
                }
                let clean = path.trim_start_matches('/');
                let encoded = if clean.is_empty() {
                    String::new()
                } else {
                    if !browser_policy::preview::preview_path_valid(clean, true) {
                        return Err("browser.address.web");
                    }
                    browser_policy::preview::encode_rel_path(clean)
                };
                let internal = format!("{}://{grant_host}/{encoded}", scheme::PREVIEW_SCHEME);
                let display = format!("artifact://{artifact_id}/{encoded}");
                let grant = Arc::new(PreviewGrant {
                    tab_id,
                    grant_host,
                    kind: PreviewKind::Artifact { artifact_id },
                    revoked: AtomicBool::new(false),
                    entry_taken: AtomicBool::new(false),
                    notified_denied: AtomicBool::new(false),
                    last_display: Mutex::new(display.clone()),
                    initiators: Mutex::new(InitiatorCache::default()),
                });
                Ok((grant, internal, display))
            }
        }
    }

    pub fn register_proxy(&self, tab_id: u64, grant: Arc<PreviewGrant>, head_only: bool) -> (u64, Arc<PendingProxy>) {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let proxy = PendingProxy::new(tab_id, grant, head_only);
        self.proxies.lock().unwrap().insert(req_id, proxy.clone());
        (req_id, proxy)
    }

    pub fn remove_proxy(&self, req_id: u64) {
        self.proxies.lock().unwrap().remove(&req_id);
    }

    pub fn cancel_tab_proxies(&self, tab_id: u64) {
        let doomed: Vec<Arc<PendingProxy>> = {
            let mut map = self.proxies.lock().unwrap();
            let ids: Vec<u64> = map.iter().filter(|(_, p)| p.tab_id == tab_id).map(|(&k, _)| k).collect();
            ids.into_iter().filter_map(|id| map.remove(&id)).collect()
        };
        for p in doomed {
            p.cancel();
        }
    }

    pub fn on_response_head(
        &self,
        req_id: u64,
        ok: bool,
        status: u16,
        mime: &str,
        content_length: Option<u64>,
        content_range: Option<String>,
    ) {
        let proxy = self.proxies.lock().unwrap().get(&req_id).cloned();
        if let Some(p) = proxy {
            p.deliver_head(ok, status, mime, content_length, content_range);
        }
    }

    pub fn on_response_chunk(&self, req_id: u64, data_b64: &str) {
        use base64::Engine as _;
        let proxy = self.proxies.lock().unwrap().get(&req_id).cloned();
        if let Some(p) = proxy {
            match base64::engine::general_purpose::STANDARD.decode(data_b64) {
                Ok(bytes) => p.deliver_chunk(bytes),
                Err(_) => {
                    p.cancel();
                    self.remove_proxy(req_id);
                }
            }
        }
    }

    pub fn on_response_end(&self, req_id: u64, ok: bool) {
        let proxy = self.proxies.lock().unwrap().remove(&req_id);
        if let Some(p) = proxy {
            p.deliver_end(ok);
        }
    }
}

/// Generates a 128-bit lowercase hex random host using OS CSPRNG (`BCryptGenRandom` on Windows,
/// `/dev/urandom` on Unix) so preview origins are unguessable across tabs and sessions.
fn random_grant_host() -> String {
    let mut bytes = [0u8; 16];
    #[cfg(windows)]
    {
        use windows::Win32::Security::Cryptography::{
            BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
        };
        let status = unsafe {
            BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
        };
        if status.is_err() {
            fallback_entropy(&mut bytes);
        }
    }
    #[cfg(unix)]
    {
        use std::io::Read as _;
        if std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .is_err()
        {
            fallback_entropy(&mut bytes);
        }
    }
    // Format as UUIDv4 hex (`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`).
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

fn fallback_entropy(bytes: &mut [u8; 16]) {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher as _, Hasher as _};
    let a = RandomState::new().build_hasher().finish();
    let b = RandomState::new().build_hasher().finish();
    bytes[..8].copy_from_slice(&a.to_le_bytes());
    bytes[8..].copy_from_slice(&b.to_le_bytes());
}
