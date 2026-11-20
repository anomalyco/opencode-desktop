//! Shared partitioned `CefRequestContext` pool (`apps/browser-engine/src/context.rs`) — the
//! **sole** caller of `cef::request_context_create_context` across WS-1, PREVIEWS (`#281`), WS-9
//! (`#274`), and BROWSER-TOOLS.
//!
//! ## Scope Partitioning & Canonical Keys
//! - `ContextKind::Default` -> Global persistent profile (`OPENCODE_BROWSER_PROFILE/Default`,
//!   `persist_session_cookies: 1`); never stored in `ContextPool`.
//! - `ContextKind::Preview { workspace, grant }` -> `ContextKey::Preview { workspace, grant }`
//!   (trimmed). Distinct per workspace and grant.
//! - `ContextKind::Artifact { artifact_id }` -> `ContextKey::Artifact(artifact_id)` (trimmed).
//!   Separate enum variant and key namespace from `Preview`.
//! - `ContextKind::Private` -> `ContextKey::Private(tab_id)`. Unique per private tab.
//! - `ContextKind::RemoteServer { server_id, proxy }` -> `ContextKey::RemoteServer(server_id)`
//!   (trimmed canonical `ServerKey::as_str()` identity, never display name). **ALWAYS requires a
//!   live loopback tunnel proxy** (`browser.proxy.required` if absent; never falls back to direct).
//!
//! ## Fail-Closed, Proxy-Init Gating & Teardown Guarantees
//! - **Fail closed:** Empty/whitespace keys -> `browser.context.invalid`; missing proxy on
//!   `RemoteServer` -> `browser.proxy.required`; non-loopback or zero-port proxy ->
//!   `browser.proxy.invalid`; CEF context/browser creation failure -> `browser.context.unavailable`.
//! - **Zero proxy-bypass race window:** Every newly created `RemoteServer` tab opens on
//!   `about:blank` and gates BOTH its initial URL and any subsequent `HostMsg::Navigate` / reload /
//!   history nav until `on_request_context_initialized` finishes applying and verifying `proxy`
//!   (`fixed_servers`, `<-loopback>`) + network lockdown preferences. If preference setup fails,
//!   deferred tabs are refused and closed immediately.
//! - **Immediate teardown:** `release_tab` ref-counts open tabs per `ContextKey` and immediately
//!   removes, closes connections, deletes in-memory cookies, and drops the `CefRequestContext` when
//!   the last tab closes. Clearing a remote server's proxy (`SetPartitionProxy { proxy: None }`),
//!   failing a proxy update, or calling `DropContext` closes all live tabs in that context first so
//!   no tab can ever continue unproxied or keep an isolated context alive.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, Frame, ImplCookieManager, ImplDictionaryValue, ImplPreferenceManager,
    ImplRequestContext, ImplRequestContextHandler, ImplValue, Request, RequestContext,
    RequestContextHandler, RequestContextSettings, ResourceRequestHandler,
    WrapRequestContextHandler, sys, wrap_request_context_handler,
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

/// Formats `cfg`'s host and port into a valid Chromium `fixed_servers` URL (bracketing IPv6).
fn format_proxy_server(cfg: &RemoteProxyConfig) -> String {
    let host = cfg.host.trim();
    let bracketed = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    format!("http://{bracketed}:{}", cfg.port)
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
    let cfg_host = cfg.host.trim().trim_start_matches('[').trim_end_matches(']');
    let chal_host = host.trim().trim_start_matches('[').trim_end_matches(']');
    scheme.eq_ignore_ascii_case("basic")
        && chal_host.eq_ignore_ascii_case(cfg_host)
        && port == i32::from(cfg.port)
        && realm == want_realm
}

