//! Shared isolated `CefRequestContext` builder for preview, artifact, private, and remote tabs.
//!
//! Tabs opened in an isolated `RequestContext` run in a separate in-memory Chromium profile
//! (`cache_path` empty, `persist_session_cookies: 0`, `persist_user_preferences: 0`): extensions
//! loaded in the default profile cannot inject content scripts, intercept requests via DNR or
//! `webRequest`, read cookies, or enumerate these tabs.

use cef::{CefString, RequestContext, RequestContextSettings};

pub fn create_isolated_context() -> Option<RequestContext> {
    let settings = RequestContextSettings {
        cache_path: CefString::default(),
        persist_session_cookies: 0,
        ..Default::default()
    };
    cef::request_context_create_context(Some(&settings), None)
}
