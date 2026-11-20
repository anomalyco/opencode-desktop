//! `CefSchemeHandlerFactory` and `CefResourceHandler` implementations for `oc-preview://`:
//! - `LocalFileResourceHandler`: opens & verifies local workspace files strictly via the open
//!   handle (`handle_path::open_verified_file`), streams directly from that verified `File` handle
//!   with full `HEAD` + `Range` (`200`/`206`/`416`) support and strict security headers.
//! - `ProxyResourceHandler`: validates remote workspace & artifact URLs/scopes/methods/initiators,
//!   asks the desktop app host for bytes over the private Unix socket (`EngineMsg::PreviewFetch`),
//!   and streams asynchronously into CEF with the exact same security headers — so remote server
//!   credentials never enter the browser engine process.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::sync::{Arc, Mutex};

use browser_policy::preview::{
    ARTIFACT_CSP, CONNECTION_ALLOWLIST, RangeSpec, WORKSPACE_PREVIEW_CSP, artifact_resource,
    mime_for_path, parse_preview_url, parse_range, preview_allowed,
};
use cef::rc::Rc as _;
use cef::{
    Browser, Callback, CefString, Frame, ImplRequest, ImplResourceHandler,
    ImplResponse, ImplSchemeHandlerFactory, Request, ResourceHandler, ResourceReadCallback,
    ResourceSkipCallback, Response, SchemeHandlerFactory, WrapResourceHandler,
    WrapSchemeHandlerFactory, wrap_resource_handler, wrap_scheme_handler_factory,
};

use super::handle_path::{OpenOutcome, open_verified_file};
use super::{PendingProxy, PreviewGrant, PreviewKind, PreviewRegistry, WaitingRead};
use crate::output::Output;
use crate::wire::EngineMsg;

wrap_scheme_handler_factory! {
    pub struct PreviewSchemeHandlerFactory {
        grant: Arc<PreviewGrant>,
        registry: Arc<PreviewRegistry>,
        out_tx: Output,
    }

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let req = request?;
            if self.grant.is_revoked() {
                return None;
            }
            let method = CefString::from(&req.method()).to_string().to_ascii_uppercase();
            if method != "GET" && method != "HEAD" {
                return None;
            }
            let head_only = method == "HEAD";
            let url = CefString::from(&req.url()).to_string();
            // Verify the non-spoofable initiator recorded by `PreviewRequestContextHandler`.
            let initiator = self.grant.take_initiator(req.identifier());
            if initiator.is_some_and(|rec| !rec.allowed) {
                return None;
            }
            let browser_initiated = initiator.is_none_or(|rec| rec.browser_initiated);
            let range_header = {
                let raw = CefString::from(&req.header_by_name(Some(&CefString::from("Range")))).to_string();
                (!raw.trim().is_empty()).then_some(raw)
            };

            match &self.grant.kind {
                PreviewKind::Local { root, pin, document, home } => {
                    let Some((rel, _)) = parse_preview_url(&url, &self.grant.grant_host, cfg!(windows)) else {
                        self.grant.mark_denied(&self.out_tx);
                        return None;
                    };
                    let entry = rel == *document && browser_initiated && self.grant.take_entry();
                    match open_verified_file(root, pin, document, &rel, entry, home.as_deref()) {
                        OpenOutcome::Opened { file, verified_rel, size } => {
                            if self.grant.is_revoked() {
                                return None;
                            }
                            let range = parse_range(range_header.as_deref(), size);
                            let (status, status_text, content_range, advertised_len) = match &range {
                                RangeSpec::Full { length } => (200, "OK", None, *length),
                                RangeSpec::Partial { length, content_range, .. } => {
                                    // CEF's `StreamReaderURLLoader` calls `skip(start)` itself when
                                    // a `Range` header is present; keep the handle at offset 0 so
                                    // `skip()` positions it exactly once.
                                    (206, "Partial Content", Some(content_range.clone()), *length)
                                }
                                RangeSpec::Unsatisfiable { content_range } => {
                                    (416, "Range Not Satisfiable", Some(content_range.clone()), 0)
                                }
                            };
                            let (mime, charset) = mime_for_path(&verified_rel);
                            let state = LocalState {
                                file: (!head_only && status != 416).then_some(file),
                                head_only,
                                status,
                                status_text,
                                mime,
                                charset,
                                file_size: size,
                                advertised_len,
                                remaining: if head_only || status == 416 { 0 } else { size },
                                content_range,
                            };
                            Some(LocalFileResourceHandler::new(self.grant.clone(), Arc::new(Mutex::new(state))))
                        }
                        OpenOutcome::Denied => {
                            self.grant.mark_denied(&self.out_tx);
                            None
                        }
                        OpenOutcome::Missing => None,
                    }
                }
                PreviewKind::Remote { root, document, .. } => {
                    let Some((rel, _)) = parse_preview_url(&url, &self.grant.grant_host, false) else {
                        self.grant.mark_denied(&self.out_tx);
                        return None;
                    };
                    let entry = rel == *document && browser_initiated && self.grant.take_entry();
                    if !preview_allowed(root, document, &rel, entry, None) {
                        self.grant.mark_denied(&self.out_tx);
                        return None;
                    }
                    let (req_id, proxy) = self.registry.register_proxy(self.grant.tab_id, self.grant.clone(), head_only);
                    let _ = self.out_tx.send(EngineMsg::PreviewFetch {
                        req_id,
                        tab_id: self.grant.tab_id,
                        kind: self.grant.kind.tag(),
                        path: rel,
                        method,
                        range: range_header,
                    });
                    Some(ProxyResourceHandler::new(req_id, proxy, self.registry.clone(), false))
                }
                PreviewKind::Artifact { artifact_id } => {
                    let Some((_, route)) = artifact_resource(&url, &self.grant.grant_host, artifact_id) else {
                        return None;
                    };
                    let (req_id, proxy) = self.registry.register_proxy(self.grant.tab_id, self.grant.clone(), head_only);
                    let _ = self.out_tx.send(EngineMsg::PreviewFetch {
                        req_id,
                        tab_id: self.grant.tab_id,
                        kind: self.grant.kind.tag(),
                        path: route,
                        method,
                        range: None,
                    });
                    Some(ProxyResourceHandler::new(req_id, proxy, self.registry.clone(), true))
                }
            }
        }
    }
}