/// Applies full privacy/network lockdown (and mandatory `fixed_servers` + `<-loopback>` proxy for
/// `RemoteServer` contexts) and verifies every required preference write succeeded.
///
/// Verified empirically on CEF 156.0.26 Alloy: `proxy`, `webrtc.ip_handling_policy`,
/// `net.quic_allowed`, and `net.network_prediction_options` are registered and writable on
/// isolated `CefRequestContext`s and MUST all return `set_preference == 1` (fail closed otherwise).
fn apply_context_prefs(
    ctx: &RequestContext,
    proxy: Option<&RemoteProxyConfig>,
    require_proxy: bool,
) -> bool {
    let mut ok = true;
    match proxy {
        Some(cfg) => {
            if !set_proxy_pref(ctx, cfg) {
                ok = false;
            }
        }
        None if require_proxy => {
            return false;
        }
        None => {}
    }
    ok &= set_required_string_pref(ctx, "webrtc.ip_handling_policy", "disable_non_proxied_udp");
    ok &= set_required_bool_pref(ctx, "net.quic_allowed", false);
    // 2 == NETWORK_PREDICTION_NEVER in Chromium's PrefService (supersedes `dns_prefetching.enabled`).
    ok &= set_required_int_pref(ctx, "net.network_prediction_options", 2);
    // Optional legacy/global prefs: enforced if registered on a build, fail closed if writable yet rejected.
    ok &= set_optional_bool_pref(ctx, "webrtc.multiple_routes_enabled", false);
    ok &= set_optional_bool_pref(ctx, "webrtc.nonproxied_udp_enabled", false);
    ok &= set_optional_string_pref(ctx, "dns_over_https.mode", "off");
    ok &= set_optional_bool_pref(ctx, "dns_prefetching.enabled", false);
    ok
}

fn set_proxy_pref(ctx: &RequestContext, cfg: &RemoteProxyConfig) -> bool {
    let Some(mut dict) = cef::dictionary_value_create() else { return false };
    let server = format_proxy_server(cfg);
    dict.set_string(Some(&CefString::from("mode")), Some(&CefString::from("fixed_servers")));
    dict.set_string(Some(&CefString::from("server")), Some(&CefString::from(server.as_str())));
    dict.set_string(Some(&CefString::from("bypass_list")), Some(&CefString::from("<-loopback>")));
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

fn set_required_string_pref(ctx: &RequestContext, name: &str, value: &str) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 || ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    let Some(mut val) = cef::value_create() else { return false };
    val.set_string(Some(&CefString::from(value)));
    let mut err = CefString::from(" ");
    ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) != 0
}

fn set_required_bool_pref(ctx: &RequestContext, name: &str, value: bool) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 || ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    let Some(mut val) = cef::value_create() else { return false };
    val.set_bool(i32::from(value));
    let mut err = CefString::from(" ");
    ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) != 0
}

fn set_required_int_pref(ctx: &RequestContext, name: &str, value: i32) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 || ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    let Some(mut val) = cef::value_create() else { return false };
    val.set_int(value);
    let mut err = CefString::from(" ");
    ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) != 0
}

fn set_optional_string_pref(ctx: &RequestContext, name: &str, value: &str) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 {
        return true;
    }
    set_required_string_pref(ctx, name, value)
}

fn set_optional_bool_pref(ctx: &RequestContext, name: &str, value: bool) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 {
        return true;
    }
    set_required_bool_pref(ctx, name, value)
}

