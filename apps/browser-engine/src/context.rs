//! Shared partitioned `CefRequestContext` pool (`apps/browser-engine/src/context.rs`) — the
//! **sole** caller of `cef::request_context_create_context` across WS-1, PREVIEWS (`#281`), WS-9
//! (`#274`), and BROWSER-TOOLS.
//!
//! ## Scope Partitioning & Canonical Keys
//! - `ContextKind::Default` -> Global persistent profile (`OPENCODE_BROWSER_PROFILE/Default`,
//!   `persist_session_cookies: 1`); never stored in `ContextPool`.
//! - `ContextKind::Preview { workspace, grant }` -> `ContextKey::Preview { workspace, grant }`.
//!   Distinct per workspace and grant; never shares cookies, storage, cache, or service workers
//!   with another workspace's previews, with artifacts, with private tabs, with remote servers, or
//!   with `Default`.
//! - `ContextKind::Artifact { artifact_id }` -> `ContextKey::Artifact(artifact_id)`. Separate enum
//!   variant and key namespace from `Preview` so artifacts never share a context with workspace
//!   previews (even if `artifact_id == grant`).
//! - `ContextKind::Private` -> `ContextKey::Private(tab_id)`. Unique per private tab; never shared
//!   with any other private tab or scope.
//! - `ContextKind::RemoteServer { server_id, proxy }` -> `ContextKey::RemoteServer(server_id)`
//!   keyed strictly by canonical server identity (`ServerKey::as_str()`, never display name).
//!
//! ## Security, Fail-Closed & Lifetime Guarantees
//! - **Fail closed:** Empty/whitespace keys return `Err("browser.context.invalid")`; non-loopback
//!   or zero-port proxies return `Err("browser.proxy.invalid")`; if CEF fails to create an isolated
//!   `CefRequestContext`, `resolve_with_hooks` returns `Err("browser.context.unavailable")` and the
//!   caller refuses the tab without creating a browser — an isolated tab NEVER falls back to
//!   `Default`.
//! - **Zero disk writes on isolated contexts:** All non-`Default` contexts (`Preview`, `Artifact`,
//!   `Private`, `RemoteServer`) use empty `cache_path` and `persist_session_cookies: 0`. They live
//!   strictly in memory and are invisible to main-profile extensions.
//! - **Immediate teardown on last tab close:** Each pooled context ref-counts its open `tab_id`s.
//!   When its last tab closes (`release_tab`), `ContextPool` removes the entry immediately, closes
//!   all connections, wipes in-memory cookies, and drops the `CefRequestContext` handle so cookies,
//!   `localStorage`, caches, and service workers are destroyed. For `RemoteServer`, any active
//!   proxy config stays in `remote_proxies` until `DropContext` so reopening a tab on that still-
//!   attached remote server gets a fresh clean context with the same tunnel proxy.
//! - **Network lockdown:** Every isolated context disables non-proxied WebRTC UDP, WebRTC multiple
//!   routes, DoH, QUIC, and DNS prefetching / network prediction; proxied `RemoteServer` contexts
//!   also enforce `fixed_servers` + `bypass_list = "<-loopback>"` and defer initial loads until
//!   `on_request_context_initialized` succeeds (failing closed if proxy preference setup fails).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    Browser, Callback, CefString, Frame, ImplCookieManager, ImplDictionaryValue,
    ImplPreferenceManager, ImplRequest, ImplRequestContext, ImplRequestContextHandler,
    ImplResourceRequestHandler, ImplValue, Request, RequestContext, RequestContextHandler,
    RequestContextSettings, ResourceRequestHandler, Response, ReturnValue,
    WrapRequestContextHandler, WrapResourceRequestHandler, sys, wrap_request_context_handler,
    wrap_resource_request_handler,
};

use crate::wire::{ContextKind, RemoteProxyConfig};

const DEFAULT_PROXY_REALM: &str = "OpenCode Browser Proxy";

/// Canonical pool key guaranteeing every isolation scope occupies its own disjoint namespace.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ContextKey {
    Preview { workspace: String, grant: String },
    Artifact(String),
    Private(u64),
    RemoteServer(String),
}

