//! Authenticated, redirect-forbidding streaming reader for remote workspace (`/api/fs/read/*`)
//! and artifact (`/api/experimental/artifacts/<id>/view/*`) browser previews (`#63002`:
//! `remote-preview.ts`, `artifact-preview.ts`).
//!
//! Security invariants:
//! - Uses ONLY `self.agent` (which sets `max_redirects(0)` and bypasses proxies on loopback) and
//!   `self.auth` (the owning session's credentials — never any header supplied by the page).
//! - Sends `Accept-Encoding: identity` so byte sizes and `Range` offsets match verbatim.
//! - Streams in 64 KiB chunks and aborts immediately if the body exceeds `max_bytes` or the
//!   caller's `on_chunk` callback returns `false` (tab closed / grant revoked).

use std::io::Read as _;
use std::time::Duration;

use super::Server;

/// Header metadata observed from a preview `HEAD` or `GET` request.
#[derive(Clone, Debug)]
pub struct PreviewHttpHead {
    pub status: u16,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub content_range: Option<String>,
}

const PREVIEW_HEADER_TIMEOUT: Duration = Duration::from_secs(15);
const PREVIEW_CHUNK_BYTES: usize = 64 * 1024;

impl Server {
    /// Issues a `HEAD` probe against `path` using this server's credentials with `max_redirects(0)`
    /// and `Accept-Encoding: identity`. Returns `None` on transport failure.
    pub fn preview_head(&self, path: &str) -> Option<PreviewHttpHead> {
        let url = format!("{}{path}", self.base);
        let mut req = self.agent.head(&url).header("Accept-Encoding", "identity");
        if let Some(auth) = &self.auth {
            req = req.header("Authorization", auth);
        }
        let resp = req
            .config()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_recv_response(Some(PREVIEW_HEADER_TIMEOUT))
            .build()
            .call()
            .ok()?;
        Some(extract_head(&resp))
    }

    /// Streams a `GET` or `HEAD` preview response from `path` with optional `Range` header,
    /// calling `on_head` first; if `on_head` returns `true` and `method == "GET"`, streams 64 KiB
    /// chunks into `on_chunk` up to `min(max_bytes, content_length)`. Returns `true` only on clean
    /// completion (including exact `Content-Length` match when declared).
    pub fn preview_stream(
        &self,
        method: &str,
        path: &str,
        range: Option<&str>,
        max_bytes: u64,
        on_head: impl FnOnce(&PreviewHttpHead) -> bool,
        mut on_chunk: impl FnMut(&[u8]) -> bool,
    ) -> bool {
        let url = format!("{}{path}", self.base);
        let is_head = method.eq_ignore_ascii_case("HEAD");
        let mut resp = if is_head {
            let mut req = self.agent.head(&url).header("Accept-Encoding", "identity");
            if let Some(auth) = &self.auth {
                req = req.header("Authorization", auth);
            }
            if let Some(r) = range {
                req = req.header("Range", r);
            }
            let Ok(r) = req
                .config()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_recv_response(Some(PREVIEW_HEADER_TIMEOUT))
                .build()
                .call()
            else {
                return false;
            };
            r
        } else {
            let mut req = self.agent.get(&url).header("Accept-Encoding", "identity");
            if let Some(auth) = &self.auth {
                req = req.header("Authorization", auth);
            }
            if let Some(r) = range {
                req = req.header("Range", r);
            }
            let Ok(r) = req
                .config()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_recv_response(Some(PREVIEW_HEADER_TIMEOUT))
                .build()
                .call()
            else {
                return false;
            };
            r
        };

        let head = extract_head(&resp);
        if !on_head(&head) {
            return false;
        }
        if is_head {
            return true;
        }

        let stream_cap = head.content_length.map_or(max_bytes, |cl| cl.min(max_bytes));
        let mut reader = resp.body_mut().as_reader();
        let mut buf = vec![0u8; PREVIEW_CHUNK_BYTES];
        let mut total = 0u64;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => return head.content_length.is_none_or(|cl| total == cl),
                Ok(n) => {
                    total = total.saturating_add(n as u64);
                    if total > stream_cap || !on_chunk(&buf[..n]) {
                        return false;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
    }
}

fn extract_head(resp: &ureq::http::Response<ureq::Body>) -> PreviewHttpHead {
    let hdr = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let status = resp.status().as_u16();
    let content_type = hdr("content-type").unwrap_or_default();
    let content_length = hdr("content-length")
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u64>().ok());
    let content_range = hdr("content-range");
    PreviewHttpHead { status, content_type, content_length, content_range }
}
