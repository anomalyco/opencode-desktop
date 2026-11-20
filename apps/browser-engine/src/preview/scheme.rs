//! Shared `oc-preview` custom scheme registration invoked by BOTH the browser process
//! (`SidecarCefApp`) and every renderer/helper subprocess (`opencode_browser_engine_helper`).
//!
//! Security properties (`packages/gui-plugins/src/schemes.ts`, #63002):
//! - `STANDARD`: standard `scheme://host/path` origin parsing (per-grant random UUID host).
//! - `DISPLAY_ISOLATED`: other schemes (`http:`, `https:`, `file:`) cannot embed/link `oc-preview:`.
//! - `SECURE`: treated as a secure context like `https:`.
//! - `CORS_ENABLED` + `FETCH_ENABLED`: same-origin `fetch()` and ES modules work inside the grant.
//! - Deliberately NOT `LOCAL` and NOT `CSP_BYPASSING`.

use cef::{CefString, ImplSchemeRegistrar, SchemeRegistrar, sys};

pub const PREVIEW_SCHEME: &str = "oc-preview";

pub fn register(registrar: Option<&mut SchemeRegistrar>) {
    let Some(registrar) = registrar else { return };
    use sys::cef_scheme_options_t as O;
    let options = (O::CEF_SCHEME_OPTION_STANDARD as i32)
        | (O::CEF_SCHEME_OPTION_DISPLAY_ISOLATED as i32)
        | (O::CEF_SCHEME_OPTION_SECURE as i32)
        | (O::CEF_SCHEME_OPTION_CORS_ENABLED as i32)
        | (O::CEF_SCHEME_OPTION_FETCH_ENABLED as i32);
    let _ = registrar.add_custom_scheme(Some(&CefString::from(PREVIEW_SCHEME)), options);
}