/// Optional context-wide `CefResourceRequestHandler` callback (used by PREVIEWS `#281` to confine
/// Web Workers, `sendBeacon`, WebSockets, and subresources on `oc-preview://` contexts).
pub type ResourceHook = Arc<
    dyn Fn(
            Option<&mut Browser>,
            Option<&mut Frame>,
            Option<&mut Request>,
            i32,
            i32,
            Option<&CefString>,
            Option<&mut i32>,
        ) -> Option<ResourceRequestHandler>
        + Send
        + Sync,
>;

/// Per-creation hooks allowing callers such as PREVIEWS (`#281`) to register custom scheme
/// factories and context-wide resource filters without duplicating `request_context_create_context`.
#[derive(Default)]
pub struct ContextHooks {
    pub resource_filter: Option<ResourceHook>,
    pub exclude_default_cookie_schemes: bool,
    pub on_created: Option<Box<dyn FnOnce(&RequestContext)>>,
}

/// Validates that a `RemoteProxyConfig` points strictly at a loopback host and non-zero port.
pub fn validate_proxy(cfg: &RemoteProxyConfig) -> Result<(), &'static str> {
    let host = cfg.host.trim();
    if host.is_empty() || cfg.port == 0 || !browser_policy::loopback_host(host) {
        return Err("browser.proxy.invalid");
    }
    if host.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, ';' | '=' | '/')) {
        return Err("browser.proxy.invalid");
    }
    Ok(())
}

/// Checks whether a CEF `auth_credentials(is_proxy == 1)` challenge matches `cfg` (`network.ts:57-66`).
pub fn proxy_matches_challenge(
    cfg: &RemoteProxyConfig,
    scheme: &str,
    host: &str,
    port: i32,
    realm: &str,
) -> bool {
    let want_realm = cfg.realm.as_deref().unwrap_or(DEFAULT_PROXY_REALM);
    scheme.eq_ignore_ascii_case("basic")
        && host.eq_ignore_ascii_case(cfg.host.trim())
        && port == i32::from(cfg.port)
        && realm == want_realm
}

/// Applies full privacy/network lockdown (and optional `fixed_servers` + `<-loopback>` proxy) to an
/// isolated `CefRequestContext`. Returns `true` when requested proxy configuration succeeded.
fn apply_context_prefs(ctx: &RequestContext, proxy: Option<&RemoteProxyConfig>) -> bool {
    let mut ok = true;
    if let Some(cfg) = proxy {
        ok = set_proxy_pref(ctx, Some(cfg));
    } else if ctx.has_preference(Some(&CefString::from("proxy"))) != 0 {
        let _ = set_proxy_pref(ctx, None);
    }
    set_string_pref(ctx, "webrtc.ip_handling_policy", "disable_non_proxied_udp");
    set_bool_pref(ctx, "webrtc.multiple_routes_enabled", false);
    set_bool_pref(ctx, "webrtc.nonproxied_udp_enabled", false);
    set_string_pref(ctx, "dns_over_https.mode", "off");
    set_bool_pref(ctx, "net.quic_allowed", false);
    // 2 == NETWORK_PREDICTION_NEVER in Chromium's PrefService.
    set_int_pref(ctx, "net.network_prediction_options", 2);
    set_bool_pref(ctx, "dns_prefetching.enabled", false);
    ok
}

