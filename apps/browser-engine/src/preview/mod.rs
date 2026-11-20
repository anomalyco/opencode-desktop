//! Preview grant registry, CSPRNG origin minting, display URL translation, bounded initiator
//! cache, off-IO-thread local worker pool, and async remote/artifact proxy coordination for
//! `oc-preview://` tabs.

pub mod context;
pub mod handle_path;
pub mod handler;
pub mod io_pool;
pub mod scheme;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use browser_policy::preview::{
    MAX_ARTIFACT_BYTES, MAX_PREVIEW_FILE_BYTES, artifact_resource, is_preview_scheme,
    local_display_url, mime_for_path, parse_content_range, parse_preview_url, preview_allowed,
    preview_url, root_is_windows,
};
use cef::{Callback, ImplCallback, ImplResourceReadCallback, ResourceReadCallback};

use crate::output::Output;
use crate::wire::{ContextKind, EngineMsg, PreviewKindTag, PreviewSpec};
use handle_path::RootPin;
use io_pool::PreviewIoPool;

/// Maximum pending initiator entries remembered per grant (strictly bounded in both map and queue).
const MAX_INITIATOR_CACHE: usize = 4096;
/// Maximum unread buffered bytes per in-flight remote/artifact proxy request (backpressure cap).
const MAX_PROXY_QUEUE_BYTES: usize = 8 * 1024 * 1024;

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
    /// Becomes `true` ONLY after `on_request_context_initialized` has applied & verified lockdown prefs.
    ready: AtomicBool,
    entry_taken: AtomicBool,
    notified_denied: AtomicBool,
    /// Count of in-flight + queued `PreviewIoPool` tasks owned by this grant (capped at `MAX_GRANT_IO_JOBS`).
    in_flight_io: AtomicUsize,
    last_display: Mutex<String>,
    /// Strictly bounded request-id -> initiator map (populated only for allowed same-grant requests).
    initiators: Mutex<InitiatorCache>,
}

#[derive(Default)]
struct InitiatorCache {
    map: HashMap<u64, (u64, InitiatorRecord)>,
    order: VecDeque<(u64, u64)>,
    next_seq: u64,
}

impl InitiatorCache {
    fn prune_front(&mut self) {
        while let Some(&(seq, req_id)) = self.order.front() {
            if self.map.get(&req_id).is_some_and(|(cur_seq, _)| *cur_seq == seq) {
                break;
            }
            self.order.pop_front();
        }
    }

    fn insert(&mut self, req_id: u64, rec: InitiatorRecord) {
        self.prune_front();
        while self.map.len() >= MAX_INITIATOR_CACHE {
            self.prune_front();
            let Some((seq, oldest_id)) = self.order.pop_front() else {
                self.map.clear();
                break;
            };
            if self.map.get(&oldest_id).is_some_and(|(cur_seq, _)| *cur_seq == seq) {
                self.map.remove(&oldest_id);
            }
        }
        self.next_seq = self.next_seq.wrapping_add(1);
        let seq = self.next_seq;
        self.map.insert(req_id, (seq, rec));
        self.order.push_back((seq, req_id));
        if self.order.len() > MAX_INITIATOR_CACHE * 2 {
            let map = &self.map;
            self.order.retain(|&(s, id)| map.get(&id).is_some_and(|(cur, _)| *cur == s));
        }
    }

