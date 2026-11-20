//! Shared isolated `CefRequestContext` pool for `Default`, `IsolatedPreview`, and `RemoteServer`
//! tabs (shared across WS-1, PREVIEWS, WS-9 extension isolation, and BROWSER-TOOLS remote proxy
//! confinement; `packages/gui-plugins/src/browser/network.ts`).
//!
//! Security invariants:
//! - `Default` tabs share CEF's global persistent profile context (`OPENCODE_BROWSER_PROFILE`).
//! - `IsolatedPreview { grant }` tabs get a dedicated in-memory `CefRequestContext` with empty
//!   `cache_path` and `persist_session_cookies: 0`: zero shared cookies, zero disk writes, and
//!   invisible to WS-9 extensions.
//! - `RemoteServer { partition, proxy }` tabs get a dedicated in-memory `CefRequestContext` per
//!   `partition` (isolated from `Default` and from other remote servers, invisible to extensions).
//!   When `proxy` is configured, only loopback proxy endpoints (`127.0.0.1` / `localhost` / `[::1]`)
//!   are accepted, `proxy` preference is set to `mode = fixed_servers` with `bypass_list =
//!   <-loopback>` (so localhost resolves through the tunnel on the remote server, never on the
//!   user's machine), `webrtc.ip_handling_policy = disable_non_proxied_udp`, and
//!   `dns_over_https.mode = off`.
//! - First-tab navigations on a newly created proxied context wait for
//!   `on_request_context_initialized` before loading the target URL so zero packets ever race ahead
//!   of `set_preference("proxy", ...)`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    CefString, ImplDictionaryValue, ImplPreferenceManager, ImplRequestContext,
    ImplRequestContextHandler, ImplValue, RequestContext, RequestContextHandler,
    RequestContextSettings, WrapRequestContextHandler, sys, wrap_request_context_handler,
};

use crate::wire::{ContextKind, RemoteProxyConfig};

const DEFAULT_PROXY_REALM: &str = "OpenCode Browser Proxy";

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

