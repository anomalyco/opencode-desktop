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
//!   live authenticated loopback IP-literal tunnel proxy** (`browser.proxy.required` if absent,
//!   `browser.proxy.invalid` if non-IP / non-loopback / missing credentials).
//!
//! ## Fail-Closed Pre-Init Gating, Readback Verification & Transient vs Fatal Lifecycle
//! - **Pre-init & suspended request gate (`ContextGate` + `ProxyGateResourceFilter`):** Every
//!   isolated context carries a shared `ContextGate` (`initialized`, `prefs_ok`, `suspended`,
//!   `deferred_initial`). Until `initialized && prefs_ok && !suspended`:
//!   - `IsolatedContextHandler::resource_request_handler` cancels EVERY non-`about:blank` request
//!     (frames, sub-frames, `<meta refresh>`, `fetch`, `XHR`, `WebSocket`, Web Workers, beacons).
//!   - `TabRequestHandler::on_before_browse` and `TabResourceRequestHandler::on_before_resource_load`
//!     enforce the same gate.
//!   - `EngineManager` rejects `CdpCall`, `ContextMenuAction`, `ClearSite*`, `SiteCookies`, `Back`,
//!     `Forward`, and `Reload`, and queues `Navigate` (while `!initialized`) or refuses it (while
//!     `suspended`).
//! - **Preference readback verification (`apply_context_prefs`):** After `set_preference`, every
//!   required preference (`proxy` dictionary `mode`/`server`/`bypass_list`, `webrtc.ip_handling_policy`,
//!   `net.quic_allowed`, `net.network_prediction_options`) is read back via `ctx.preference(...)`
//!   and asserted to equal the written value (fail closed on any mismatch).
//! - **Transient tunnel loss vs fatal/revocation teardown:**
//!   - `SetPartitionProxy(None)` (transient tunnel drop/sleep/reconnect): suspends the live context
//!     (`suspended = true`), closes connections, and emits `Refused("browser.proxy.required")` so
//!     zero bytes escape while preserving the user's open tabs and unsaved page state until the
//!     tunnel reconnects (`SetPartitionProxy(Some(new_cfg))`).
//!   - Fatal preference failure (`prefs_ok == false` on init or failed proxy update) or explicit
//!     revocation (`DropContext`): refuses and closes affected live tabs and drops the context.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    Browser, Callback, CefString, CompletionCallback, Frame, ImplCompletionCallback,
    ImplCookieManager, ImplDictionaryValue, ImplPreferenceManager, ImplRequest,
    ImplRequestContext, ImplRequestContextHandler, ImplResourceRequestHandler, ImplValue, Request,
    RequestContext, RequestContextHandler, RequestContextSettings, ResourceRequestHandler,
    Response, ReturnValue, WrapCompletionCallback, WrapRequestContextHandler,
    WrapResourceRequestHandler, sys, wrap_completion_callback, wrap_request_context_handler,
    wrap_resource_request_handler,
};

wrap_completion_callback! {
    struct NoopCompletion;
    impl CompletionCallback {
        fn on_complete(&self) {}
    }
}

/// Closes all active connections on `ctx`. CEF 156.0.26 can fault with `0xC0000005` when
/// `close_all_connections` is invoked with a NULL callback on a live connected `RequestContext`,
/// so we always pass a valid `CompletionCallback`.
fn close_connections(ctx: &RequestContext) {
    let mut cb = NoopCompletion::new();
    ctx.close_all_connections(Some(&mut cb));
}

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

/// Live atomic gate shared between `ContextPool`, `IsolatedContextHandler`, `TabRequestHandler`,
/// `TabResourceRequestHandler`, and `TabLoadHandler`.
#[derive(Clone)]
pub struct ContextGate {
    pub require_proxy: bool,
    pub initialized: Arc<AtomicBool>,
    pub prefs_ok: Arc<AtomicBool>,
    pub suspended: Arc<AtomicBool>,
    /// True while a newly opened tab is sitting on placeholder `about:blank` waiting for its
    /// initial deferred URL to start loading (prevents premature `loading: false` settles).
    pub deferred_initial: Arc<AtomicBool>,
}