fn set_proxy_pref(ctx: &RequestContext, proxy: Option<&RemoteProxyConfig>) -> bool {
    let Some(mut dict) = cef::dictionary_value_create() else { return false };
    match proxy {
        Some(cfg) => {
            let server = format!("http://{}:{}", cfg.host.trim(), cfg.port);
            dict.set_string(Some(&CefString::from("mode")), Some(&CefString::from("fixed_servers")));
            dict.set_string(Some(&CefString::from("server")), Some(&CefString::from(server.as_str())));
            dict.set_string(Some(&CefString::from("bypass_list")), Some(&CefString::from("<-loopback>")));
        }
        None => {
            dict.set_string(Some(&CefString::from("mode")), Some(&CefString::from("direct")));
        }
    }
    let Some(mut val) = cef::value_create() else { return false };
    val.set_dictionary(Some(&mut dict));
    // `CefString::default()` maps to NULL in `cef-rs`, which CEF's `SetPreference` rejects;
    // initialize a non-empty `CefString` so the out-parameter points to a real `cef_string_t`.
    let mut err = CefString::from(" ");
    let ok = ctx.set_preference(Some(&CefString::from("proxy")), Some(&mut val), Some(&mut err)) != 0;
    if !ok {
        eprintln!(
            "context: set_preference(proxy) failed: {} (has={} can={})",
            err,
            ctx.has_preference(Some(&CefString::from("proxy"))),
            ctx.can_set_preference(Some(&CefString::from("proxy")))
        );
    }
    ok
}

fn set_string_pref(ctx: &RequestContext, name: &str, value: &str) {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 {
        return;
    }
    let Some(mut val) = cef::value_create() else { return };
    val.set_string(Some(&CefString::from(value)));
    let mut err = CefString::from(" ");
    let _ = ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err));
}

fn set_bool_pref(ctx: &RequestContext, name: &str, value: bool) {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 {
        return;
    }
    let Some(mut val) = cef::value_create() else { return };
    val.set_bool(i32::from(value));
    let mut err = CefString::from(" ");
    let _ = ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err));
}

fn set_int_pref(ctx: &RequestContext, name: &str, value: i32) {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 {
        return;
    }
    let Some(mut val) = cef::value_create() else { return };
    val.set_int(value);
    let mut err = CefString::from(" ");
    let _ = ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err));
}

/// Outcome produced once a deferred context finishes `on_request_context_initialized`.
pub enum DeferredOutcome {
    Load { tab_id: u64, url: String },
    Refuse { tab_id: u64, reason: &'static str },
}

struct ContextEntry {
    ctx: RequestContext,
    tabs: HashSet<u64>,
    initialized: Arc<AtomicBool>,
    prefs_ok: Arc<AtomicBool>,
    proxy: Arc<Mutex<Option<RemoteProxyConfig>>>,
    /// `(tab_id, url)` loads waiting for `on_request_context_initialized` to finish applying proxy prefs.
    deferred_loads: Arc<Mutex<Vec<(u64, String)>>>,
}

impl ContextEntry {
    fn teardown(&self) {
        self.deferred_loads.lock().unwrap().clear();
        self.ctx.close_all_connections(None);
        if let Some(mgr) = self.ctx.cookie_manager(None) {
            let _ = mgr.delete_cookies(None, None, None);
        }
    }
}

wrap_request_context_handler! {
    struct IsolatedContextHandler {
        initialized: Arc<AtomicBool>,
        prefs_ok: Arc<AtomicBool>,
        proxy: Arc<Mutex<Option<RemoteProxyConfig>>>,
        ready_loads: Arc<Mutex<Vec<DeferredOutcome>>>,
        deferred_loads: Arc<Mutex<Vec<(u64, String)>>>,
        resource_filter: Option<ResourceHook>,
        pump: Sender<crate::Event>,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            let proxy = self.proxy.lock().unwrap().clone();
            let applied = request_context
                .is_some_and(|ctx| apply_context_prefs(ctx, proxy.as_ref()));
            self.prefs_ok.store(applied, Ordering::Release);
            self.initialized.store(true, Ordering::Release);
            let pending = std::mem::take(&mut *self.deferred_loads.lock().unwrap());
            if !pending.is_empty() {
                let mut out = self.ready_loads.lock().unwrap();
                for (tab_id, url) in pending {
                    if applied {
                        out.push(DeferredOutcome::Load { tab_id, url });
                    } else {
                        out.push(DeferredOutcome::Refuse {
                            tab_id,
                            reason: "browser.proxy.invalid",
                        });
                    }
                }
                let _ = self.pump.send(crate::Event::Pump(0));
            }
        }

        fn resource_request_handler(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            is_navigation: ::std::os::raw::c_int,
            is_download: ::std::os::raw::c_int,
            request_initiator: Option<&CefString>,
            disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            self.resource_filter.as_ref().and_then(|hook| {
                hook(
                    browser,
                    frame,
                    request,
                    is_navigation,
                    is_download,
                    request_initiator,
                    disable_default_handling,
                )
            })
        }
    }
}

