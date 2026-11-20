//! Per-preview `ContextPool` hook builder and context-wide `ResourceRequestHandler` (covering
//! frames, Web Workers, `sendBeacon`, WebSockets, and subresources even when `frame` is `None`).
//!
//! Uses `crate::context::ContextPool::resolve_with_hooks` so `context.rs` remains the sole caller
//! of `cef::request_context_create_context` and initial `oc-preview://` loads wait for
//! `on_request_context_initialized` lockdown prefs before navigating (`notes/review-rd281.md` #2).

use std::sync::Arc;

use browser_policy::preview::{
    is_preview_scheme, preview_initiator_allowed, preview_subresource_allowed,
};
use cef::rc::Rc as _;
use cef::{
    Browser, Callback, CefString, Frame, ImplFrame, ImplRequest, ImplRequestContext,
    ImplResourceRequestHandler, Request, ResourceRequestHandler, Response, ReturnValue,
    WrapResourceRequestHandler, wrap_resource_request_handler,
};

use super::handler::PreviewSchemeHandlerFactory;
use super::{PreviewGrant, PreviewRegistry, scheme};
use crate::context::ContextHooks;
use crate::output::Output;

/// Builds `ContextPool` creation hooks for an isolated preview/artifact context:
/// - `exclude_default_cookie_schemes: true` and `require_lockdown_prefs: true` (defers initial
///   navigation until `on_request_context_initialized` verifies lockdown prefs).
/// - Registers `PreviewSchemeHandlerFactory` **only** on this context for **only** `grant_host`.
/// - Installs `handle_resource_request` as the context-wide `ResourceRequestHandler` covering
///   Web Workers, `sendBeacon`, and non-frame requests (`frame == None`).
pub fn preview_context_hooks(
    grant: Arc<PreviewGrant>,
    registry: Arc<PreviewRegistry>,
    out_tx: Output,
) -> ContextHooks {
    let filter_grant = grant.clone();
    let factory_grant = grant;
    ContextHooks {
        resource_filter: Some(Arc::new(move |_browser, frame, request, is_navigation, _is_download, initiator, _disable| {
            handle_resource_request(&filter_grant, frame, request, is_navigation, initiator)
        })),
        exclude_default_cookie_schemes: true,
        require_lockdown_prefs: true,
        on_created: Some(Box::new(move |ctx| {
            let mut factory = PreviewSchemeHandlerFactory::new(factory_grant.clone(), registry, out_tx);
            let _ = ctx.register_scheme_handler_factory(
                Some(&CefString::from(scheme::PREVIEW_SCHEME)),
                Some(&CefString::from(factory_grant.grant_host.as_str())),
                Some(&mut factory),
            );
        })),
    }
}

/// Shared filter invoked by BOTH `TabRequestHandler::resource_request_handler` (frame requests)
/// and `ContextPool`'s `IsolatedContextHandler::resource_request_handler` (workers, beacons,
/// non-frame requests) so no request in a preview context can bypass `preview_subresource_allowed`
/// or `request_initiator` verification.
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
    // Only a main-frame navigation accepts an empty/`"null"` initiator; ALL subresources, workers,
    // and subframes MUST carry the exact `oc-preview://<grant_host>` initiator (`T5`/`T6`).
    let browser_initiated = is_main_nav && matches!(trimmed, None | Some("") | Some("null"));
    let initiator_ok = browser_initiated || preview_initiator_allowed(trimmed, &grant.grant_host);
    if let Some(req) = request {
        let req_id = req.identifier();
        let url = CefString::from(&req.url()).to_string();
        // Only record valid, allowed same-grant `oc-preview://` requests (with non-zero id) so
        // foreign/blocked URLs that `PreviewResourceFilter` cancels can never pollute `InitiatorCache`.
        if req_id != 0
            && is_preview_scheme(&url)
            && preview_subresource_allowed(&url, &grant.grant_host)
            && initiator_ok
        {
            grant.record_initiator(req_id, browser_initiated, true);
        }
    }
    Some(PreviewResourceFilter::new(grant.clone(), initiator_ok))
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
            let Some(req) = request else {
                return ReturnValue::CANCEL;
            };
            let url = CefString::from(&req.url()).to_string();
            if !preview_subresource_allowed(&url, &self.grant.grant_host) {
                return ReturnValue::CANCEL;
            }
            if is_preview_scheme(&url) && (!self.initiator_ok || req.identifier() == 0) {
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