    fn take(&mut self, req_id: u64) -> Option<InitiatorRecord> {
        let rec = self.map.remove(&req_id).map(|(_, r)| r);
        self.prune_front();
        rec
    }
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

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire) && !self.is_revoked()
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    pub fn try_acquire_io_slot(&self, cap: usize) -> bool {
        let mut cur = self.in_flight_io.load(Ordering::Acquire);
        loop {
            if cur >= cap {
                return false;
            }
            match self.in_flight_io.compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(next) => cur = next,
            }
        }
    }

    pub fn release_io_slot(&self) {
        self.in_flight_io.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn is_artifact(&self) -> bool {
        matches!(self.kind, PreviewKind::Artifact { .. })
    }

    /// Canonical `ContextPool` scope key matching WS-1 `#288` (`Preview { workspace, grant }`,
    /// `Artifact { artifact_id }`). Every tab gets a disjoint context keyed with its unique grant.
    pub fn context_kind(&self) -> ContextKind {
        match &self.kind {
            PreviewKind::Local { pin, .. } => ContextKind::Preview {
                workspace: pin.canonical_path.clone(),
                grant: self.grant_host.clone(),
            },
            PreviewKind::Remote { root, .. } => ContextKind::Preview {
                workspace: root.clone(),
                grant: self.grant_host.clone(),
            },
            PreviewKind::Artifact { artifact_id } => ContextKind::Artifact {
                artifact_id: format!("{artifact_id}:{}", self.grant_host),
            },
        }
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
        if req_id == 0 {
            return;
        }
        self.initiators
            .lock()
            .unwrap()
            .insert(req_id, InitiatorRecord { browser_initiated, allowed });
    }

    pub fn take_initiator(&self, req_id: u64) -> Option<InitiatorRecord> {
        if req_id == 0 {
            return None;
        }
        self.initiators.lock().unwrap().take(req_id)
    }

    pub fn last_display_url(&self) -> String {
        self.last_display.lock().unwrap().clone()
    }

    /// Pure read-only translation of an internal `oc-preview://<grant_host>/...` URL into its
    /// clean display URL, or `None` if it fails scope/grant validation.
    pub fn compute_display_url(&self, raw_url: &str) -> Option<String> {
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
                let (rel, suffix) =
                    parse_preview_url(raw_url, &self.grant_host, root_is_windows(root))?;
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
    /// Artifacts allow only `about:blank` or same-grant `oc-preview://` (no `data:`/`blob:`);
    /// workspace previews allow `about:blank`, `about:srcdoc`, same-origin `blob:`, and same-grant
    /// `oc-preview://` (never `data:` subframes).
    pub fn allows_subframe_navigation(&self, raw_url: &str) -> bool {
        if self.is_revoked() {
            return false;
        }
        let t = raw_url.trim();
        if t.eq_ignore_ascii_case("about:blank") {
            return true;
        }
        if self.is_artifact() {
            return self.compute_display_url(t).is_some();
        }
        if t.eq_ignore_ascii_case("about:srcdoc") {
            return true;
        }
        let own_blob = format!("blob:{}://{}/", scheme::PREVIEW_SCHEME, self.grant_host);
        if t.len() >= own_blob.len() && t.as_bytes()[..own_blob.len()].eq_ignore_ascii_case(own_blob.as_bytes()) {
            return true;
        }
        self.compute_display_url(t).is_some()
    }

    /// Sanitizes any URL reported by CEF (`on_address_change`, `on_load_error`, context menu, JS
    /// dialogs) so the internal random `oc-preview://<uuid>` origin (and the pre-init deferred
    /// `about:blank` placeholder) never escapes into `TabSnapshot` or overwrites the display URL.
    pub fn sanitize_reported_url(&self, raw_url: &str) -> String {
        if let Some(display) = self.display_url(raw_url) {
            return display;
        }
        let trimmed = raw_url.trim();
        let own = format!("{}://{}/", scheme::PREVIEW_SCHEME, self.grant_host);
        if trimmed.eq_ignore_ascii_case("about:blank")
            || (trimmed.len() >= own.len()
                && trimmed.as_bytes()[..own.len()].eq_ignore_ascii_case(own.as_bytes()))
        {
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
    /// For `PreviewKind::Remote`, verified workspace-relative path used to derive MIME strictly
    /// from extension (never trusting server-supplied `Content-Type`). `None` for artifacts.
    pub remote_rel: Option<String>,
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
    pub total_size: Option<u64>,
    pub content_range: Option<String>,
    pub open_callback: Option<Callback>,
    pub chunks: VecDeque<Vec<u8>>,
    pub queued_bytes: usize,
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
    pub fn new(tab_id: u64, grant: Arc<PreviewGrant>, head_only: bool, remote_rel: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            tab_id,
            grant,
            head_only,
            remote_rel,
            state: Mutex::new(ProxyStreamState {
                canceled: false,
                head_ready: false,
                head_ok: false,
                status: 404,
                mime: "application/octet-stream".into(),
                charset: None,
                content_length: None,
                total_size: None,
                content_range: None,
                open_callback: None,
                chunks: VecDeque::new(),
                queued_bytes: 0,
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
            st.queued_bytes = 0;
            (st.open_callback.take(), st.waiting_read.take())
        };
        if let Some(cb) = open_cb {
            cb.cancel();
        }
        if let Some(wait) = read_cb {
            wait.callback.cont(-2);
        }
    }

    /// Applies response headers; returns `true` if the proxy failed before `open()` and can be
    /// removed from `PreviewRegistry::proxies` immediately.
    pub fn deliver_head(
        &self,
        ok: bool,
        status: u16,
        mime: &str,
        content_length: Option<u64>,
        content_range: Option<String>,
    ) -> bool {
        let max_bytes = self.grant.kind.max_bytes();
        let is_artifact = self.grant.is_artifact();
        let (open_cb, failed_early) = {
            let mut st = self.state.lock().unwrap();
            if st.canceled || self.grant.is_revoked() {
                st.canceled = true;
                let cb = st.open_callback.take();
                (cb, true)
            } else {
                let over_cap = content_length.is_some_and(|n| n > max_bytes);
                let (valid_status, total_size) = if is_artifact {
                    // Upstream `serve.ts:25`: artifacts require strictly 200 OK.
                    (status == 200, content_length)
                } else {
                    match status {
                        200 => (true, content_length),
                        206 => match parse_content_range(content_range.as_deref()) {
                            Some((start, end, total))
                                if total <= max_bytes
                                    && content_length == Some(end - start + 1) =>
                            {
                                (true, Some(total))
                            }
                            _ => (false, None),
                        },
                        416 => (true, Some(0)),
                        _ => (false, None),
                    }
                };
                st.head_ready = true;
                st.head_ok = ok && valid_status && !over_cap;
                st.status = status;
                if let Some(rel) = &self.remote_rel {
                    // Remote workspace files derive MIME strictly from extension (`serve.ts:78`).
                    let (ext_mime, ext_cs) = mime_for_path(rel);
                    st.mime = ext_mime.to_owned();
                    st.charset = ext_cs.map(str::to_owned);
                } else {
                    let (clean_mime, charset) = browser_policy::preview::sanitize_content_type(mime);
                    st.mime = clean_mime;
                    st.charset = charset;
                }
                st.content_length = content_length;
                st.total_size = total_size;
                st.content_range = content_range;
                let cb = st.open_callback.take();
                let should_remove = !st.head_ok;
                (cb, should_remove)
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
        failed_early
    }

    /// Appends a chunk; returns `true` if the proxy overflowed/canceled and should be removed.
    pub fn deliver_chunk(&self, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let (wake, canceled_now) = {
            let mut st = self.state.lock().unwrap();
            if st.canceled || st.ended || self.grant.is_revoked() {
                return st.canceled;
            }
            st.total_received = st.total_received.saturating_add(bytes.len() as u64);
            let exceeds_len = st.content_length.is_some_and(|cl| st.total_received > cl);
            let exceeds_cap = st.total_received > self.grant.kind.max_bytes();
            let exceeds_queue = st.queued_bytes.saturating_add(bytes.len()) > MAX_PROXY_QUEUE_BYTES;
            if exceeds_cap || exceeds_len || exceeds_queue {
                st.canceled = true;
                st.ended = true;
                st.end_ok = false;
                st.chunks.clear();
                st.queued_bytes = 0;
                (st.waiting_read.take().map(|w| (w.callback, -2i32)), true)
            } else if let Some(wait) = st.waiting_read.take() {
                let n = bytes.len().min(wait.max_len);
                // SAFETY: CEF guarantees `data_out` remains valid until `wait.callback.cont` runs
                // or `cancel()` returns; `cancel()` acquires `self.state` first.
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), wait.data_out as *mut u8, n);
                }
                if n < bytes.len() {
                    let rem = bytes[n..].to_vec();
                    st.queued_bytes = st.queued_bytes.saturating_add(rem.len());
                    st.chunks.push_front(rem);
                }
                (Some((wait.callback, n as i32)), false)
            } else {
                st.queued_bytes = st.queued_bytes.saturating_add(bytes.len());
                st.chunks.push_back(bytes);
                (None, false)
            }
        };
        if let Some((cb, n)) = wake {
            cb.cont(n);
        }
        canceled_now
    }

    pub fn deliver_end(&self, ok: bool) {
        let wake = {
            let mut st = self.state.lock().unwrap();
            st.ended = true;
            st.end_ok = ok && !st.canceled && !self.grant.is_revoked();
            if !st.end_ok {
                st.chunks.clear();
                st.queued_bytes = 0;
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

/// Engine-wide preview grant, bounded local I/O pool, & async proxy fetch registry.
pub struct PreviewRegistry {
    next_req_id: AtomicU64,
    proxies: Mutex<HashMap<u64, Arc<PendingProxy>>>,
    io: PreviewIoPool,
}

impl PreviewRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_req_id: AtomicU64::new(1),
            proxies: Mutex::new(HashMap::new()),
            io: PreviewIoPool::new(),
        })
    }

    pub fn io(&self) -> &PreviewIoPool {
        &self.io
    }

    /// Creates a new isolated grant from `spec`, pinning the local workspace root immediately.
    /// Returns `(grant, internal_oc_preview_url, initial_display_url)`.
    pub fn create_grant(
        &self,
        tab_id: u64,
        spec: PreviewSpec,
        home: Option<&str>,
    ) -> Result<(Arc<PreviewGrant>, String, String), &'static str> {
        let grant_host = random_grant_host()?;
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
                    ready: AtomicBool::new(false),
                    entry_taken: AtomicBool::new(false),
                    notified_denied: AtomicBool::new(false),
                    in_flight_io: AtomicUsize::new(0),
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
                if !browser_policy::preview::preview_path_valid(&doc, root_is_windows(&root))
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
                    ready: AtomicBool::new(false),
                    entry_taken: AtomicBool::new(false),
                    notified_denied: AtomicBool::new(false),
                    in_flight_io: AtomicUsize::new(0),
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
                    ready: AtomicBool::new(false),
                    entry_taken: AtomicBool::new(false),
                    notified_denied: AtomicBool::new(false),
                    in_flight_io: AtomicUsize::new(0),
                    last_display: Mutex::new(display.clone()),
                    initiators: Mutex::new(InitiatorCache::default()),
                });
                Ok((grant, internal, display))
            }
        }
    }

    pub fn register_proxy(
        &self,
        tab_id: u64,
        grant: Arc<PreviewGrant>,
        head_only: bool,
        remote_rel: Option<String>,
    ) -> (u64, Arc<PendingProxy>) {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let proxy = PendingProxy::new(tab_id, grant, head_only, remote_rel);
        self.proxies.lock().unwrap().insert(req_id, proxy.clone());
        (req_id, proxy)
    }

    pub fn remove_proxy(&self, req_id: u64) {
        self.proxies.lock().unwrap().remove(&req_id);
    }

    pub fn cancel_tab_proxies(&self, tab_id: u64) {
        let victims: Vec<Arc<PendingProxy>> = {
            let mut map = self.proxies.lock().unwrap();
            let ids: Vec<u64> = map
                .iter()
                .filter_map(|(&id, p)| (p.tab_id == tab_id).then_some(id))
                .collect();
            ids.into_iter().filter_map(|id| map.remove(&id)).collect()
        };
        for p in victims {
            p.cancel();
        }
    }

    pub fn cancel_all_proxies(&self) {
        let victims: Vec<Arc<PendingProxy>> = {
            let mut map = self.proxies.lock().unwrap();
            map.drain().map(|(_, p)| p).collect()
        };
        for p in victims {
            p.cancel();
        }
    }

    pub fn deliver_head(
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
            let remove_now = p.deliver_head(ok, status, mime, content_length, content_range);
            if remove_now {
                self.remove_proxy(req_id);
            }
        }
    }

    pub fn deliver_chunk(&self, req_id: u64, data_b64: &str) {
        use base64::Engine as _;
        let proxy = self.proxies.lock().unwrap().get(&req_id).cloned();
        if let Some(p) = proxy {
            match base64::engine::general_purpose::STANDARD.decode(data_b64) {
                Ok(bytes) => {
                    if p.deliver_chunk(bytes) {
                        self.remove_proxy(req_id);
                    }
                }
                Err(_) => {
                    p.cancel();
                    self.remove_proxy(req_id);
                }
            }
        }
    }

    pub fn deliver_end(&self, req_id: u64, ok: bool) {
        let proxy = self.proxies.lock().unwrap().remove(&req_id);
        if let Some(p) = proxy {
            p.deliver_end(ok);
        }
    }
}

/// Generates a CSPRNG UUIDv4 grant hostname (122 random bits). Fails closed if OS CSPRNG fails.
fn random_grant_host() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    fill_os_random(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ))
}

#[cfg(windows)]
fn fill_os_random(out: &mut [u8]) -> Result<(), &'static str> {
    use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    unsafe { BCryptGenRandom(None, out, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(|_| "browser.context.unavailable")
}

#[cfg(unix)]
fn fill_os_random(out: &mut [u8]) -> Result<(), &'static str> {
    use std::io::Read as _;
    let mut f = std::fs::File::open("/dev/urandom").map_err(|_| "browser.context.unavailable")?;
    f.read_exact(out).map_err(|_| "browser.context.unavailable")
}