impl ContextGate {
    /// Whether this context has finished applying & verifying all required preferences and is not
    /// currently suspended due to transient proxy loss.
    pub fn is_network_allowed(&self) -> bool {
        !self.suspended.load(Ordering::Acquire)
            && self.initialized.load(Ordering::Acquire)
            && self.prefs_ok.load(Ordering::Acquire)
    }

    pub fn refusal_reason(&self) -> &'static str {
        if self.suspended.load(Ordering::Acquire) {
            "browser.proxy.required"
        } else if self.initialized.load(Ordering::Acquire) && !self.prefs_ok.load(Ordering::Acquire)
        {
            "browser.proxy.invalid"
        } else if self.require_proxy {
            "browser.proxy.required"
        } else {
            "browser.context.unavailable"
        }
    }
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

/// Validates that a `RemoteProxyConfig` names a strict loopback IP literal (`127.0.0.1` / `::1`,
/// never a hostname like `localhost`), non-zero port, and non-empty Basic credentials.
pub fn validate_proxy(cfg: &RemoteProxyConfig) -> Result<(), &'static str> {
    browser_policy::validate_proxy_endpoint(
        &cfg.host,
        cfg.port,
        cfg.username.as_deref(),
        cfg.password.as_deref(),
    )
}

/// Formats `cfg`'s loopback IP and port into a valid Chromium `fixed_servers` URL (bracketing IPv6).
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
/// `RemoteServer` contexts) AND reads every required preference back via `ctx.preference(...)` to
/// verify the written values actually took effect.
fn apply_context_prefs(
    ctx: &RequestContext,
    proxy: Option<&RemoteProxyConfig>,
    require_proxy: bool,
) -> bool {
    let mut ok = true;
    match proxy {
        Some(cfg) => {
            if !set_and_verify_proxy_pref(ctx, cfg) {
                ok = false;
            }
        }
        None if require_proxy => {
            return false;
        }
        None => {}
    }
    ok &= set_and_verify_string_pref(ctx, "webrtc.ip_handling_policy", "disable_non_proxied_udp");
    ok &= set_and_verify_bool_pref(ctx, "net.quic_allowed", false);
    // 2 == NETWORK_PREDICTION_NEVER in Chromium's PrefService (supersedes `dns_prefetching.enabled`).
    ok &= set_and_verify_int_pref(ctx, "net.network_prediction_options", 2);
    // Optional legacy/global prefs: enforced + verified if registered on a CEF build.
    ok &= set_optional_bool_pref(ctx, "webrtc.multiple_routes_enabled", false);
    ok &= set_optional_bool_pref(ctx, "webrtc.nonproxied_udp_enabled", false);
    ok &= set_optional_string_pref(ctx, "dns_over_https.mode", "off");
    ok &= set_optional_bool_pref(ctx, "dns_prefetching.enabled", false);
    ok
}

fn set_and_verify_proxy_pref(ctx: &RequestContext, cfg: &RemoteProxyConfig) -> bool {
    let key = CefString::from("proxy");
    if ctx.has_preference(Some(&key)) == 0 || ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    let Some(mut dict) = cef::dictionary_value_create() else { return false };
    let server = format_proxy_server(cfg);
    dict.set_string(Some(&CefString::from("mode")), Some(&CefString::from("fixed_servers")));
    dict.set_string(Some(&CefString::from("server")), Some(&CefString::from(server.as_str())));
    dict.set_string(Some(&CefString::from("bypass_list")), Some(&CefString::from("<-loopback>")));
    let Some(mut val) = cef::value_create() else { return false };
    val.set_dictionary(Some(&mut dict));
    // `CefString::default()` maps to NULL in `cef-rs`, which CEF's `SetPreference` rejects.
    let mut err = CefString::from(" ");
    if ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) == 0 {
        eprintln!("context: set_preference(proxy) failed: {err}");
        return false;
    }
    // Read back effective preference value and verify every field matches what was written.
    let Some(read_val) = ctx.preference(Some(&key)) else { return false };
    let Some(read_dict) = read_val.dictionary() else { return false };
    let got_mode = CefString::from(&read_dict.string(Some(&CefString::from("mode")))).to_string();
    let got_server = CefString::from(&read_dict.string(Some(&CefString::from("server")))).to_string();
    let got_bypass = CefString::from(&read_dict.string(Some(&CefString::from("bypass_list")))).to_string();
    got_mode == "fixed_servers" && got_server == server && got_bypass == "<-loopback>"
}