fn is_blocked_file_url(url: &str) -> bool {
    let mut cur = url.trim();
    while cur.len() >= 5 && cur.as_bytes()[..5].eq_ignore_ascii_case(b"blob:") {
        cur = cur[5..].trim();
    }
    browser_policy::is_file_scheme(cur)
}

wrap_resource_request_handler! {
    // Blocks all `file://` (and `blob:file://`) requests and redirects initiated by extension
    // service workers, offscreen documents, or extension pages (`review-rd274.md` §2).
    struct ExtensionFileBlockHandler;

    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            if is_blocked_file_url(&url) {
                ReturnValue::CANCEL
            } else {
                ReturnValue::CONTINUE
            }
        }

        fn on_resource_redirect(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _response: Option<&mut Response>,
            new_url: Option<&mut CefString>,
        ) {
            if let Some(new_url) = new_url
                && is_blocked_file_url(&new_url.to_string())
            {
                *new_url = CefString::from("");
            }
        }

        fn on_protocol_execution(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            allow_os_execution: Option<&mut ::std::os::raw::c_int>,
        ) {
            if let Some(allow) = allow_os_execution {
                *allow = 0;
            }
        }
    }
}

/// Disables `file://` scheme access in Chromium's `ExtensionPrefs`
/// (`extensions.settings.<id>.newAllowFileAccess = false`) for `ext_id` before
/// `Extensions.loadUnpacked` runs so `UnpackedInstaller` never grants
/// `Extension::ALLOW_FILE_ACCESS` (`review-rd274.md` §2).
pub fn disallow_extension_file_access(ext_id: &str) -> bool {
    let Some(ctx) = cef::request_context_get_global_context() else {
        return false;
    };
    let key = CefString::from("extensions.settings");
    if ctx.has_preference(Some(&key)) == 0 || ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    let existing = ctx.preference(Some(&key));
    let mut root_dict = existing
        .as_ref()
        .and_then(|v| v.dictionary())
        .and_then(|d| d.copy(0))
        .or_else(cef::dictionary_value_create);
    let Some(mut root_dict) = root_dict.take() else {
        return false;
    };
    let ext_key = CefString::from(ext_id);
    let mut ext_dict = root_dict
        .dictionary(Some(&ext_key))
        .and_then(|d| d.copy(0))
        .or_else(cef::dictionary_value_create);
    let Some(mut ext_dict) = ext_dict.take() else {
        return false;
    };
    ext_dict.set_bool(Some(&CefString::from("newAllowFileAccess")), 0);
    ext_dict.set_bool(Some(&CefString::from("allow_file_access")), 0);
    root_dict.set_dictionary(Some(&ext_key), Some(&mut ext_dict));
    let Some(mut val) = cef::value_create() else {
        return false;
    };
    val.set_dictionary(Some(&mut root_dict));
    let mut err = CefString::from(" ");
    ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) != 0
}

wrap_request_context_handler! {
    // Attached to CEF's default/global `CefRequestContext` via
    // `BrowserProcessHandler::default_request_context_handler`: cancels every `file://` fetch
    // and redirect from extension service workers, offscreen documents, and extension-origin
    // initiators (`review-rd274.md` §2).
    pub struct DefaultRequestContextHandler;

    impl RequestContextHandler {
        fn resource_request_handler(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int,
            request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            let initiator = request_initiator.map(CefString::to_string).unwrap_or_default();
            if browser.is_none() || browser_policy::extension_origin(&initiator) {
                return Some(ExtensionFileBlockHandler::new());
            }
            None
        }
    }
}

