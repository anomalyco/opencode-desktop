//! Per-preview isolated `CefRequestContext` creation, privacy/network preference lockdown, and
//! context-wide `ResourceRequestHandler` (covering Web Workers, `sendBeacon`, WebSockets, and
//! subresources even when `browser`/`frame` is `None`).

use std::sync::Arc;

use browser_policy::preview::{
    is_preview_scheme, preview_initiator_allowed, preview_subresource_allowed,
};
use cef::rc::Rc as _;
use cef::{
    Browser, Callback, CefString, Frame, ImplFrame, ImplPreferenceManager, ImplRequest,
    ImplRequestContext, ImplRequestContextHandler, ImplResourceRequestHandler, ImplValue, Request,
    RequestContext, RequestContextHandler, RequestContextSettings, ResourceRequestHandler, Response,
    ReturnValue, WrapRequestContextHandler, WrapResourceRequestHandler,
    wrap_request_context_handler, wrap_resource_request_handler,
};

use super::handler::PreviewSchemeHandlerFactory;
use super::{PreviewGrant, PreviewRegistry, scheme};
use crate::output::Output;

/// Creates a brand-new ephemeral in-memory `CefRequestContext` dedicated solely to `grant`:
/// - Empty `cache_path`, `persist_session_cookies: 0`, `persist_user_preferences: 0`: shares ZERO
///   cookies, `localStorage`, `IndexedDB`, `CacheStorage`, HTTP cache, or extensions with the
///   signed-in profile or any other preview tab.
/// - Registers `PreviewSchemeHandlerFactory` ONLY on this context for ONLY `grant.grant_host`.
/// - Attaches `PreviewRequestContextHandler` to disable WebRTC non-proxied UDP + DNS prefetching
///   and enforce `preview_subresource_allowed` across all frames, workers, and beacons.
pub fn create_preview_context(
    grant: Arc<PreviewGrant>,
    registry: Arc<PreviewRegistry>,
    out_tx: Output,
) -> Option<RequestContext> {
    let settings = RequestContextSettings {
        cache_path: CefString::default(),
        persist_session_cookies: 0,
        cookieable_schemes_exclude_defaults: 1,
        ..Default::default()
    };
    let mut ctx_handler = PreviewRequestContextHandler::new(grant.clone());
    let ctx = cef::request_context_create_context(Some(&settings), Some(&mut ctx_handler))?;
    let mut factory = PreviewSchemeHandlerFactory::new(grant.clone(), registry, out_tx);
    let _ = ctx.register_scheme_handler_factory(
        Some(&CefString::from(scheme::PREVIEW_SCHEME)),
        Some(&CefString::from(grant.grant_host.as_str())),
        Some(&mut factory),
    );
    Some(ctx)
}

/// Shared filter invoked by BOTH `TabRequestHandler::resource_request_handler` (frame requests)
/// and `PreviewRequestContextHandler::resource_request_handler` (workers, beacons, non-frame
/// requests) so no request in a preview context can bypass `preview_subresource_allowed` or
/// `request_initiator` verification.
pub fn handle_resource_request(
    grant: &Arc<PreviewGrant>,
    frame: Option<&mut Frame>,
    request: Option<&mut Request>,
    is_navigation: i32,
    request_initiator: Option<&CefString>,
) -> Option<ResourceRequestHandler> {
    let initiator = request_initiator.map(CefString::to_string);
    let trimmed = initiator.as_deref().map(str::trim);
    let is_main_nav = is_navigation != 0 && frame.is_some_and(|f| f.is_main() == 1);
    // CEF serializes an empty/browser-initiated navigation origin as `"null"` in `request_initiator`.
    // Only a main-frame navigation accepts an empty/`"null"` initiator; subresources, workers, and
    // sandboxed subframes with `"null"` initiator are strictly refused (`T5`).
    let browser_initiated = is_main_nav && matches!(trimmed, None | Some("") | Some("null"));
    let initiator_ok = browser_initiated || preview_initiator_allowed(trimmed, &grant.grant_host);
    if let Some(req) = request {
        let url = CefString::from(&req.url()).to_string();
        if is_preview_scheme(&url) {
            grant.record_initiator(req.identifier(), browser_initiated, initiator_ok);
        }
    }
    Some(PreviewResourceFilter::new(grant.clone(), initiator_ok))
}

wrap_request_context_handler! {
    struct PreviewRequestContextHandler {
        grant: Arc<PreviewGrant>,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            let Some(ctx) = request_context else { return };
            // Lock down WebRTC IP leaks / UDP STUN and DNS prefetching / network prediction on
            // this preview context (`T7`, `T8`).
            set_string_pref(ctx, "webrtc.ip_handling_policy", "disable_non_proxied_udp");
            set_bool_pref(ctx, "webrtc.multiple_routes_enabled", false);
            set_bool_pref(ctx, "webrtc.nonproxied_udp_enabled", false);
            // 2 == NETWORK_PREDICTION_NEVER in Chromium's PrefService.
            set_int_pref(ctx, "net.network_prediction_options", 2);
            set_bool_pref(ctx, "dns_prefetching.enabled", false);
        }

        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int,
            request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            handle_resource_request(&self.grant, frame, request, is_navigation, request_initiator)
        }
    }
}

wrap_resource_request_handler! {
    pub struct PreviewResourceFilter {
        grant: Arc<PreviewGrant>,
        initiator_ok: bool,
    }

    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            if self.grant.is_revoked() {
                return ReturnValue::CANCEL;
            }
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            if !preview_subresource_allowed(&url, &self.grant.grant_host) {
                return ReturnValue::CANCEL;
            }
            if is_preview_scheme(&url) && !self.initiator_ok {
                return ReturnValue::CANCEL;
            }
            ReturnValue::CONTINUE
        }

        fn on_resource_redirect(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _response: Option<&mut Response>,
            new_url: Option<&mut CefString>,
        ) {
            let Some(new_url) = new_url else { return };
            let target = new_url.to_string();
            if self.grant.is_revoked() || !preview_subresource_allowed(&target, &self.grant.grant_host) {
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

fn set_string_pref(ctx: &RequestContext, name: &str, val: &str) {
    let Some(mut v) = cef::value_create() else { return };
    v.set_string(Some(&CefString::from(val)));
    let mut err = CefString::default();
    let _ = ctx.set_preference(Some(&CefString::from(name)), Some(&mut v), Some(&mut err));
}

fn set_bool_pref(ctx: &RequestContext, name: &str, val: bool) {
    let Some(mut v) = cef::value_create() else { return };
    v.set_bool(i32::from(val));
    let mut err = CefString::default();
    let _ = ctx.set_preference(Some(&CefString::from(name)), Some(&mut v), Some(&mut err));
}

fn set_int_pref(ctx: &RequestContext, name: &str, val: i32) {
    let Some(mut v) = cef::value_create() else { return };
    v.set_int(val);
    let mut err = CefString::default();
    let _ = ctx.set_preference(Some(&CefString::from(name)), Some(&mut v), Some(&mut err));
}