fn set_and_verify_string_pref(ctx: &RequestContext, name: &str, value: &str) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 || ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    let Some(mut val) = cef::value_create() else { return false };
    val.set_string(Some(&CefString::from(value)));
    let mut err = CefString::from(" ");
    if ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) == 0 {
        return false;
    }
    ctx.preference(Some(&key))
        .is_some_and(|v| CefString::from(&v.string()).to_string() == value)
}

fn set_and_verify_bool_pref(ctx: &RequestContext, name: &str, value: bool) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 || ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    let Some(mut val) = cef::value_create() else { return false };
    val.set_bool(i32::from(value));
    let mut err = CefString::from(" ");
    if ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) == 0 {
        return false;
    }
    ctx.preference(Some(&key))
        .is_some_and(|v| (v.bool() != 0) == value)
}

fn set_and_verify_int_pref(ctx: &RequestContext, name: &str, value: i32) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 || ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    let Some(mut val) = cef::value_create() else { return false };
    val.set_int(value);
    let mut err = CefString::from(" ");
    if ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) == 0 {
        return false;
    }
    ctx.preference(Some(&key))
        .is_some_and(|v| v.int() == value)
}

fn set_optional_string_pref(ctx: &RequestContext, name: &str, value: &str) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 {
        return true;
    }
    set_and_verify_string_pref(ctx, name, value)
}

fn set_optional_bool_pref(ctx: &RequestContext, name: &str, value: bool) -> bool {
    let key = CefString::from(name);
    if ctx.has_preference(Some(&key)) == 0 {
        return true;
    }
    set_and_verify_bool_pref(ctx, name, value)
}

