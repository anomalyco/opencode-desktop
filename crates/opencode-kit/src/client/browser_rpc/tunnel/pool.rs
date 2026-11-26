//! Per-origin plain HTTP/1.1 keep-alive connection pool and response-framing forwarder for
//! [`super::proxy::TunnelProxy`] (`packages/plugin-browser/src/proxy.ts:35-360`).
//!
//! Strictly scoped to one attachment's proxy: sockets are keyed by canonicalized `host:port`,
//! never shared across origins or attachments, and evicted immediately on error, abort, reset,
//! or idle timeout.

use std::collections::{BTreeMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use rune::time::Instant;

use super::super::framing::{ParsedHead, ProxyUrl, build_forwarded_head, client_wants_close};
use super::super::http::AbortHandle;
use super::proxy::ProxyCtx;
use super::stream::UpstreamStream;
use super::super::types::{TUNNEL_CHUNK_BYTES, TunnelTarget};

const MAX_POOLED_ORIGINS: usize = 64;
const MAX_FREE_SOCKETS_PER_ORIGIN: usize = 6;
const IDLE_POOL_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESP_HEAD_BYTES: usize = 64 * 1024;
const MAX_CHUNK_LINE_BYTES: usize = 4 * 1024;

struct IdleEntry {
    stream: UpstreamStream,
    idle_since: Instant,
}

struct OriginBucket {
    last_used: u64,
    idle: Vec<IdleEntry>,
}

struct PoolInner {
    closed: bool,
    tick: u64,
    origins: BTreeMap<String, OriginBucket>,
}

/// Per-origin LIFO idle connection pool scoped to one [`super::proxy::TunnelProxy`].
pub(super) struct OriginPool {
    inner: Mutex<PoolInner>,
}

impl Default for OriginPool {
    fn default() -> Self {
        Self {
            inner: Mutex::new(PoolInner {
                closed: false,
                tick: 0,
                origins: BTreeMap::new(),
            }),
        }
    }
}

impl OriginPool {
    pub fn origin_key(target: &TunnelTarget) -> String {
        format!("{}:{}", target.host.to_ascii_lowercase(), target.port)
    }

    /// Pops the newest non-expired reusable idle stream for `origin` (LIFO), closing any stale ones.
    pub fn acquire(&self, origin: &str) -> Option<UpstreamStream> {
        let now = Instant::now();
        let mut stale = Vec::new();
        let picked = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if g.closed {
                return None;
            }
            g.tick = g.tick.saturating_add(1);
            let tick = g.tick;
            let bucket = g.origins.get_mut(origin)?;
            bucket.last_used = tick;
            let mut found = None;
            while let Some(entry) = bucket.idle.pop() {
                if now.saturating_duration_since(entry.idle_since) < IDLE_POOL_SOCKET_TIMEOUT
                    && entry.stream.is_reusable()
                {
                    found = Some(entry.stream);
                    break;
                }
                stale.push(entry.stream);
            }
            if bucket.idle.is_empty() {
                g.origins.remove(origin);
            }
            found
        };
        for s in stale {
            s.close_or_reset();
        }
        picked
    }

    /// Returns a clean idle stream to `origin`'s LIFO pool, evicting LRU origins and oldest
    /// per-origin sockets beyond caps.
    pub fn release(&self, origin: &str, stream: UpstreamStream) {
        if !stream.is_reusable() {
            stream.close_or_reset();
            return;
        }
        let mut evicted = Vec::new();
        {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if g.closed {
                drop(g);
                stream.close_or_reset();
                return;
            }
            g.tick = g.tick.saturating_add(1);
            let tick = g.tick;
            if !g.origins.contains_key(origin)
                && g.origins.len() >= MAX_POOLED_ORIGINS
                && let Some(oldest_key) = g
                    .origins
                    .iter()
                    .min_by_key(|(_, b)| b.last_used)
                    .map(|(k, _)| k.clone())
                && let Some(old_bucket) = g.origins.remove(&oldest_key)
            {
                evicted.extend(old_bucket.idle.into_iter().map(|e| e.stream));
            }
            let bucket = g.origins.entry(origin.to_owned()).or_insert_with(|| OriginBucket {
                last_used: tick,
                idle: Vec::new(),
            });
            bucket.last_used = tick;
            if bucket.idle.len() >= MAX_FREE_SOCKETS_PER_ORIGIN {
                evicted.push(bucket.idle.remove(0).stream);
            }
            bucket.idle.push(IdleEntry { stream, idle_since: Instant::now() });
        }
        for s in evicted {
            s.close_or_reset();
        }
    }

    /// Immediately evicts and closes every idle pooled socket across all origins.
    pub fn clear(&self) {
        let drained: Vec<UpstreamStream> = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut g.origins)
                .into_values()
                .flat_map(|b| b.idle.into_iter().map(|e| e.stream))
                .collect()
        };
        for s in drained {
            s.close_or_reset();
        }
    }

    pub fn close(&self) {
        let drained: Vec<UpstreamStream> = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            g.closed = true;
            std::mem::take(&mut g.origins)
                .into_values()
                .flat_map(|b| b.idle.into_iter().map(|e| e.stream))
                .collect()
        };
        for s in drained {
            s.close_or_reset();
        }
    }
}