/// Applies `fixed_servers` proxy (`<-loopback>`), `disable_non_proxied_udp`, and DoH-off
/// preferences to `ctx`. Returns `true` when the proxy preference (if requested) succeeded.
fn apply_context_prefs(ctx: &RequestContext, proxy: Option<&RemoteProxyConfig>) -> bool {
    let mut ok = true;
    if let Some(cfg) = proxy {
        ok = set_proxy_pref(ctx, Some(cfg));
    } else if ctx.has_preference(Some(&CefString::from("proxy"))) != 0 {
        let _ = set_proxy_pref(ctx, None);
    }
    set_string_pref(ctx, "webrtc.ip_handling_policy", "disable_non_proxied_udp");
    set_string_pref(ctx, "dns_over_https.mode", "off");
    set_bool_pref(ctx, "net.quic_allowed", false);
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

/// Outcome produced once a deferred context finishes `on_request_context_initialized`.
pub enum DeferredOutcome {
    Load { tab_id: u64, url: String },
    Refuse { tab_id: u64, reason: &'static str },
}

struct ContextEntry {
    ctx: RequestContext,
    initialized: Arc<AtomicBool>,
    prefs_ok: Arc<AtomicBool>,
    proxy: Arc<Mutex<Option<RemoteProxyConfig>>>,
    /// `(tab_id, url)` loads waiting for `on_request_context_initialized` to finish applying proxy prefs.
    deferred_loads: Arc<Mutex<Vec<(u64, String)>>>,
}

wrap_request_context_handler! {
    struct IsolatedContextHandler {
        initialized: Arc<AtomicBool>,
        prefs_ok: Arc<AtomicBool>,
        proxy: Arc<Mutex<Option<RemoteProxyConfig>>>,
        ready_loads: Arc<Mutex<Vec<DeferredOutcome>>>,
        deferred_loads: Arc<Mutex<Vec<(u64, String)>>>,
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
    }
}

/// Pool of non-default isolated `CefRequestContext` instances managed by `EngineManager`.
pub struct ContextPool {
    previews: HashMap<String, ContextEntry>,
    remotes: HashMap<String, ContextEntry>,
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
            previews: HashMap::new(),
            remotes: HashMap::new(),
            ready_loads: Arc::new(Mutex::new(Vec::new())),
            pump,
        }
    }
    /// Resolves (or creates) the `CefRequestContext` for `kind`.
    /// Returns `Err(reason)` if a `RemoteServer` specifies an invalid proxy or empty partition.
    pub fn resolve(&mut self, kind: &ContextKind, tab_id: u64, initial_url: &str) -> Result<ResolvedContext, &'static str> {
        match kind {
            ContextKind::Default => Ok(ResolvedContext {
                request_context: None,
                proxy: None,
                defer_initial_load: false,
            }),
            ContextKind::IsolatedPreview { grant } => {
                if grant.trim().is_empty() {
                    return Err("browser.address.web");
                }
                let entry = self.get_or_create_preview(grant)?;
                Ok(ResolvedContext {
                    request_context: Some(entry.ctx.clone()),
                    proxy: None,
                    defer_initial_load: false,
                })
            }
            ContextKind::RemoteServer { partition, proxy } => {
                if partition.trim().is_empty() {
                    return Err("browser.address.web");
                }
                if let Some(cfg) = proxy {
                    validate_proxy(cfg)?;
                }
                let entry = self.get_or_create_remote(partition, proxy.clone())?;
                let effective_proxy = entry.proxy.lock().unwrap().clone();
                let is_init = entry.initialized.load(Ordering::Acquire);
                if effective_proxy.is_some() && is_init && !entry.prefs_ok.load(Ordering::Acquire) {
                    return Err("browser.proxy.invalid");
                }
                let need_defer = effective_proxy.is_some()
                    && !is_init
                    && !initial_url.eq_ignore_ascii_case("about:blank");
                if need_defer {
                    entry.deferred_loads.lock().unwrap().push((tab_id, initial_url.to_owned()));
                }
                Ok(ResolvedContext {
                    request_context: Some(entry.ctx.clone()),
                    proxy: effective_proxy,
                    defer_initial_load: need_defer,
                })
            }
        }
    }

    /// Updates the proxy on an existing or newly created remote server partition, closing active
    /// connections so future traffic immediately uses the new proxy (`network.ts:47`).
    pub fn set_partition_proxy(
        &mut self,
        partition: &str,
        proxy: Option<RemoteProxyConfig>,
    ) -> Result<Option<RemoteProxyConfig>, &'static str> {
        if partition.trim().is_empty() {
            return Err("browser.proxy.invalid");
        }
        if let Some(cfg) = &proxy {
            validate_proxy(cfg)?;
        }
        let entry = self.get_or_create_remote(partition, proxy.clone())?;
        *entry.proxy.lock().unwrap() = proxy.clone();
        if entry.initialized.load(Ordering::Acquire) {
            let ok = apply_context_prefs(&entry.ctx, proxy.as_ref());
            entry.prefs_ok.store(ok, Ordering::Release);
            entry.ctx.close_all_connections(None);
            if proxy.is_some() && !ok {
                return Err("browser.proxy.invalid");
            }
        }
        Ok(proxy)
    }

    /// Returns the live `RequestContext` for a preview grant (for PREVIEWS `oc-preview://` scheme
    /// factory registration on isolated preview contexts).
    #[allow(dead_code)]
    pub fn preview_context(&self, grant: &str) -> Option<RequestContext> {
        self.previews.get(grant).map(|e| e.ctx.clone())
    }

    /// Returns the live `RequestContext` for a remote server partition (for Phase 3b cookie mirroring).
    pub fn remote_context(&self, partition: &str) -> Option<RequestContext> {
        self.remotes.get(partition).map(|e| e.ctx.clone())
    }

    /// Returns all active remote server partitions and their `RequestContext`s.
    pub fn remote_partitions(&self) -> Vec<(String, RequestContext)> {
        self.remotes
            .iter()
            .map(|(k, e)| (k.clone(), e.ctx.clone()))
            .collect()
    }

    /// Closes connections and releases a non-default context when a preview grant or remote
    /// attachment ends.
    pub fn drop_context(&mut self, kind: &ContextKind) {
        let removed = match kind {
            ContextKind::Default => None,
            ContextKind::IsolatedPreview { grant } => self.previews.remove(grant),
            ContextKind::RemoteServer { partition, .. } => self.remotes.remove(partition),
        };
        if let Some(entry) = removed {
            entry.deferred_loads.lock().unwrap().clear();
            entry.ctx.close_all_connections(None);
        }
    }

    /// Drains any initial tab URLs (or fail-closed refusals) whose proxied `RequestContext`
    /// just finished initializing.
    pub fn drain_ready_loads(&self) -> Vec<DeferredOutcome> {
        std::mem::take(&mut *self.ready_loads.lock().unwrap())
    }

    fn get_or_create_preview(&mut self, grant: &str) -> Result<&mut ContextEntry, &'static str> {
        if !self.previews.contains_key(grant) {
            let entry = create_isolated_context(None, self.ready_loads.clone(), self.pump.clone())?;
            self.previews.insert(grant.to_owned(), entry);
        }
        Ok(self.previews.get_mut(grant).unwrap())
    }

    fn get_or_create_remote(
        &mut self,
        partition: &str,
        proxy: Option<RemoteProxyConfig>,
    ) -> Result<&mut ContextEntry, &'static str> {
        if !self.remotes.contains_key(partition) {
            let entry = create_isolated_context(proxy, self.ready_loads.clone(), self.pump.clone())?;
            self.remotes.insert(partition.to_owned(), entry);
            return Ok(self.remotes.get_mut(partition).unwrap());
        }
        let existing = self.remotes.get_mut(partition).unwrap();
        if let Some(cfg) = proxy {
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
        Ok(existing)
    }
}

fn create_isolated_context(
    proxy: Option<RemoteProxyConfig>,
    ready_loads: Arc<Mutex<Vec<DeferredOutcome>>>,
    pump: Sender<crate::Event>,
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
        pump,
    );
    // Empty `cache_path` + `persist_session_cookies: 0` creates a strictly ephemeral in-memory
    // storage/cookie partition isolated from the default profile (`OPENCODE_BROWSER_PROFILE`).
    let settings = RequestContextSettings {
        size: std::mem::size_of::<sys::_cef_request_context_settings_t>(),
        cache_path: CefString::default(),
        persist_session_cookies: 0,
        accept_language_list: CefString::default(),
        cookieable_schemes_list: CefString::default(),
        cookieable_schemes_exclude_defaults: 0,
    };
    let Some(ctx) = cef::request_context_create_context(Some(&settings), Some(&mut handler)) else {
        return Err("browser.context.unavailable");
    };
    // If CEF initialized the context synchronously, apply prefs immediately.
    if initialized.load(Ordering::Acquire) {
        let ok = apply_context_prefs(&ctx, proxy.as_ref());
        prefs_ok.store(ok, Ordering::Release);
    }
    Ok(ContextEntry {
        ctx,
        initialized,
        prefs_ok,
        proxy: proxy_arc,
        deferred_loads,
    })
}