/// Outcome produced once a deferred context finishes `on_request_context_initialized`.
pub enum DeferredOutcome {
    Load { tab_id: u64, url: String },
    Refuse { tab_id: u64, reason: &'static str },
}

struct ContextEntry {
    ctx: RequestContext,
    tabs: HashSet<u64>,
    require_proxy: bool,
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
        require_proxy: bool,
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
                .is_some_and(|ctx| apply_context_prefs(ctx, proxy.as_ref(), self.require_proxy));
            self.prefs_ok.store(applied, Ordering::Release);
            self.initialized.store(true, Ordering::Release);
            let pending = std::mem::take(&mut *self.deferred_loads.lock().unwrap());
            if !pending.is_empty() {
                let mut out = self.ready_loads.lock().unwrap();
                for (tab_id, url) in pending {
                    if applied {
                        if !url.eq_ignore_ascii_case("about:blank") {
                            out.push(DeferredOutcome::Load { tab_id, url });
                        }
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

/// Pool of non-default partitioned `CefRequestContext` instances managed by `EngineManager`.
pub struct ContextPool {
    entries: HashMap<ContextKey, ContextEntry>,
    /// Tracks which pooled context each open `tab_id` belongs to for immediate teardown on close.
    tab_keys: HashMap<u64, ContextKey>,
    /// Pre-configured remote server proxies kept across tab close/reopen until cleared/dropped.
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
    /// PREVIEWS scheme/resource hooks. Always fails closed (never falls back to `Default` and
    /// never opens a `RemoteServer` tab without a valid loopback proxy).
    pub fn resolve_with_hooks(
        &mut self,
        kind: &ContextKind,
        tab_id: u64,
        initial_url: &str,
        hooks: ContextHooks,
    ) -> Result<ResolvedContext, &'static str> {
        let (key, explicit_proxy, require_proxy) = match kind {
            ContextKind::Default => {
                return Ok(ResolvedContext {
                    request_context: None,
                    proxy: None,
                    defer_initial_load: false,
                });
            }
            ContextKind::Preview { workspace, grant } => {
                let ws = workspace.trim();
                let gr = grant.trim();
                if ws.is_empty() || gr.is_empty() {
                    return Err("browser.context.invalid");
                }
                (
                    ContextKey::Preview {
                        workspace: ws.to_owned(),
                        grant: gr.to_owned(),
                    },
                    None,
                    false,
                )
            }
            ContextKind::Artifact { artifact_id } => {
                let aid = artifact_id.trim();
                if aid.is_empty() {
                    return Err("browser.context.invalid");
                }
                (ContextKey::Artifact(aid.to_owned()), None, false)
            }
            ContextKind::Private => (ContextKey::Private(tab_id), None, false),
            ContextKind::RemoteServer { server_id, proxy } => {
                let sid = server_id.trim();
                if sid.is_empty() {
                    return Err("browser.context.invalid");
                }
                if let Some(cfg) = proxy {
                    validate_proxy(cfg)?;
                }
                // Remote servers MUST have an authenticated loopback tunnel proxy; never run direct.
                let effective = match proxy {
                    Some(cfg) => {
                        self.remote_proxies.insert(sid.to_owned(), cfg.clone());
                        cfg.clone()
                    }
                    None => match self.remote_proxies.get(sid).cloned() {
                        Some(cfg) => {
                            validate_proxy(&cfg)?;
                            cfg
                        }
                        None => return Err("browser.proxy.required"),
                    },
                };
                (ContextKey::RemoteServer(sid.to_owned()), Some(effective), true)
            }
        };

        // Ensure a reused `tab_id` never leaks a stale ref-count in a previous context.
        if self.tab_keys.get(&tab_id).is_some_and(|prev| prev != &key) {
            let _ = self.release_tab(tab_id);
        }

        if !self.entries.contains_key(&key) {
            let entry = create_isolated_context(
                explicit_proxy.clone(),
                require_proxy,
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
                    let ok = apply_context_prefs(&existing.ctx, Some(&cfg), true);
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
        if is_init && !entry.prefs_ok.load(Ordering::Acquire) {
            if entry.tabs.is_empty()
                && let Some(e) = self.entries.remove(&key)
            {
                e.teardown();
            }
            return Err("browser.proxy.invalid");
        }

        entry.tabs.insert(tab_id);
        self.tab_keys.insert(tab_id, key);

        // Even when `initial_url == "about:blank"`, register the tab in `deferred_loads` while
        // `!is_init` on a proxied `RemoteServer` context so if proxy setup fails, `poll_picks`
        // receives `DeferredOutcome::Refuse` and closes the tab immediately.
        let need_defer = require_proxy && !is_init;
        if need_defer {
            let mut loads = entry.deferred_loads.lock().unwrap();
            loads.retain(|(id, _)| *id != tab_id);
            loads.push((tab_id, initial_url.to_owned()));
        }

        Ok(ResolvedContext {
            request_context: Some(entry.ctx.clone()),
            proxy: effective_proxy,
            defer_initial_load: need_defer && !initial_url.eq_ignore_ascii_case("about:blank"),
        })
    }

    /// Gates `HostMsg::Navigate` for `tab_id` so a `RemoteServer` tab can NEVER call `load_url`
    /// before `on_request_context_initialized` finishes applying and verifying proxy preferences.
    ///
    /// - Returns `Ok(true)` when navigation may proceed immediately.
    /// - Returns `Ok(false)` when the URL was queued in `deferred_loads` awaiting proxy init.
    /// - Returns `Err(reason)` when the context is missing or proxy prefs failed (caller must
    ///   refuse and close the tab).
    pub fn gate_navigation(&mut self, tab_id: u64, url: &str) -> Result<bool, &'static str> {
        let Some(key) = self.tab_keys.get(&tab_id) else {
            return Ok(true);
        };
        if !matches!(key, ContextKey::RemoteServer(_)) {
            return Ok(true);
        }
        let Some(entry) = self.entries.get(key) else {
            return Err("browser.proxy.required");
        };
        if !entry.initialized.load(Ordering::Acquire) {
            let mut loads = entry.deferred_loads.lock().unwrap();
            loads.retain(|(id, _)| *id != tab_id);
            loads.push((tab_id, url.to_owned()));
            return Ok(false);
        }
        if !entry.prefs_ok.load(Ordering::Acquire) {
            return Err("browser.proxy.invalid");
        }
        Ok(true)
    }

    /// Whether `tab_id`'s context has completed proxy/lockdown initialization (guards `Reload` /
    /// `Back` / `Forward` while a proxied `RemoteServer` context is still initializing).
    pub fn is_ready(&self, tab_id: u64) -> bool {
        let Some(key) = self.tab_keys.get(&tab_id) else {
            return true;
        };
        let Some(entry) = self.entries.get(key) else {
            return false;
        };
        if !entry.require_proxy {
            return true;
        }
        entry.initialized.load(Ordering::Acquire) && entry.prefs_ok.load(Ordering::Acquire)
    }

    /// Releases `tab_id` from its pooled context. When the context's last tab closes, the context
    /// is removed from the pool immediately, its connections are closed, its in-memory cookies are
    /// wiped, and its `CefRequestContext` handle is dropped.
    pub fn release_tab(&mut self, tab_id: u64) -> Option<ContextKey> {
        let key = self.tab_keys.remove(&tab_id)?;
        let empty = if let Some(entry) = self.entries.get_mut(&key) {
            entry.tabs.remove(&tab_id);
            entry.deferred_loads.lock().unwrap().retain(|(id, _)| *id != tab_id);
            entry.tabs.is_empty()
        } else {
            false
        };
        if empty && let Some(entry) = self.entries.remove(&key) {
            entry.teardown();
            return Some(key);
        }
        None
    }

    /// Tears down every pooled isolated context on engine shutdown.
    pub fn release_all(&mut self) {
        self.tab_keys.clear();
        self.remote_proxies.clear();
        for (_, entry) in self.entries.drain() {
            entry.teardown();
        }
    }

    /// Updates or clears the local loopback proxy on a remote server partition.
    /// Clearing (`proxy: None`) or failing preference application removes the live context so the
    /// caller can close any live tabs belonging to `server_id` (zero unproxied fallback).
    pub fn set_partition_proxy(
        &mut self,
        server_id: &str,
        proxy: Option<RemoteProxyConfig>,
    ) -> Result<Option<RemoteProxyConfig>, &'static str> {
        let sid = server_id.trim();
        if sid.is_empty() {
            return Err("browser.context.invalid");
        }
        let key = ContextKey::RemoteServer(sid.to_owned());
        let Some(cfg) = proxy else {
            self.remote_proxies.remove(sid);
            if let Some(entry) = self.entries.remove(&key) {
                for tid in &entry.tabs {
                    self.tab_keys.remove(tid);
                }
                entry.teardown();
            }
            return Ok(None);
        };
        validate_proxy(&cfg)?;
        self.remote_proxies.insert(sid.to_owned(), cfg.clone());
        if let Some(entry) = self.entries.get_mut(&key) {
            *entry.proxy.lock().unwrap() = Some(cfg.clone());
            if entry.initialized.load(Ordering::Acquire) {
                let ok = apply_context_prefs(&entry.ctx, Some(&cfg), true);
                entry.prefs_ok.store(ok, Ordering::Release);
                entry.ctx.close_all_connections(None);
                if !ok {
                    if let Some(removed) = self.entries.remove(&key) {
                        for tid in &removed.tabs {
                            self.tab_keys.remove(tid);
                        }
                        removed.teardown();
                    }
                    return Err("browser.proxy.invalid");
                }
            }
        }
        Ok(Some(cfg))
    }

    /// Lists all live `tab_id`s currently attached to `server_id`'s `RemoteServer` context.
    pub fn tabs_for_remote(&self, server_id: &str) -> Vec<u64> {
        let key = ContextKey::RemoteServer(server_id.trim().to_owned());
        self.entries
            .get(&key)
            .map(|e| e.tabs.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Lists all live `tab_id`s currently attached to `kind`'s pooled context.
    pub fn tabs_for_context(&self, kind: &ContextKind) -> Vec<u64> {
        let Some(key) = Self::key_for_drop(kind) else {
            return Vec::new();
        };
        self.entries
            .get(&key)
            .map(|e| e.tabs.iter().copied().collect())
            .unwrap_or_default()
    }

    fn key_for_drop(kind: &ContextKind) -> Option<ContextKey> {
        match kind {
            ContextKind::Default | ContextKind::Private => None,
            ContextKind::Preview { workspace, grant } => Some(ContextKey::Preview {
                workspace: workspace.trim().to_owned(),
                grant: grant.trim().to_owned(),
            }),
            ContextKind::Artifact { artifact_id } => {
                Some(ContextKey::Artifact(artifact_id.trim().to_owned()))
            }
            ContextKind::RemoteServer { server_id, .. } => {
                Some(ContextKey::RemoteServer(server_id.trim().to_owned()))
            }
        }
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
            .get(&ContextKey::RemoteServer(server_id.trim().to_owned()))
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
    /// Callers MUST close any live `tabs_for_context(kind)` browsers first so Chromium immediately
    /// releases the underlying `BrowserContext`.
    pub fn drop_context(&mut self, kind: &ContextKind) {
        if let ContextKind::RemoteServer { server_id, .. } = kind {
            self.remote_proxies.remove(server_id.trim());
        }
        if let Some(k) = Self::key_for_drop(kind)
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
    require_proxy: bool,
    ready_loads: Arc<Mutex<Vec<DeferredOutcome>>>,
    pump: Sender<crate::Event>,
    hooks: ContextHooks,
) -> Result<ContextEntry, &'static str> {
    let initialized = Arc::new(AtomicBool::new(false));
    let prefs_ok = Arc::new(AtomicBool::new(false));
    let proxy_arc = Arc::new(Mutex::new(proxy.clone()));
    let deferred_loads = Arc::new(Mutex::new(Vec::new()));
    let mut handler = IsolatedContextHandler::new(
        require_proxy,
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
    // If CEF initialized the context synchronously, apply & verify prefs immediately.
    if initialized.load(Ordering::Acquire) {
        let ok = apply_context_prefs(&ctx, proxy.as_ref(), require_proxy);
        prefs_ok.store(ok, Ordering::Release);
    }
    Ok(ContextEntry {
        ctx,
        tabs: HashSet::new(),
        require_proxy,
        initialized,
        prefs_ok,
        proxy: proxy_arc,
        deferred_loads,
    })
}
