//! `CefSchemeHandlerFactory` and async off-IO-thread `CefResourceHandler` implementations for
//! `oc-preview://`:
//! - `LocalFileResourceHandler`: performs ALL filesystem open/verify/seek/read syscalls off CEF's
//!   global IO thread on `PreviewIoPool`, verifies local files via pre-open intermediate-component
//!   walk + open-handle final path (`handle_path::open_verified_file`), and streams strictly from
//!   that verified `File` handle with `HEAD` + `Range` (`200`/`206`/`416`) + 8 security headers.
//! - `ProxyResourceHandler`: validates remote workspace & artifact URLs/scopes/methods/initiators,
//!   per-segment percent-encodes remote paths and validates single-range syntax before asking the
//!   desktop host over the private Unix socket (`EngineMsg::PreviewFetch`), derives remote MIME
//!   strictly from extension, and streams asynchronously with backpressure and disconnect cleanup.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::sync::{Arc, Mutex};

use browser_policy::preview::{
    ARTIFACT_CSP, CONNECTION_ALLOWLIST, RangeSpec, WORKSPACE_PREVIEW_CSP, artifact_resource,
    encode_rel_path, mime_for_path, normalize_range_header, parse_preview_url, parse_range,
    preview_allowed, root_is_windows,
};
use cef::rc::Rc as _;
use cef::{
    Browser, Callback, CefString, Frame, ImplCallback, ImplRequest, ImplRequestContext,
    ImplResourceHandler, ImplResourceReadCallback, ImplResourceSkipCallback, ImplResponse,
    ImplSchemeHandlerFactory, Request, RequestContext, ResourceHandler, ResourceReadCallback,
    ResourceSkipCallback, Response, SchemeHandlerFactory, WrapResourceHandler,
    WrapSchemeHandlerFactory, wrap_resource_handler, wrap_scheme_handler_factory,
};

use super::handle_path::{OpenOutcome, RootPin, open_verified_file};
use super::{PendingProxy, PreviewGrant, PreviewKind, PreviewRegistry, WaitingRead};
use crate::output::Output;
use crate::wire::EngineMsg;

const LOCAL_READ_CHUNK_MAX: usize = 64 * 1024;

// Fail-closed `CefSchemeHandlerFactory` registered for `"file"` on the global `Default`
// `CefRequestContext` and every isolated context so raw `file://` requests are blocked engine-wide
// across main frames, sub-frames, subresources, `fetch`, and Web Workers (`notes/review-rd281.md`).
wrap_scheme_handler_factory! {
    struct FileSchemeBanFactory;

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            _request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            Some(FileSchemeDenyHandler::new())
        }
    }
}

wrap_resource_handler! {
    struct FileSchemeDenyHandler;

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
            0
        }

        fn cancel(&self) {}
    }
}