/// Pool of non-default partitioned `CefRequestContext` instances managed by `EngineManager`.
pub struct ContextPool {
    entries: HashMap<ContextKey, ContextEntry>,
    /// Tracks which pooled context each open `tab_id` belongs to for immediate teardown on close.
    tab_keys: HashMap<u64, ContextKey>,
    /// Pre-configured remote server proxies kept across tab close/reopen until `DropContext`.
    remote_proxies: HashMap<String, RemoteProxyConfig>,
    /// Outcomes promoted by `on_request_context_initialized` ready for `EngineManager::poll_picks`.
    ready_loads: Arc<Mutex<Vec<DeferredOutcome>>>,
    pump: Sender<crate::Event>,
}

/// Result of resolving a tab's `ContextKind` before creating its CEF browser.
pub struct ResolvedContext {
    pub request_context: Option<RequestContext>,
    pub proxy: Option<RemoteProxyConfig>,
    /// When `true`, open on `about:blank` and defer `initial_url` until context preferences apply.
    pub defer_initial_load: bool,
}

impl ContextPool {
    pub fn new(pump: Sender<crate::Event>) -> Self {
        Self {
            entries: HashMap::new(),
            tab_keys: HashMap::new(),
            remote_proxies: HashMap::new(),
            ready_loads: Arc::new(Mutex::new(Vec::new())),
            pump,
        }
    }