// Fail-closed context-wide `ResourceRequestHandler` blocking EVERY non-`about:blank` network
// request (frames, sub-frames, `<meta refresh>`, `fetch`, `XHR`, `WebSocket`, workers, beacons)
// while the context is uninitialized, failed verification, or suspended.
wrap_resource_request_handler! {
    struct ProxyGateResourceFilter {
        gate: ContextGate,
    }

    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            if self.gate.is_network_allowed() {
                return ReturnValue::CONTINUE;
            }
            let url = request
                .map(|r| CefString::from(&r.url()).to_string())
                .unwrap_or_default();
            if url.trim().eq_ignore_ascii_case("about:blank") {
                ReturnValue::CONTINUE
            } else {
                ReturnValue::CANCEL
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
            if !self.gate.is_network_allowed()
                && let Some(target) = new_url
            {
                *target = CefString::from("");
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

/// Outcome produced once a deferred context finishes `on_request_context_initialized`.
pub enum DeferredOutcome {
    Load { tab_id: u64, generation: u64, url: String },
    Refuse { tab_id: u64, generation: u64, reason: &'static str },
}

/// Result of updating a remote server's loopback proxy via `set_partition_proxy`.
pub enum ProxyUpdate {
    /// New proxy verified and applied to the remote server (unsuspends live tabs if suspended).
    Applied(RemoteProxyConfig),
    /// Transient proxy loss (`proxy: None`): live tabs are suspended (network blocked fail-closed)
    /// without destroying open pages or unsaved user state.
    Suspended(Vec<u64>),
}

/// Failure from `set_partition_proxy` or `resolve_with_hooks` proxy update; `close_tabs` lists
/// any live tabs left in a broken context that MUST be refused & closed immediately.
pub struct ProxyFatal {
    pub reason: &'static str,
    pub close_tabs: Vec<u64>,
}

struct ContextEntry {
    ctx: RequestContext,
    tabs: HashSet<u64>,
    gate: ContextGate,
    proxy: Arc<Mutex<Option<RemoteProxyConfig>>>,
    /// `(tab_id, generation, url)` loads waiting for `on_request_context_initialized` to finish applying prefs.
    deferred_loads: Arc<Mutex<Vec<(u64, u64, String)>>>,
}

impl ContextEntry {
    fn teardown(&self) {
        self.gate.suspended.store(true, Ordering::Release);
        self.deferred_loads.lock().unwrap().clear();
        close_connections(&self.ctx);
        if let Some(mgr) = self.ctx.cookie_manager(None) {
            let _ = mgr.delete_cookies(None, None, None);
        }
    }
}

wrap_request_context_handler! {
    struct IsolatedContextHandler {
        gate: ContextGate,
        proxy: Arc<Mutex<Option<RemoteProxyConfig>>>,
        ready_loads: Arc<Mutex<Vec<DeferredOutcome>>>,
        deferred_loads: Arc<Mutex<Vec<(u64, u64, String)>>>,
        resource_filter: Option<ResourceHook>,
        pump: Sender<crate::Event>,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            let proxy = self.proxy.lock().unwrap().clone();
            let applied = request_context.is_some_and(|ctx| {
                apply_context_prefs(ctx, proxy.as_ref(), self.gate.require_proxy)
            });
            self.gate.prefs_ok.store(applied, Ordering::Release);
            self.gate.initialized.store(true, Ordering::Release);
            let pending = std::mem::take(&mut *self.deferred_loads.lock().unwrap());
            if !pending.is_empty() {
                let mut out = self.ready_loads.lock().unwrap();
                for (tab_id, generation, url) in pending {
                    if applied {
                        if !self.gate.suspended.load(Ordering::Acquire) {
                            out.push(DeferredOutcome::Load { tab_id, generation, url });
                        }
                    } else {
                        out.push(DeferredOutcome::Refuse {
                            tab_id,
                            generation,
                            reason: if self.gate.require_proxy {
                                "browser.proxy.invalid"
                            } else {
                                "browser.context.unavailable"
                            },
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
            // While uninitialized, failed verification, or suspended, cancel EVERY non-about:blank
            // request context-wide (frames, sub-frames, fetch, XHR, WebSocket, workers, beacons).
            if !self.gate.is_network_allowed() {
                return Some(ProxyGateResourceFilter::new(self.gate.clone()));
            }
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
    /// Monotonic generation per pooled `tab_id` so stale `DeferredOutcome`s can never apply to a reused ID.
    tab_gens: HashMap<u64, u64>,
    next_gen: u64,
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
    pub gate: Option<ContextGate>,
    /// When `true`, open on `about:blank` and defer `initial_url` until context preferences verify.
    pub defer_initial_load: bool,
}

impl ContextPool {
    pub fn new(pump: Sender<crate::Event>) -> Self {
        Self {
            entries: HashMap::new(),
            tab_keys: HashMap::new(),
            tab_gens: HashMap::new(),
            next_gen: 1,
            remote_proxies: HashMap::new(),
            ready_loads: Arc::new(Mutex::new(Vec::new())),
            pump,
        }
    }

    pub fn current_generation(&self, tab_id: u64) -> Option<u64> {
        self.tab_gens.get(&tab_id).copied()
    }

    /// Resolves (or creates) the partitioned `CefRequestContext` for `kind` with default hooks.
    pub fn resolve(
        &mut self,
        kind: &ContextKind,
        tab_id: u64,
        initial_url: &str,
    ) -> Result<ResolvedContext, ProxyFatal> {
        self.resolve_with_hooks(kind, tab_id, initial_url, ContextHooks::default())
    }

    /// Resolves (or creates) the partitioned `CefRequestContext` for `kind`, attaching optional
    /// PREVIEWS scheme/resource hooks. Always fails closed (never falls back to `Default` and
    /// never opens a `RemoteServer` tab without a valid authenticated loopback IP proxy).
    pub fn resolve_with_hooks(
        &mut self,
        kind: &ContextKind,
        tab_id: u64,
        initial_url: &str,
        hooks: ContextHooks,
    ) -> Result<ResolvedContext, ProxyFatal> {
        let fatal = |reason: &'static str| ProxyFatal {
            reason,
            close_tabs: Vec::new(),
        };
        let (key, explicit_proxy, require_proxy) = match kind {
            ContextKind::Default => {
                return Ok(ResolvedContext {
                    request_context: None,
                    proxy: None,
                    gate: None,
                    defer_initial_load: false,
                });
            }
            ContextKind::Preview { workspace, grant } => {
                let ws = workspace.trim();
                let gr = grant.trim();
                if ws.is_empty() || gr.is_empty() {
                    return Err(fatal("browser.context.invalid"));
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
                    return Err(fatal("browser.context.invalid"));
                }
                (ContextKey::Artifact(aid.to_owned()), None, false)
            }
            ContextKind::Private => (ContextKey::Private(tab_id), None, false),
            ContextKind::RemoteServer { server_id, proxy } => {
                let sid = server_id.trim();
                if sid.is_empty() {
                    return Err(fatal("browser.context.invalid"));
                }
                if let Some(cfg) = proxy {
                    validate_proxy(cfg).map_err(fatal)?;
                }
                let effective = match proxy {
                    Some(cfg) => {
                        self.remote_proxies.insert(sid.to_owned(), cfg.clone());
                        cfg.clone()
                    }
                    None => match self.remote_proxies.get(sid).cloned() {
                        Some(cfg) => {
                            validate_proxy(&cfg).map_err(fatal)?;
                            cfg
                        }
                        None => return Err(fatal("browser.proxy.required")),
                    },
                };
                (ContextKey::RemoteServer(sid.to_owned()), Some(effective), true)
            }
        };

        if !self.entries.contains_key(&key) {
            let entry = create_isolated_context(
                explicit_proxy.clone(),
                require_proxy,
                self.ready_loads.clone(),
                self.pump.clone(),
                hooks,
            )
            .map_err(fatal)?;
            self.entries.insert(key.clone(), entry);
        } else if let Some(cfg) = explicit_proxy {
            let needs_teardown = {
                let existing = self.entries.get_mut(&key).unwrap();
                let changed = existing.proxy.lock().unwrap().as_ref() != Some(&cfg)
                    || existing.gate.suspended.load(Ordering::Acquire);
                if changed {
                    *existing.proxy.lock().unwrap() = Some(cfg.clone());
                    if existing.gate.initialized.load(Ordering::Acquire) {
                        let ok = apply_context_prefs(&existing.ctx, Some(&cfg), true);
                        existing.gate.prefs_ok.store(ok, Ordering::Release);
                        existing.gate.suspended.store(!ok, Ordering::Release);
                        close_connections(&existing.ctx);
                        !ok
                    } else {
                        existing.gate.suspended.store(false, Ordering::Release);
                        false
                    }
                } else {
                    false
                }
            };
            if needs_teardown {
                let close_tabs = self.teardown_key(&key);
                return Err(ProxyFatal {
                    reason: "browser.proxy.invalid",
                    close_tabs,
                });
            }
        }

        let is_init = self.entries.get(&key).unwrap().gate.initialized.load(Ordering::Acquire);
        let prefs_ok = self.entries.get(&key).unwrap().gate.prefs_ok.load(Ordering::Acquire);
        if is_init && !prefs_ok {
            let close_tabs = self.teardown_key(&key);
            return Err(ProxyFatal {
                reason: if require_proxy {
                    "browser.proxy.invalid"
                } else {
                    "browser.context.unavailable"
                },
                close_tabs,
            });
        }

        // Only release a prior context key for `tab_id` AFTER the new context validated cleanly.
        if self.tab_keys.get(&tab_id).is_some_and(|prev| prev != &key) {
            let _ = self.release_tab(tab_id);
        }

        let entry = self.entries.get_mut(&key).unwrap();
        let effective_proxy = entry.proxy.lock().unwrap().clone();
        let generation = self.next_gen;
        self.next_gen = self.next_gen.wrapping_add(1).max(1);
        entry.tabs.insert(tab_id);
        self.tab_keys.insert(tab_id, key);
        self.tab_gens.insert(tab_id, generation);

        // Defer ALL isolated contexts (not just `RemoteServer`) until `on_request_context_initialized`
        // finishes verifying lockdown + proxy prefs so even Tab 1 cannot load un-locked.
        let need_defer = !is_init;
        let tab_gate = ContextGate {
            require_proxy: entry.gate.require_proxy,
            initialized: entry.gate.initialized.clone(),
            prefs_ok: entry.gate.prefs_ok.clone(),
            suspended: entry.gate.suspended.clone(),
            deferred_initial: Arc::new(AtomicBool::new(
                need_defer && !initial_url.eq_ignore_ascii_case("about:blank"),
            )),
        };
        if need_defer {
            let mut loads = entry.deferred_loads.lock().unwrap();
            loads.retain(|(id, _, _)| *id != tab_id);
            loads.push((tab_id, generation, initial_url.to_owned()));
        }

        Ok(ResolvedContext {
            request_context: Some(entry.ctx.clone()),
            proxy: effective_proxy,
            gate: Some(tab_gate),
            defer_initial_load: need_defer && !initial_url.eq_ignore_ascii_case("about:blank"),
        })
    }

    /// Gates `HostMsg::Navigate` for `tab_id`. Fails closed whenever an isolated tab is missing
    /// from `tab_keys`, suspended, or failed preference verification. Prunes stale `ready_loads`.
    pub fn gate_navigation(
        &mut self,
        tab_id: u64,
        kind: &ContextKind,
        url: &str,
    ) -> Result<bool, &'static str> {
        if kind.is_default() {
            return Ok(true);
        }
        // Any newer explicit navigation supersedes any earlier deferred `ready_loads` entry.
        self.prune_ready_loads(tab_id);
        let Some(key) = self.tab_keys.get(&tab_id) else {
            return Err("browser.context.unavailable");
        };
        let Some(entry) = self.entries.get(key) else {
            return Err("browser.context.unavailable");
        };
        if entry.gate.suspended.load(Ordering::Acquire) {
            return Err("browser.proxy.required");
        }
        if !entry.gate.initialized.load(Ordering::Acquire) {
            let generation = self.tab_gens.get(&tab_id).copied().unwrap_or(0);
            let mut loads = entry.deferred_loads.lock().unwrap();
            loads.retain(|(id, _, _)| *id != tab_id);
            loads.push((tab_id, generation, url.to_owned()));
            return Ok(false);
        }
        if !entry.gate.prefs_ok.load(Ordering::Acquire) {
            return Err(if entry.gate.require_proxy {
                "browser.proxy.invalid"
            } else {
                "browser.context.unavailable"
            });
        }
        Ok(true)
    }

    /// Whether `tab_id`'s context has completed & verified initialization and is not suspended.
    /// Fails closed (`false`) if an isolated tab is missing from `tab_keys`.
    pub fn is_ready(&self, tab_id: u64, kind: &ContextKind) -> bool {
        if kind.is_default() {
            return true;
        }
        let Some(key) = self.tab_keys.get(&tab_id) else {
            return false;
        };
        self.entries
            .get(key)
            .is_some_and(|e| e.gate.is_network_allowed())
    }

    /// Releases `tab_id` from its pooled context, pruning any queued `ready_loads`. When the last
    /// tab closes, removes the entry, closes connections, deletes cookies, and drops the context.
    pub fn release_tab(&mut self, tab_id: u64) -> Option<ContextKey> {
        self.prune_ready_loads(tab_id);
        self.tab_gens.remove(&tab_id);
        let key = self.tab_keys.remove(&tab_id)?;
        let empty = if let Some(entry) = self.entries.get_mut(&key) {
            entry.tabs.remove(&tab_id);
            entry.deferred_loads.lock().unwrap().retain(|(id, _, _)| *id != tab_id);
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
        self.tab_gens.clear();
        self.remote_proxies.clear();
        self.ready_loads.lock().unwrap().clear();
        for (_, entry) in self.entries.drain() {
            entry.teardown();
        }
    }

    /// Updates or suspends the local loopback proxy on a remote server partition:
    /// - `proxy: None` (transient tunnel loss): suspends the live context without destroying open
    ///   tabs (blocking all requests/CDP/menu/nav fail-closed until a new proxy is supplied).
    /// - `proxy: Some(cfg)`: validates strictly, applies & verifies readback, and unsuspends; on
    ///   fatal verification failure tears down the broken context and returns `close_tabs`.
    pub fn set_partition_proxy(
        &mut self,
        server_id: &str,
        proxy: Option<RemoteProxyConfig>,
    ) -> Result<ProxyUpdate, ProxyFatal> {
        let sid = server_id.trim();
        if sid.is_empty() {
            return Err(ProxyFatal {
                reason: "browser.context.invalid",
                close_tabs: Vec::new(),
            });
        }
        let key = ContextKey::RemoteServer(sid.to_owned());
        let Some(cfg) = proxy else {
            self.remote_proxies.remove(sid);
            let mut suspended_tabs = Vec::new();
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.gate.suspended.store(true, Ordering::Release);
                *entry.proxy.lock().unwrap() = None;
                entry.deferred_loads.lock().unwrap().clear();
                close_connections(&entry.ctx);
                suspended_tabs.extend(entry.tabs.iter().copied());
            }
            for &tid in &suspended_tabs {
                self.prune_ready_loads(tid);
            }
            return Ok(ProxyUpdate::Suspended(suspended_tabs));
        };
        validate_proxy(&cfg).map_err(|reason| ProxyFatal {
            reason,
            close_tabs: Vec::new(),
        })?;
        self.remote_proxies.insert(sid.to_owned(), cfg.clone());
        let failed = if let Some(entry) = self.entries.get_mut(&key) {
            *entry.proxy.lock().unwrap() = Some(cfg.clone());
            if entry.gate.initialized.load(Ordering::Acquire) {
                let ok = apply_context_prefs(&entry.ctx, Some(&cfg), true);
                entry.gate.prefs_ok.store(ok, Ordering::Release);
                entry.gate.suspended.store(!ok, Ordering::Release);
                close_connections(&entry.ctx);
                !ok
            } else {
                entry.gate.suspended.store(false, Ordering::Release);
                false
            }
        } else {
            false
        };
        if failed {
            let close_tabs = self.teardown_key(&key);
            return Err(ProxyFatal {
                reason: "browser.proxy.invalid",
                close_tabs,
            });
        }
        Ok(ProxyUpdate::Applied(cfg))
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

    fn teardown_key(&mut self, key: &ContextKey) -> Vec<u64> {
        let Some(entry) = self.entries.remove(key) else {
            return Vec::new();
        };
        let tabs: Vec<u64> = entry.tabs.iter().copied().collect();
        for &tid in &tabs {
            self.tab_keys.remove(&tid);
            self.tab_gens.remove(&tid);
            self.prune_ready_loads(tid);
        }
        entry.teardown();
        tabs
    }

    fn prune_ready_loads(&self, tab_id: u64) {
        self.ready_loads.lock().unwrap().retain(|outcome| match outcome {
            DeferredOutcome::Load { tab_id: id, .. } | DeferredOutcome::Refuse { tab_id: id, .. } => {
                *id != tab_id
            }
        });
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

    /// Explicitly drops a pooled context (for revocation / sign-out / server removal).
    /// Callers MUST close `tabs_for_context(kind)` first so Chromium drops the `BrowserContext`.
    pub fn drop_context(&mut self, kind: &ContextKind) {
        if let ContextKind::RemoteServer { server_id, .. } = kind {
            self.remote_proxies.remove(server_id.trim());
        }
        if let Some(k) = Self::key_for_drop(kind) {
            let _ = self.teardown_key(&k);
        }
    }

    /// Drains any initial tab URLs (or fail-closed refusals) whose `RequestContext` finished init.
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
    let gate = ContextGate {
        require_proxy,
        initialized: Arc::new(AtomicBool::new(false)),
        prefs_ok: Arc::new(AtomicBool::new(false)),
        suspended: Arc::new(AtomicBool::new(false)),
        deferred_initial: Arc::new(AtomicBool::new(false)),
    };
    let proxy_arc = Arc::new(Mutex::new(proxy));
    let deferred_loads = Arc::new(Mutex::new(Vec::new()));
    let mut handler = IsolatedContextHandler::new(
        gate.clone(),
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
    Ok(ContextEntry {
        ctx,
        tabs: HashSet::new(),
        gate,
        proxy: proxy_arc,
        deferred_loads,
    })
}