struct LocalState {
    file: Option<File>,
    head_only: bool,
    status: i32,
    status_text: &'static str,
    mime: &'static str,
    charset: Option<&'static str>,
    file_size: u64,
    advertised_len: u64,
    remaining: u64,
    content_range: Option<String>,
}

wrap_resource_handler! {
    struct LocalFileResourceHandler {
        grant: Arc<PreviewGrant>,
        state: Arc<Mutex<LocalState>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            if let Some(h) = handle_request {
                *h = 1;
            }
            i32::from(!self.grant.is_revoked())
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let Some(resp) = response else { return };
            let st = self.state.lock().unwrap();
            resp.set_status(st.status);
            resp.set_status_text(Some(&CefString::from(st.status_text)));
            resp.set_mime_type(Some(&CefString::from(st.mime)));
            if let Some(cs) = st.charset {
                resp.set_charset(Some(&CefString::from(cs)));
            }
            set_preview_security_headers(resp, false, Some(st.advertised_len), st.content_range.as_deref());
            if let Some(len) = response_length {
                // Report full file length on 200/206 so CEF's Range bounds check matches the file;
                // `read()` immediately returns 0 on HEAD / 416.
                *len = if st.status == 416 {
                    0
                } else if st.head_only {
                    i64::try_from(st.advertised_len).unwrap_or(0)
                } else {
                    i64::try_from(st.file_size).unwrap_or(-1)
                };
            }
        }

        fn skip(
            &self,
            bytes_to_skip: i64,
            bytes_skipped: Option<&mut i64>,
            _callback: Option<&mut ResourceSkipCallback>,
        ) -> ::std::os::raw::c_int {
            let mut st = self.state.lock().unwrap();
            if bytes_to_skip < 0 || self.grant.is_revoked() || st.status == 416 {
                if let Some(s) = bytes_skipped {
                    *s = -2;
                }
                return 0;
            }
            if st.head_only {
                if let Some(s) = bytes_skipped {
                    *s = bytes_to_skip;
                }
                return 1;
            }
            let skip = (bytes_to_skip as u64).min(st.remaining);
            if let Some(file) = st.file.as_mut()
                && file.seek(SeekFrom::Current(skip as i64)).is_err()
            {
                if let Some(s) = bytes_skipped {
                    *s = -2;
                }
                return 0;
            }
            st.remaining = st.advertised_len;
            if let Some(s) = bytes_skipped {
                *s = skip as i64;
            }
            1
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: ::std::os::raw::c_int,
            bytes_read: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(out_n) = bytes_read else { return 0 };
            if self.grant.is_revoked() || data_out.is_null() || bytes_to_read <= 0 {
                *out_n = if self.grant.is_revoked() { -2 } else { 0 };
                return 0;
            }
            let mut st = self.state.lock().unwrap();
            if st.remaining == 0 {
                st.file = None;
                *out_n = 0;
                return 0;
            }
            let want = (bytes_to_read as u64).min(st.remaining) as usize;
            let Some(file) = st.file.as_mut() else {
                *out_n = 0;
                return 0;
            };
            // SAFETY: CEF passes a writable buffer of at least `bytes_to_read` bytes.
            let slice = unsafe { std::slice::from_raw_parts_mut(data_out, want) };
            match file.read(slice) {
                Ok(0) => {
                    st.remaining = 0;
                    st.file = None;
                    *out_n = 0;
                    0
                }
                Ok(n) => {
                    st.remaining = st.remaining.saturating_sub(n as u64);
                    *out_n = n as i32;
                    1
                }
                Err(_) => {
                    st.file = None;
                    *out_n = -2;
                    0
                }
            }
        }

        fn cancel(&self) {
            self.state.lock().unwrap().file = None;
        }
    }
}