    /// Resolves (or creates) the partitioned `CefRequestContext` for `kind` with default hooks.
    pub fn resolve(
        &mut self,
        kind: &ContextKind,
        tab_id: u64,
        initial_url: &str,
    ) -> Result<ResolvedContext, &'static str> {
        self.resolve_with_hooks(kind, tab_id, initial_url, ContextHooks::default())
    }

    /// Resolves (or creates) the partitioned `CefRequestContext` for `kind`, attaching optional
    /// PREVIEWS scheme/resource hooks. Always fails closed (never falls back to `Default`).
    pub fn resolve_with_hooks(
        &mut self,
        kind: &ContextKind,
        tab_id: u64,
        initial_url: &str,
        hooks: ContextHooks,
    ) -> Result<ResolvedContext, &'static str> {
        let (key, explicit_proxy) = match kind {
            ContextKind::Default => {
                return Ok(ResolvedContext {
                    request_context: None,
                    proxy: None,
                    defer_initial_load: false,
                });
            }
            ContextKind::Preview { workspace, grant } => {
                if workspace.trim().is_empty() || grant.trim().is_empty() {
                    return Err("browser.context.invalid");
                }
                (
                    ContextKey::Preview {
                        workspace: workspace.clone(),
                        grant: grant.clone(),
                    },
                    None,
                )
            }
            ContextKind::Artifact { artifact_id } => {
                if artifact_id.trim().is_empty() {
                    return Err("browser.context.invalid");
                }
                (ContextKey::Artifact(artifact_id.clone()), None)
            }
            ContextKind::Private => (ContextKey::Private(tab_id), None),
            ContextKind::RemoteServer { server_id, proxy } => {
                if server_id.trim().is_empty() {
                    return Err("browser.context.invalid");
                }
                if let Some(cfg) = proxy {
                    validate_proxy(cfg)?;
                }
                let effective = match proxy {
                    Some(cfg) => {
                        self.remote_proxies.insert(server_id.clone(), cfg.clone());
                        Some(cfg.clone())
                    }
                    None => self.remote_proxies.get(server_id).cloned(),
                };
                (ContextKey::RemoteServer(server_id.clone()), effective)
            }
        };

        // Ensure a reused `tab_id` never leaks a stale ref-count in a previous context.
        if self.tab_keys.get(&tab_id).is_some_and(|prev| prev != &key) {
            self.release_tab(tab_id);
        }

        if !self.entries.contains_key(&key) {
            let entry = create_isolated_context(
                explicit_proxy.clone(),
                self.ready_loads.clone(),
                self.pump.clone(),
                hooks,
            )?;
            self.entries.insert(key.clone(), entry);
        } else if let Some(cfg) = explicit_proxy {
            let existing = self.entries.get_mut(&key).unwrap();
            let changed = existing.proxy.lock().unwrap().as_ref() != Some(&cfg);
            if changed {
                *existing.proxy.lock().unwrap() = Some(cfg.clone());
                if existing.initialized.load(Ordering::Acquire) {
                    let ok = apply_context_prefs(&existing.ctx, Some(&cfg));
                    existing.prefs_ok.store(ok, Ordering::Release);
                    existing.ctx.close_all_connections(None);
                    if !ok {
                        return Err("browser.proxy.invalid");
                    }
                }
            }
        }

        let entry = self.entries.get_mut(&key).unwrap();
        let effective_proxy = entry.proxy.lock().unwrap().clone();
        let is_init = entry.initialized.load(Ordering::Acquire);
        if effective_proxy.is_some() && is_init && !entry.prefs_ok.load(Ordering::Acquire) {
            if entry.tabs.is_empty() {
                let removed = self.entries.remove(&key);
                if let Some(e) = removed {
                    e.teardown();
                }
            }
            return Err("browser.proxy.invalid");
        }

        entry.tabs.insert(tab_id);
        self.tab_keys.insert(tab_id, key);

        let need_defer = effective_proxy.is_some()
            && !is_init
            && !initial_url.eq_ignore_ascii_case("about:blank");
        if need_defer {
            entry
                .deferred_loads
                .lock()
                .unwrap()
                .push((tab_id, initial_url.to_owned()));
        }

        Ok(ResolvedContext {
            request_context: Some(entry.ctx.clone()),
            proxy: effective_proxy,
            defer_initial_load: need_defer,
        })
    }

    /// Releases `tab_id` from its pooled context. When the context's last tab closes, the context
    /// is removed from the pool immediately, its connections are closed, its in-memory cookies are
    /// wiped, and its `CefRequestContext` handle is dropped so all in-memory storage, caches, and
    /// service workers are destroyed.
    pub fn release_tab(&mut self, tab_id: u64) {
        let Some(key) = self.tab_keys.remove(&tab_id) else {
            return;
        };
        let empty = if let Some(entry) = self.entries.get_mut(&key) {
            entry.tabs.remove(&tab_id);
            entry.deferred_loads.lock().unwrap().retain(|(id, _)| *id != tab_id);
            entry.tabs.is_empty()
        } else {
            false
        };
        if empty && let Some(entry) = self.entries.remove(&key) {
            entry.teardown();
        }
    }

    /// Tears down every pooled isolated context on engine shutdown.
    pub fn release_all(&mut self) {
        self.tab_keys.clear();
        self.remote_proxies.clear();
        for (_, entry) in self.entries.drain() {
            entry.teardown();
        }
    }

    /// Updates or clears the local loopback proxy on a remote server partition (even before its
    /// first tab opens), closing active connections immediately on a live context (`network.ts:47`).
    pub fn set_partition_proxy(
        &mut self,
        server_id: &str,
        proxy: Option<RemoteProxyConfig>,
    ) -> Result<Option<RemoteProxyConfig>, &'static str> {
        if server_id.trim().is_empty() {
            return Err("browser.context.invalid");
        }
        if let Some(cfg) = &proxy {
            validate_proxy(cfg)?;
            self.remote_proxies.insert(server_id.to_owned(), cfg.clone());
        } else {
            self.remote_proxies.remove(server_id);
        }
        let key = ContextKey::RemoteServer(server_id.to_owned());
        if let Some(entry) = self.entries.get_mut(&key) {
            *entry.proxy.lock().unwrap() = proxy.clone();
            if entry.initialized.load(Ordering::Acquire) {
                let ok = apply_context_prefs(&entry.ctx, proxy.as_ref());
                entry.prefs_ok.store(ok, Ordering::Release);
                entry.ctx.close_all_connections(None);
                if proxy.is_some() && !ok {
                    return Err("browser.proxy.invalid");
                }
            }
        }
        Ok(proxy)
    }

    /// Returns the live `RequestContext` for a canonical `ContextKey`.
    #[allow(dead_code)]
    pub fn context_for_key(&self, key: &ContextKey) -> Option<RequestContext> {
        self.entries.get(key).map(|e| e.ctx.clone())
    }

    /// Returns the live `RequestContext` for a remote server identity (for Phase 3b cookie mirror).
    #[allow(dead_code)]
    pub fn remote_context(&self, server_id: &str) -> Option<RequestContext> {
        self.entries
            .get(&ContextKey::RemoteServer(server_id.to_owned()))
            .map(|e| e.ctx.clone())
    }

    /// Returns all active remote server partitions and their `RequestContext`s.
    #[allow(dead_code)]
    pub fn remote_partitions(&self) -> Vec<(String, RequestContext)> {
        self.entries
            .iter()
            .filter_map(|(k, e)| match k {
                ContextKey::RemoteServer(id) => Some((id.clone(), e.ctx.clone())),
                _ => None,
            })
            .collect()
    }

    /// Explicitly drops a pooled context and (for `RemoteServer`) clears its stored proxy config.
    pub fn drop_context(&mut self, kind: &ContextKind) {
        let key = match kind {
            ContextKind::Default | ContextKind::Private => None,
            ContextKind::Preview { workspace, grant } => Some(ContextKey::Preview {
                workspace: workspace.clone(),
                grant: grant.clone(),
            }),
            ContextKind::Artifact { artifact_id } => Some(ContextKey::Artifact(artifact_id.clone())),
            ContextKind::RemoteServer { server_id, .. } => {
                self.remote_proxies.remove(server_id);
                Some(ContextKey::RemoteServer(server_id.clone()))
            }
        };
        if let Some(k) = key
            && let Some(entry) = self.entries.remove(&k)
        {
            for tab_id in &entry.tabs {
                self.tab_keys.remove(tab_id);
            }
            entry.teardown();
        }
    }

    /// Drains any initial tab URLs (or fail-closed refusals) whose proxied `RequestContext`
    /// just finished initializing.
    pub fn drain_ready_loads(&self) -> Vec<DeferredOutcome> {
        std::mem::take(&mut *self.ready_loads.lock().unwrap())
    }
}