/// Registers a fail-closed `"file"` scheme handler on `ctx` blocking all native `file://` loads.
pub fn ban_file_scheme(ctx: &RequestContext) {
    let mut factory = FileSchemeBanFactory::new();
    let ok = ctx.register_scheme_handler_factory(
        Some(&CefString::from("file")),
        Some(&CefString::from("")),
        Some(&mut factory),
    );
    if ok == 0 {
        eprintln!("browser-engine: register_scheme_handler_factory(file) returned 0");
    }
}

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
            if !self.grant.is_ready() {
                return None;
            }
            let req_id = req.identifier();
            if req_id == 0 {
                self.grant.mark_denied(&self.out_tx);
                return None;
            }
            let method = CefString::from(&req.method()).to_string().to_ascii_uppercase();
            if method != "GET" && method != "HEAD" {
                return None;
            }
            let head_only = method == "HEAD";
            let url = CefString::from(&req.url()).to_string();
            // Fail closed: every `oc-preview://` request MUST have been validated and recorded by
            // `TabRequestHandler` or `ContextPool`'s `IsolatedContextHandler` (`frame == None` workers).
            let Some(rec) = self.grant.take_initiator(req_id) else {
                self.grant.mark_denied(&self.out_tx);
                return None;
            };
            if !rec.allowed {
                self.grant.mark_denied(&self.out_tx);
                return None;
            }
            let browser_initiated = rec.browser_initiated;
            let raw_range = {
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
                    // Fast in-memory lexical gate on the IO thread; all disk syscalls run in `open()`
                    // on `PreviewIoPool` off CEF's IO thread.
                    if !preview_allowed(root, document, &rel, entry, home.as_deref()) {
                        self.grant.mark_denied(&self.out_tx);
                        return None;
                    }
                    let req_spec = LocalOpenSpec {
                        root: root.clone(),
                        pin: pin.clone(),
                        document: document.clone(),
                        rel,
                        entry,
                        home: home.clone(),
                        head_only,
                        range_header: raw_range,
                    };
                    Some(LocalFileResourceHandler::new(
                        self.grant.clone(),
                        self.registry.clone(),
                        self.out_tx.clone(),
                        Arc::new(Mutex::new(LocalState::pending(req_spec))),
                    ))
                }
                PreviewKind::Remote { root, document, .. } => {
                    let Some((rel, _)) =
                        parse_preview_url(&url, &self.grant.grant_host, root_is_windows(root))
                    else {
                        self.grant.mark_denied(&self.out_tx);
                        return None;
                    };
                    let entry = rel == *document && browser_initiated && self.grant.take_entry();
                    if !preview_allowed(root, document, &rel, entry, None) {
                        self.grant.mark_denied(&self.out_tx);
                        return None;
                    }
                    // Per-segment percent-encode `rel` and validate single-range syntax before host fetch.
                    let encoded_rel = encode_rel_path(&rel);
                    let safe_range = normalize_range_header(raw_range.as_deref());
                    let Some((req_id, proxy)) = self.registry.register_proxy(
                        self.grant.tab_id,
                        self.grant.clone(),
                        head_only,
                        Some(rel),
                        safe_range.clone(),
                    ) else {
                        return None;
                    };
                    let _ = self.out_tx.send(EngineMsg::PreviewFetch {
                        req_id,
                        tab_id: self.grant.tab_id,
                        kind: self.grant.kind.tag(),
                        path: encoded_rel,
                        method,
                        range: safe_range,
                    });
                    Some(ProxyResourceHandler::new(req_id, proxy, self.registry.clone(), false))
                }
                PreviewKind::Artifact { artifact_id } => {
                    let Some((_, route)) = artifact_resource(&url, &self.grant.grant_host, artifact_id) else {
                        return None;
                    };
                    let Some((req_id, proxy)) = self.registry.register_proxy(
                        self.grant.tab_id,
                        self.grant.clone(),
                        head_only,
                        None,
                        None,
                    ) else {
                        return None;
                    };
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

struct LocalOpenSpec {
    root: String,
    pin: RootPin,
    document: String,
    rel: String,
    entry: bool,
    home: Option<String>,
    head_only: bool,
    range_header: Option<String>,
}

struct LocalState {
    canceled: bool,
    open_spec: Option<LocalOpenSpec>,
    file: Option<File>,
    head_only: bool,
    status: i32,
    status_text: &'static str,
    mime: &'static str,
    charset: Option<&'static str>,
    file_size: u64,
    offset: u64,
    advertised_len: u64,
    remaining: u64,
    content_range: Option<String>,
}

impl LocalState {
    fn pending(spec: LocalOpenSpec) -> Self {
        let head_only = spec.head_only;
        Self {
            canceled: false,
            open_spec: Some(spec),
            file: None,
            head_only,
            status: 404,
            status_text: "Not Found",
            mime: "application/octet-stream",
            charset: None,
            file_size: 0,
            offset: 0,
            advertised_len: 0,
            remaining: 0,
            content_range: None,
        }
    }
}

wrap_resource_handler! {
    struct LocalFileResourceHandler {
        grant: Arc<PreviewGrant>,
        registry: Arc<PreviewRegistry>,
        out_tx: Output,
        state: Arc<Mutex<LocalState>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            if self.grant.is_revoked() {
                if let Some(h) = handle_request {
                    *h = 1;
                }
                return 0;
            }
            let spec = {
                let mut st = self.state.lock().unwrap();
                if st.canceled {
                    None
                } else {
                    st.open_spec.take()
                }
            };
            let (Some(spec), Some(cb)) = (spec, callback.cloned()) else {
                if let Some(h) = handle_request {
                    *h = 1;
                }
                return 0;
            };
            let state = self.state.clone();
            let grant = self.grant.clone();
            let out_tx = self.out_tx.clone();
            let cb_panic = cb.clone();
            let state_panic = self.state.clone();
            let spawned = self.registry.io().try_spawn_for(
                &self.grant,
                move || {
                    let mut st = state_panic.lock().unwrap_or_else(|e| e.into_inner());
                    st.canceled = true;
                    st.file = None;
                    drop(st);
                    cb_panic.cancel();
                },
                move || {
                    if grant.is_revoked() || state.lock().unwrap().canceled {
                        cb.cancel();
                        return;
                    }
                    match open_verified_file(
                        &spec.root,
                        &spec.pin,
                        &spec.document,
                        &spec.rel,
                        spec.entry,
                        spec.home.as_deref(),
                    ) {
                        OpenOutcome::Opened { file, verified_rel, size } => {
                            let range = parse_range(spec.range_header.as_deref(), size);
                            let (status, status_text, content_range, advertised_len) = match &range {
                                RangeSpec::Full { length } => (200, "OK", None, *length),
                                RangeSpec::Partial { length, content_range, .. } => {
                                    (206, "Partial Content", Some(content_range.clone()), *length)
                                }
                                RangeSpec::Unsatisfiable { content_range } => {
                                    (416, "Range Not Satisfiable", Some(content_range.clone()), 0)
                                }
                            };
                            let (mime, charset) = mime_for_path(&verified_rel);
                            let mut st = state.lock().unwrap();
                            if st.canceled || grant.is_revoked() {
                                drop(st);
                                cb.cancel();
                                return;
                            }
                            st.file = (!spec.head_only && status != 416).then_some(file);
                            st.status = status;
                            st.status_text = status_text;
                            st.mime = mime;
                            st.charset = charset;
                            st.file_size = size;
                            st.offset = 0;
                            st.advertised_len = advertised_len;
                            st.remaining = if spec.head_only || status == 416 { 0 } else { advertised_len };
                            st.content_range = content_range;
                            drop(st);
                            cb.cont();
                        }
                        OpenOutcome::Denied => {
                            grant.mark_denied(&out_tx);
                            cb.cancel();
                        }
                        OpenOutcome::Missing => {
                            cb.cancel();
                        }
                    }
                },
            );
            if !spawned {
                if let Some(h) = handle_request {
                    *h = 1;
                }
                return 0;
            }
            if let Some(h) = handle_request {
                *h = 0;
            }
            1
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
            callback: Option<&mut ResourceSkipCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(out_s) = bytes_skipped else { return 0 };
            let (file, next_offset, skip) = {
                let mut st = self.state.lock().unwrap();
                if bytes_to_skip < 0 || st.canceled || self.grant.is_revoked() || st.status == 416 {
                    *out_s = -2;
                    return 0;
                }
                if st.head_only || bytes_to_skip == 0 {
                    *out_s = bytes_to_skip;
                    return 1;
                }
                let skip = bytes_to_skip as u64;
                let Some(next_offset) = st.offset.checked_add(skip) else {
                    *out_s = -2;
                    return 0;
                };
                if next_offset > st.file_size {
                    *out_s = -2;
                    return 0;
                }
                let Some(file) = st.file.take() else {
                    *out_s = -2;
                    return 0;
                };
                (file, next_offset, skip)
            };
            let Some(cb) = callback.cloned() else {
                *out_s = -2;
                return 0;
            };
            let state = self.state.clone();
            let grant = self.grant.clone();
            let cb_panic = cb.clone();
            let state_panic = self.state.clone();
            let spawned = self.registry.io().try_spawn_for(
                &self.grant,
                move || {
                    let mut st = state_panic.lock().unwrap_or_else(|e| e.into_inner());
                    st.canceled = true;
                    st.file = None;
                    drop(st);
                    cb_panic.cont(-2);
                },
                move || {
                    let mut file = file;
                    let seek_ok = file.seek(SeekFrom::Start(next_offset)).is_ok();
                    let mut st = state.lock().unwrap();
                    if !seek_ok || st.canceled || grant.is_revoked() {
                        drop(st);
                        cb.cont(-2);
                        return;
                    }
                    st.file = Some(file);
                    st.offset = next_offset;
                    st.remaining = st.advertised_len.min(st.file_size.saturating_sub(next_offset));
                    drop(st);
                    cb.cont(skip as i64);
                },
            );
            if !spawned {
                *out_s = -2;
                return 0;
            }
            *out_s = 0;
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
            if self.grant.is_revoked() || data_out.is_null() || bytes_to_read <= 0 {
                *out_n = if self.grant.is_revoked() { -2 } else { 0 };
                return 0;
            }
            let (file, want) = {
                let mut st = self.state.lock().unwrap();
                if st.canceled {
                    *out_n = -2;
                    return 0;
                }
                if st.head_only || st.status == 416 || st.remaining == 0 {
                    st.file = None;
                    *out_n = 0;
                    return 0;
                }
                let want = (bytes_to_read as u64)
                    .min(st.remaining)
                    .min(LOCAL_READ_CHUNK_MAX as u64) as usize;
                let Some(file) = st.file.take() else {
                    *out_n = 0;
                    return 0;
                };
                (file, want)
            };
            let Some(cb) = callback.cloned() else {
                *out_n = -2;
                return 0;
            };
            let data_out_addr = data_out as usize;
            let state = self.state.clone();
            let grant = self.grant.clone();
            let cb_panic = cb.clone();
            let state_panic = self.state.clone();
            let spawned = self.registry.io().try_spawn_for(
                &self.grant,
                move || {
                    let mut st = state_panic.lock().unwrap_or_else(|e| e.into_inner());
                    st.canceled = true;
                    st.file = None;
                    drop(st);
                    cb_panic.cont(-2);
                },
                move || {
                    let mut file = file;
                    let mut buf = vec![0u8; want];
                    let res = file.read(&mut buf);
                    // Hold `state` mutex across the `data_out` copy so `cancel()` cannot return to
                    // CEF until any in-flight write finishes (zero UAF risk).
                    let mut st = state.lock().unwrap();
                    if st.canceled || grant.is_revoked() {
                        drop(st);
                        cb.cont(-2);
                        return;
                    }
                    match res {
                        Ok(0) => {
                            st.remaining = 0;
                            st.file = None;
                            drop(st);
                            cb.cont(0);
                        }
                        Ok(n) => {
                            // SAFETY: `!st.canceled` under mutex guarantees CEF's `data_out` buffer
                            // of at least `bytes_to_read >= want >= n` bytes is still valid.
                            unsafe {
                                std::ptr::copy_nonoverlapping(buf.as_ptr(), data_out_addr as *mut u8, n);
                            }
                            st.offset = st.offset.saturating_add(n as u64);
                            st.remaining = st.remaining.saturating_sub(n as u64);
                            if st.remaining > 0 {
                                st.file = Some(file);
                            }
                            drop(st);
                            cb.cont(n as i32);
                        }
                        Err(_) => {
                            st.file = None;
                            drop(st);
                            cb.cont(-2);
                        }
                    }
                },
            );
            if !spawned {
                *out_n = -2;
                return 0;
            }
            *out_n = 0;
            1
        }

        fn cancel(&self) {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            st.canceled = true;
            st.open_spec = None;
            st.file = None;
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
                drop(st);
                self.registry.remove_proxy(self.req_id);
                if let Some(h) = handle_request {
                    *h = 1;
                }
                return 0;
            }
            if st.head_ready {
                let ok = st.head_ok;
                drop(st);
                if !ok {
                    self.registry.remove_proxy(self.req_id);
                }
                if let Some(h) = handle_request {
                    *h = 1;
                }
                return i32::from(ok);
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
                // On 206 Partial Content, report the total resource size (`total_size` parsed from
                // `Content-Range`) so CEF's `StreamReaderURLLoader` bounds check passes even when
                // `start >= slice_len`.
                *len = if st.status == 416 {
                    0
                } else if st.status == 206 {
                    st.total_size.and_then(|n| i64::try_from(n).ok()).unwrap_or(-1)
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
            let mut st = self.proxy.state.lock().unwrap();
            if bytes_to_skip < 0 || st.canceled || self.proxy.grant.is_revoked() || st.status == 416 {
                if let Some(s) = bytes_skipped {
                    *s = -2;
                }
                return 0;
            }
            if bytes_to_skip == 0 {
                if let Some(s) = bytes_skipped {
                    *s = 0;
                }
                return 1;
            }
            let want = bytes_to_skip as u64;
            if st.status == 206 {
                // For `206 Partial Content`, accept ONLY the exact verified `Content-Range` start
                // offset once (the server already sliced at `range_start`); refuse any other skip.
                if !st.skipped_initial && st.range_start == Some(want) {
                    st.skipped_initial = true;
                    if let Some(s) = bytes_skipped {
                        *s = bytes_to_skip;
                    }
                    return 1;
                }
                if let Some(s) = bytes_skipped {
                    *s = -2;
                }
                return 0;
            }
            if st.status == 200 {
                // On a `200 OK` response, actually discard `want` bytes from buffered & incoming
                // chunks so CEF never receives offset-0 bytes when skipping to an offset.
                let max_bytes = self.proxy.grant.kind.max_bytes();
                let next_skip = st.skip_remaining.saturating_add(want);
                if next_skip > max_bytes {
                    if let Some(s) = bytes_skipped {
                        *s = -2;
                    }
                    return 0;
                }
                let mut left = want;
                while left > 0 {
                    let Some(mut front) = st.chunks.pop_front() else { break };
                    let drop_n = (front.len() as u64).min(left) as usize;
                    st.queued_bytes = st.queued_bytes.saturating_sub(drop_n);
                    left -= drop_n as u64;
                    if drop_n < front.len() {
                        st.chunks.push_front(front.split_off(drop_n));
                    }
                }
                st.skip_remaining = st.skip_remaining.saturating_add(left);
                if let Some(s) = bytes_skipped {
                    *s = bytes_to_skip;
                }
                return 1;
            }
            if let Some(s) = bytes_skipped {
                *s = -2;
            }
            0
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
            if st.status == 416 {
                *out_n = 0;
                return 0;
            }
            if let Some(mut chunk) = st.chunks.pop_front() {
                let max_len = bytes_to_read as usize;
                let n = chunk.len().min(max_len);
                // SAFETY: CEF passes a writable buffer of at least `bytes_to_read` bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(chunk.as_ptr(), data_out, n);
                }
                st.queued_bytes = st.queued_bytes.saturating_sub(n);
                if n < chunk.len() {
                    st.chunks.push_front(chunk.split_off(n));
                }
                *out_n = n as i32;
                return 1;
            }
            if st.ended {
                *out_n = if st.end_ok && st.skip_remaining == 0 { 0 } else { -2 };
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