wrap_resource_handler! {
    struct ProxyResourceHandler {
        req_id: u64,
        proxy: Arc<PendingProxy>,
        registry: Arc<PreviewRegistry>,
        is_artifact: bool,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            let mut st = self.proxy.state.lock().unwrap();
            if st.canceled || self.proxy.grant.is_revoked() {
                if let Some(h) = handle_request {
                    *h = 1;
                }
                return 0;
            }
            if st.head_ready {
                if let Some(h) = handle_request {
                    *h = 1;
                }
                return i32::from(st.head_ok);
            }
            if let Some(h) = handle_request {
                *h = 0;
            }
            st.open_callback = callback.cloned();
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            let Some(resp) = response else { return };
            let st = self.proxy.state.lock().unwrap();
            let status_text = match st.status {
                200 => "OK",
                206 => "Partial Content",
                416 => "Range Not Satisfiable",
                _ => "Error",
            };
            resp.set_status(st.status as i32);
            resp.set_status_text(Some(&CefString::from(status_text)));
            resp.set_mime_type(Some(&CefString::from(st.mime.as_str())));
            if let Some(cs) = &st.charset {
                resp.set_charset(Some(&CefString::from(cs.as_str())));
            }
            set_preview_security_headers(
                resp,
                self.is_artifact,
                st.content_length,
                st.content_range.as_deref(),
            );
            if let Some(len) = response_length {
                *len = if st.status == 416 {
                    0
                } else {
                    st.content_length.and_then(|n| i64::try_from(n).ok()).unwrap_or(-1)
                };
            }
        }

        fn skip(
            &self,
            bytes_to_skip: i64,
            bytes_skipped: Option<&mut i64>,
            _callback: Option<&mut ResourceSkipCallback>,
        ) -> ::std::os::raw::c_int {
            let st = self.proxy.state.lock().unwrap();
            if bytes_to_skip < 0 || st.canceled || self.proxy.grant.is_revoked() || st.status == 416 {
                if let Some(s) = bytes_skipped {
                    *s = -2;
                }
                return 0;
            }
            // Remote `/api/fs/read/*` already served the exact requested byte slice for `206`.
            if let Some(s) = bytes_skipped {
                *s = bytes_to_skip;
            }
            1
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: ::std::os::raw::c_int,
            bytes_read: Option<&mut ::std::os::raw::c_int>,
            callback: Option<&mut ResourceReadCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(out_n) = bytes_read else { return 0 };
            if self.proxy.head_only {
                *out_n = 0;
                return 0;
            }
            if data_out.is_null() || bytes_to_read <= 0 {
                *out_n = 0;
                return 0;
            }
            let mut st = self.proxy.state.lock().unwrap();
            if st.canceled || self.proxy.grant.is_revoked() {
                *out_n = -2;
                return 0;
            }
            if let Some(mut chunk) = st.chunks.pop_front() {
                let max_len = bytes_to_read as usize;
                let n = chunk.len().min(max_len);
                // SAFETY: CEF passes a writable buffer of at least `bytes_to_read` bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(chunk.as_ptr(), data_out, n);
                }
                if n < chunk.len() {
                    st.chunks.push_front(chunk.split_off(n));
                }
                *out_n = n as i32;
                return 1;
            }
            if st.ended {
                *out_n = if st.end_ok { 0 } else { -2 };
                return 0;
            }
            let Some(cb) = callback.cloned() else {
                *out_n = -2;
                return 0;
            };
            st.waiting_read = Some(WaitingRead {
                data_out: data_out as usize,
                max_len: bytes_to_read as usize,
                callback: cb,
            });
            *out_n = 0;
            1
        }

        fn cancel(&self) {
            self.proxy.cancel();
            self.registry.remove_proxy(self.req_id);
        }
    }
}

/// Applies the full preview defense-in-depth response headers to every `oc-preview://` response
/// (documents, scripts, styles, workers, and assets alike).
fn set_preview_security_headers(
    resp: &Response,
    is_artifact: bool,
    content_length: Option<u64>,
    content_range: Option<&str>,
) {
    let set = |k: &str, v: &str| {
        resp.set_header_by_name(Some(&CefString::from(k)), Some(&CefString::from(v)), 1);
    };
    set("Content-Security-Policy", if is_artifact { ARTIFACT_CSP } else { WORKSPACE_PREVIEW_CSP });
    set("Connection-Allowlist", CONNECTION_ALLOWLIST);
    set("Cross-Origin-Resource-Policy", "same-origin");
    set("X-DNS-Prefetch-Control", "off");
    set("Referrer-Policy", "no-referrer");
    set("Cache-Control", "no-store");
    set("X-Content-Type-Options", "nosniff");
    if !is_artifact {
        set("Accept-Ranges", "bytes");
    }
    if let Some(len) = content_length {
        set("Content-Length", &len.to_string());
    }
    if let Some(cr) = content_range {
        set("Content-Range", cr);
    }
}