pub(super) enum ForwardOutcome {
    /// Response completed cleanly; `keep_client_alive` says whether the local TCP connection can
    /// accept another HTTP/1.1 request.
    Done { keep_client_alive: bool },
    /// Failed before any response byte was sent to the client (caller sends 502 with message).
    BadGateway(String),
    /// Failed after response bytes started flowing (caller closes the client connection).
    MidStreamClosed,
}

/// Forwards one plain HTTP/1.1 request using per-origin keep-alive pooling.
#[allow(clippy::too_many_arguments)]
pub(super) fn forward_http_pooled(
    ctx: &ProxyCtx,
    client_id: u64,
    req: &ParsedHead,
    url: &ProxyUrl,
    target: &TunnelTarget,
    content_length: u64,
    prebuffered: &[u8],
    client: &mut TcpStream,
    io_abort: &AbortHandle,
) -> ForwardOutcome {
    let origin = OriginPool::origin_key(target);
    let head = build_forwarded_head(req, url, false, content_length, true);
    let wants_close = client_wants_close(req);

    // Try a pooled idle socket first; if an idempotent zero-body request fails before any
    // response byte is received (stale keep-alive socket), evict it and redial once.
    let mut from_pool = false;
    let mut stream = if let Some(pooled) = ctx.pool.acquire(&origin) {
        from_pool = true;
        pooled
    } else {
        match ctx.transport.open_stream(target, io_abort) {
            Ok(s) => s,
            Err(msg) => return ForwardOutcome::BadGateway(msg),
        }
    };

    if !ctx.admits() {
        stream.close_or_reset();
        return ForwardOutcome::MidStreamClosed;
    }

    let mut attempt = 0u8;
    loop {
        let send_ok = stream.write_all(head.as_bytes(), io_abort).is_ok()
            && (content_length == 0
                || pump_client_body(ctx, client_id, &stream, prebuffered, client, content_length, io_abort));
        if !send_ok {
            stream.close_or_reset();
            if from_pool && content_length == 0 && attempt == 0 && ctx.admits() && !io_abort.is_aborted() {
                attempt += 1;
                from_pool = false;
                match ctx.transport.open_stream(target, io_abort) {
                    Ok(fresh) => {
                        stream = fresh;
                        continue;
                    }
                    Err(msg) => return ForwardOutcome::BadGateway(msg),
                }
            }
            return ForwardOutcome::BadGateway("Upstream write failed.".into());
        }

        let mut reader = StreamBufReader::new(&stream, io_abort);
        let resp_head = match read_response_head(&mut reader) {
            Ok(h) => h,
            Err(msg) => {
                stream.close_or_reset();
                if from_pool && content_length == 0 && attempt == 0 && ctx.admits() && !io_abort.is_aborted() {
                    attempt += 1;
                    from_pool = false;
                    match ctx.transport.open_stream(target, io_abort) {
                        Ok(fresh) => {
                            stream = fresh;
                            continue;
                        }
                        Err(e) => return ForwardOutcome::BadGateway(e),
                    }
                }
                return ForwardOutcome::BadGateway(msg);
            }
        };

        // Forward any 1xx interim responses (e.g. 103 Early Hints) and read the final head.
        let mut current = resp_head;
        while (100..200).contains(&current.status) && current.status != 101 {
            if client.write_all(&current.raw_for_interim).is_err() {
                stream.close_or_reset();
                return ForwardOutcome::MidStreamClosed;
            }
            match read_response_head(&mut reader) {
                Ok(next) => current = next,
                Err(_) => {
                    stream.close_or_reset();
                    return ForwardOutcome::MidStreamClosed;
                }
            }
        }

        let bodyless = (100..200).contains(&current.status)
            || current.status == 204
            || current.status == 304
            || req.method.eq_ignore_ascii_case("HEAD");
        let explicit_framing = bodyless || current.chunked || current.content_length.is_some();
        let keep_client_alive = !wants_close && explicit_framing;
        let out_head = build_client_response_head(&current, keep_client_alive);
        if client.write_all(out_head.as_bytes()).is_err() {
            stream.close_or_reset();
            return ForwardOutcome::MidStreamClosed;
        }

        let body_ok = if bodyless {
            true
        } else if current.chunked {
            pump_chunked_response(&mut reader, client)
        } else if let Some(len) = current.content_length {
            pump_exact_response(&mut reader, client, len)
        } else {
            pump_eof_response(&mut reader, client)
        };

        if !body_ok {
            stream.close_or_reset();
            return ForwardOutcome::MidStreamClosed;
        }

        let reusable =
            explicit_framing && !current.upstream_close && reader.buf.is_empty() && stream.is_reusable();
        if reusable {
            ctx.pool.release(&origin, stream);
        } else {
            stream.close_or_reset();
        }
        return ForwardOutcome::Done { keep_client_alive };
    }
}

