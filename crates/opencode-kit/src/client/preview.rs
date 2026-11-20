//! Authenticated, redirect-forbidding, abortable streaming reader for remote workspace
//! (`/api/fs/read/*`) and artifact (`/api/experimental/artifacts/<id>/view/*`) browser previews
//! (`#63002`: `remote-preview.ts`, `artifact-preview.ts`).
//!
//! Security & resource-control invariants (`notes/review-rd281.md` PR 2 Blocker #3):
//! - Uses a dedicated per-request `ureq::Agent` (`max_redirects(0)`, `max_idle_connections(0)`,
//!   `http_status_as_error(false)`, loopback proxy bypass, connect 10 s, header 15 s, body read
//!   30 s, global 120 s) registered in the tab's [`PreviewAbort`] handle so closing a tab or
//!   signing out shuts down every blocked socket immediately via `ureq::Agent::shutdown`.
//! - Sends `Accept-Encoding: identity` and ONLY `self.auth` (owning session's server credentials).
//! - Streams in 64 KiB chunks, caps bytes at `min(max_bytes, content_length)`, and requires exact
//!   `Content-Length` completion when declared.

use std::io::Read as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::{Server, http};

/// Header metadata observed from a preview `HEAD` or `GET` request.
#[derive(Clone, Debug)]
pub struct PreviewHttpHead {
    pub status: u16,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub content_range: Option<String>,
}

const PREVIEW_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PREVIEW_HEADER_TIMEOUT: Duration = Duration::from_secs(15);
const PREVIEW_BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);
const PREVIEW_GLOBAL_TIMEOUT: Duration = Duration::from_secs(120);
const PREVIEW_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct AbortState {
    aborted: bool,
    agents: Vec<(u64, ureq::Agent)>,
}

/// Shared cancellation handle owned by a preview tab binding: calling [`PreviewAbort::abort`]
/// marks the tab aborted and immediately shuts down every in-flight HTTP socket via
/// `ureq::Agent::shutdown`, unblocking stalled reads on tab close, navigation, or sign-out.
#[derive(Clone, Default)]
pub struct PreviewAbort {
    inner: Arc<Mutex<AbortState>>,
}

static NEXT_ABORT_TOKEN: AtomicU64 = AtomicU64::new(1);

impl PreviewAbort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_aborted(&self) -> bool {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).aborted
    }

    pub fn abort(&self) {
        let agents: Vec<ureq::Agent> = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            g.aborted = true;
            g.agents.drain(..).map(|(_, a)| a).collect()
        };
        for agent in agents {
            agent.shutdown();
        }
    }

    fn register(&self, agent: &ureq::Agent) -> Option<AgentLease> {
        let token = NEXT_ABORT_TOKEN.fetch_add(1, Ordering::Relaxed);
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if g.aborted {
            drop(g);
            agent.clone().shutdown();
            return None;
        }
        g.agents.push((token, agent.clone()));
        Some(AgentLease { inner: Arc::clone(&self.inner), token })
    }
}

struct AgentLease {
    inner: Arc<Mutex<AbortState>>,
    token: u64,
}

impl Drop for AgentLease {
    fn drop(&mut self) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(pos) = g.agents.iter().position(|(t, _)| *t == self.token) {
            let (_, agent) = g.agents.swap_remove(pos);
            drop(g);
            agent.shutdown();
        }
    }
}

impl Server {
    fn preview_agent(&self) -> ureq::Agent {
        let cfg = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_idle_connections(0)
            .timeout_connect(Some(PREVIEW_CONNECT_TIMEOUT))
            .timeout_recv_response(Some(PREVIEW_HEADER_TIMEOUT))
            .timeout_recv_body(Some(PREVIEW_BODY_READ_TIMEOUT))
            .timeout_global(Some(PREVIEW_GLOBAL_TIMEOUT));
        if http::loopback(&self.base) {
            cfg.proxy(None).build().into()
        } else {
            cfg.build().into()
        }
    }

    /// Streams a `GET` or `HEAD` preview response from `path` with optional `Range` header,
    /// calling `on_head` first; if `on_head` returns `true` and `method == "GET"`, streams 64 KiB
    /// chunks into `on_chunk` up to `min(max_bytes, content_length)`. Immediately aborts on
    /// `abort.abort()` or body read/global timeout. Returns `true` only on clean completion.
    pub fn preview_stream(
        &self,
        method: &str,
        path: &str,
        range: Option<&str>,
        max_bytes: u64,
        abort: &PreviewAbort,
        on_head: impl FnOnce(&PreviewHttpHead) -> bool,
        mut on_chunk: impl FnMut(&[u8]) -> bool,
    ) -> bool {
        if abort.is_aborted() {
            return false;
        }
        let agent = self.preview_agent();
        let Some(_lease) = abort.register(&agent) else {
            return false;
        };
        let url = format!("{}{path}", self.base);
        let is_head = method.eq_ignore_ascii_case("HEAD");
        let mut resp = if is_head {
            let mut req = agent.head(&url).header("Accept-Encoding", "identity");
            if let Some(auth) = &self.auth {
                req = req.header("Authorization", auth);
            }
            if let Some(r) = range {
                req = req.header("Range", r);
            }
            let Ok(r) = req.call() else {
                return false;
            };
            r
        } else {
            let mut req = agent.get(&url).header("Accept-Encoding", "identity");
            if let Some(auth) = &self.auth {
                req = req.header("Authorization", auth);
            }
            if let Some(r) = range {
                req = req.header("Range", r);
            }
            let Ok(r) = req.call() else {
                return false;
            };
            r
        };

        if abort.is_aborted() {
            return false;
        }
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
            if abort.is_aborted() {
                return false;
            }
            match reader.read(&mut buf) {
                Ok(0) => return !abort.is_aborted() && head.content_length.is_none_or(|cl| total == cl),
                Ok(n) => {
                    total = total.saturating_add(n as u64);
                    if abort.is_aborted() || total > stream_cap || !on_chunk(&buf[..n]) {
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