fn create_isolated_context(
    proxy: Option<RemoteProxyConfig>,
    ready_loads: Arc<Mutex<Vec<DeferredOutcome>>>,
    pump: Sender<crate::Event>,
    hooks: ContextHooks,
) -> Result<ContextEntry, &'static str> {
    let initialized = Arc::new(AtomicBool::new(false));
    let prefs_ok = Arc::new(AtomicBool::new(false));
    let proxy_arc = Arc::new(Mutex::new(proxy.clone()));
    let deferred_loads = Arc::new(Mutex::new(Vec::new()));
    let mut handler = IsolatedContextHandler::new(
        initialized.clone(),
        prefs_ok.clone(),
        proxy_arc.clone(),
        ready_loads,
        deferred_loads.clone(),
        hooks.resource_filter,
        pump,
    );
    // Empty `cache_path` + `persist_session_cookies: 0` creates a strictly ephemeral in-memory
    // storage/cookie/cache/SW partition isolated from `OPENCODE_BROWSER_PROFILE` and extensions.
    let settings = RequestContextSettings {
        size: std::mem::size_of::<sys::_cef_request_context_settings_t>(),
        cache_path: CefString::default(),
        persist_session_cookies: 0,
        accept_language_list: CefString::default(),
        cookieable_schemes_list: CefString::default(),
        cookieable_schemes_exclude_defaults: i32::from(hooks.exclude_default_cookie_schemes),
    };
    let Some(ctx) = cef::request_context_create_context(Some(&settings), Some(&mut handler)) else {
        return Err("browser.context.unavailable");
    };
    if let Some(on_created) = hooks.on_created {
        on_created(&ctx);
    }
    // If CEF initialized the context synchronously, apply prefs immediately.
    if initialized.load(Ordering::Acquire) {
        let ok = apply_context_prefs(&ctx, proxy.as_ref());
        prefs_ok.store(ok, Ordering::Release);
    }
    Ok(ContextEntry {
        ctx,
        tabs: HashSet::new(),
        initialized,
        prefs_ok,
        proxy: proxy_arc,
        deferred_loads,
    })
}