fn pump_client_body(
    ctx: &ProxyCtx,
    client_id: u64,
    stream: &UpstreamStream,
    prebuffered: &[u8],
    client: &mut TcpStream,
    content_length: u64,
    abort: &AbortHandle,
) -> bool {
    let mut remaining = content_length;
    if remaining > 0 && !prebuffered.is_empty() {
        let take = (prebuffered.len() as u64).min(remaining) as usize;
        remaining -= take as u64;
        if stream.write_all(&prebuffered[..take], abort).is_err() {
            return false;
        }
    }
    let mut buf = vec![0u8; TUNNEL_CHUNK_BYTES];
    while remaining > 0 {
        let want = (buf.len() as u64).min(remaining) as usize;
        match client.read(&mut buf[..want]) {
            Ok(0) => return false,
            Ok(n) => {
                remaining -= n as u64;
                if stream.write_all(&buf[..n], abort).is_err() {
                    return false;
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                if ctx.is_expired(client_id) || abort.is_aborted() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

struct StreamBufReader<'a> {
    stream: &'a UpstreamStream,
    abort: &'a AbortHandle,
    buf: VecDeque<u8>,
    eof: bool,
}

impl<'a> StreamBufReader<'a> {
    fn new(stream: &'a UpstreamStream, abort: &'a AbortHandle) -> Self {
        Self { stream, abort, buf: VecDeque::with_capacity(8192), eof: false }
    }

    fn fill(&mut self) -> Result<usize, String> {
        if self.eof {
            return Ok(0);
        }
        let mut tmp = [0u8; 32 * 1024];
        let n = self.stream.read(&mut tmp, self.abort)?;
        if n == 0 {
            self.eof = true;
        } else {
            self.buf.extend(&tmp[..n]);
        }
        Ok(n)
    }

    fn read_line_crlf(&mut self, max_bytes: usize) -> Result<Vec<u8>, String> {
        let mut line = Vec::with_capacity(64);
        loop {
            while let Some(b) = self.buf.pop_front() {
                line.push(b);
                if line.ends_with(b"\r\n") {
                    return Ok(line);
                }
                if line.len() > max_bytes {
                    return Err("Response line exceeded limit.".into());
                }
            }
            if self.fill()? == 0 {
                return Err("Upstream closed before line completed.".into());
            }
        }
    }

    fn read_exact_to(&mut self, mut remaining: u64, client: &mut TcpStream) -> bool {
        while remaining > 0 {
            if self.buf.is_empty() {
                match self.fill() {
                    Ok(0) | Err(_) => return false,
                    Ok(_) => {}
                }
            }
            let (head, _) = self.buf.as_slices();
            let take = (head.len() as u64).min(remaining) as usize;
            if take == 0 {
                self.buf.make_contiguous();
                continue;
            }
            if client.write_all(&head[..take]).is_err() {
                return false;
            }
            self.buf.drain(..take);
            remaining -= take as u64;
        }
        true
    }
}

struct ParsedRespHead {
    status_line: String,
    status: u16,
    headers: Vec<(String, String)>,
    chunked: bool,
    content_length: Option<u64>,
    upstream_close: bool,
    raw_for_interim: Vec<u8>,
}

fn read_response_head(reader: &mut StreamBufReader<'_>) -> Result<ParsedRespHead, String> {
    let mut raw = Vec::with_capacity(1024);
    loop {
        while let Some(b) = reader.buf.pop_front() {
            raw.push(b);
            if raw.ends_with(b"\r\n\r\n") {
                return parse_resp_head(raw);
            }
            if raw.len() > MAX_RESP_HEAD_BYTES {
                return Err("Upstream response headers exceeded 64 KiB.".into());
            }
        }
        if reader.fill()? == 0 {
            return Err("Upstream closed before sending HTTP response headers.".into());
        }
    }
}

fn parse_resp_head(raw: Vec<u8>) -> Result<ParsedRespHead, String> {
    let head_str = std::str::from_utf8(&raw[..raw.len() - 4])
        .map_err(|_| "Upstream HTTP response headers were not valid UTF-8.".to_owned())?;
    let mut lines = head_str.split("\r\n");
    let status_line = lines
        .next()
        .filter(|l| !l.is_empty())
        .ok_or_else(|| "Empty HTTP status line from upstream.".to_owned())?
        .to_owned();
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or("");
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("Malformed HTTP status line: {status_line}"))?;
    let mut headers = Vec::new();
    let mut chunked = false;
    let mut content_length = None;
    let mut conn_close = false;
    let mut conn_keep_alive = false;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            if key.eq_ignore_ascii_case("transfer-encoding")
                && val.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            } else if key.eq_ignore_ascii_case("content-length")
                && content_length.is_none()
                && let Ok(n) = val.parse::<u64>()
            {
                content_length = Some(n);
            } else if key.eq_ignore_ascii_case("connection") {
                for tok in val.split(',').map(|t| t.trim().to_ascii_lowercase()) {
                    if tok == "close" {
                        conn_close = true;
                    } else if tok == "keep-alive" {
                        conn_keep_alive = true;
                    }
                }
            }
            headers.push((key.to_owned(), val.to_owned()));
        }
    }
    if chunked {
        content_length = None;
    }
    let upstream_close = conn_close || (version == "HTTP/1.0" && !conn_keep_alive);
    Ok(ParsedRespHead {
        status_line,
        status,
        headers,
        chunked,
        content_length,
        upstream_close,
        raw_for_interim: raw,
    })
}

fn build_client_response_head(resp: &ParsedRespHead, keep_client_alive: bool) -> String {
    let mut hop_names: Vec<String> = resp
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| v.split(',').map(|s| s.trim().to_ascii_lowercase()))
        .filter(|s| !s.is_empty())
        .collect();
    hop_names.extend(
        [
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "proxy-connection",
            "te",
            "trailer",
            "upgrade",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let mut out = format!("{}\r\n", resp.status_line);
    for (k, v) in &resp.headers {
        let lower = k.to_ascii_lowercase();
        if hop_names.iter().any(|h| h == &lower) {
            continue;
        }
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    if keep_client_alive {
        out.push_str("Connection: keep-alive\r\n\r\n");
    } else {
        out.push_str("Connection: close\r\n\r\n");
    }
    out
}

fn pump_exact_response(reader: &mut StreamBufReader<'_>, client: &mut TcpStream, len: u64) -> bool {
    reader.read_exact_to(len, client)
}

fn pump_chunked_response(reader: &mut StreamBufReader<'_>, client: &mut TcpStream) -> bool {
    loop {
        let Ok(size_line) = reader.read_line_crlf(MAX_CHUNK_LINE_BYTES) else {
            return false;
        };
        let Ok(text) = std::str::from_utf8(&size_line[..size_line.len() - 2]) else {
            return false;
        };
        let hex = text.split(';').next().unwrap_or("").trim();
        let Ok(chunk_len) = u64::from_str_radix(hex, 16) else {
            return false;
        };
        if client.write_all(&size_line).is_err() {
            return false;
        }
        if chunk_len == 0 {
            loop {
                let Ok(trailer) = reader.read_line_crlf(MAX_CHUNK_LINE_BYTES) else {
                    return false;
                };
                if client.write_all(&trailer).is_err() {
                    return false;
                }
                if trailer == b"\r\n" {
                    return true;
                }
            }
        }
        if !reader.read_exact_to(chunk_len + 2, client) {
            return false;
        }
    }
}

fn pump_eof_response(reader: &mut StreamBufReader<'_>, client: &mut TcpStream) -> bool {
    loop {
        if !reader.buf.is_empty() {
            reader.buf.make_contiguous();
            let (head, _) = reader.buf.as_slices();
            if client.write_all(head).is_err() {
                return false;
            }
            reader.buf.clear();
        }
        match reader.fill() {
            Ok(0) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}
