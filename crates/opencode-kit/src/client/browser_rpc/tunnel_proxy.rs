//! Authenticated local loopback HTTP/CONNECT/WebSocket proxy forwarding remote-server browser
//! traffic through `experimental.browser/tunnel.{open,read,write,close}`
//! (`packages/plugin-browser/src/proxy.ts`, `packages/gui-plugins/src/browser/network.ts`).
//!
//! Security properties:
//! - Binds strictly to `127.0.0.1:0` (ephemeral loopback port, never `0.0.0.0`).
//! - Requires per-attachment 384-bit random Basic credentials (`16 B` username + `32 B` password)
//!   compared in constant time; returns `407 Proxy Authentication Required` otherwise.
//! - Strips `Proxy-Authorization` and hop-by-hop headers before forwarding to the remote server.
//! - Rejects embedded URL credentials and non-HTTP(S)/WS(S) schemes.
//! - Never dials the user's local network directly: every connection routes exclusively through
//!   the remote server's `tunnel.open` RPC.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use base64::Engine as _;

use super::http::AbortHandle;
use super::proxy_machine::{ProxyEffect, ProxyEvent, ProxyState, ReadEvent, ReadState};
use super::types::{LocationRef, TUNNEL_CHUNK_BYTES, TunnelTarget, random_hex};
use crate::client::Server;

pub const PROXY_REALM: &str = "OpenCode Browser Proxy";
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const POST_EOF_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct ProxyCredentials {
    pub username: String,
    pub password: String,
}

struct SharedProxy {
    state: ProxyState,
    next_id: u64,
    clients: BTreeMap<u64, TcpStream>,
    aborts: BTreeMap<u64, AbortHandle>,
}

/// Running loopback tunnel proxy for one remote-server browser attachment.
pub struct TunnelProxy {
    pub host: &'static str,
    pub port: u16,
    pub url: String,
    pub credentials: ProxyCredentials,
    shared: Arc<Mutex<SharedProxy>>,
    listener_addr: std::net::SocketAddr,
    accept_thread: Option<JoinHandle<()>>,
}

impl TunnelProxy {
    /// Starts an authenticated loopback proxy forwarding all traffic through `server`'s browser
    /// tunnel RPCs for `(location, session_id, connection_id)`.
    pub fn start(
        server: Server,
        location: LocationRef,
        session_id: String,
        connection_id: String,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| format!("Failed to bind browser tunnel proxy: {e}"))?;
        let listener_addr = listener
            .local_addr()
            .map_err(|e| format!("Failed to read browser tunnel proxy address: {e}"))?;
        let username = random_hex(16);
        let password = random_hex(32);
        let expected_auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
        );
        let shared = Arc::new(Mutex::new(SharedProxy {
            state: ProxyState::Open,
            next_id: 1,
            clients: BTreeMap::new(),
            aborts: BTreeMap::new(),
        }));
        let ctx = Arc::new(ProxyCtx {
            server,
            location,
            session_id,
            connection_id,
            expected_auth: expected_auth.into_bytes(),
            shared: shared.clone(),
        });
        let accept_thread = std::thread::Builder::new()
            .name("browser-tunnel-proxy".into())
            .spawn(move || accept_loop(listener, ctx))
            .map_err(|e| format!("Failed to spawn browser tunnel proxy thread: {e}"))?;
        let port = listener_addr.port();
        Ok(Self {
            host: "127.0.0.1",
            port,
            url: format!("http://127.0.0.1:{port}"),
            credentials: ProxyCredentials { username, password },
            shared,
            listener_addr,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn close(&mut self) {
        let (clients, aborts) = {
            let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            if g.state.step(ProxyEvent::Close) != Some(ProxyEffect::Shutdown) {
                return;
            }
            (
                std::mem::take(&mut g.clients).into_values().collect::<Vec<_>>(),
                std::mem::take(&mut g.aborts).into_values().collect::<Vec<_>>(),
            )
        };
        for a in aborts {
            a.abort();
        }
        for c in clients {
            c.shutdown(Shutdown::Both).ok();
        }
        // Poke the blocking accept() on loopback so the listener exits immediately.
        TcpStream::connect_timeout(&self.listener_addr, Duration::from_millis(200)).ok();
        if let Some(handle) = self.accept_thread.take() {
            handle.join().ok();
        }
    }
}

impl Drop for TunnelProxy {
    fn drop(&mut self) {
        self.close();
    }
}

struct ProxyCtx {
    server: Server,
    location: LocationRef,
    session_id: String,
    connection_id: String,
    expected_auth: Vec<u8>,
    shared: Arc<Mutex<SharedProxy>>,
}

/// Removes a connection's tracked socket clone and abort handles when `handle_client` exits,
/// guaranteeing zero OS file-descriptor or memory leaks across long sessions.
struct ConnGuard {
    ctx: Arc<ProxyCtx>,
    client_id: u64,
    abort_ids: Vec<u64>,
}

impl ConnGuard {
    fn track_abort(&mut self, abort: &AbortHandle) -> bool {
        let mut g = self.ctx.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.state.admits() {
            abort.abort();
            return false;
        }
        let id = g.next_id;
        g.next_id = g.next_id.saturating_add(1);
        g.aborts.insert(id, abort.clone());
        self.abort_ids.push(id);
        true
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        let mut g = self.ctx.shared.lock().unwrap_or_else(PoisonError::into_inner);
        g.clients.remove(&self.client_id);
        for id in &self.abort_ids {
            g.aborts.remove(id);
        }
    }
}

impl ProxyCtx {
    fn admits(&self) -> bool {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner).state.admits()
    }

    fn authorized(&self, header: Option<&str>) -> bool {
        let Some(value) = header else { return false };
        constant_time_eq(value.trim().as_bytes(), &self.expected_auth)
    }

    fn track_client(self: &Arc<Self>, stream: &TcpStream) -> Option<ConnGuard> {
        let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.state.admits() {
            return None;
        }
        let id = g.next_id;
        g.next_id = g.next_id.saturating_add(1);
        if let Ok(cloned) = stream.try_clone() {
            g.clients.insert(id, cloned);
        }
        Some(ConnGuard { ctx: self.clone(), client_id: id, abort_ids: Vec::new() })
    }
}

fn close_tunnel(ctx: &ProxyCtx, tunnel_id: &str) {
    ctx.server
        .browser_rpc_tunnel_close(&ctx.location, &ctx.session_id, &ctx.connection_id, tunnel_id)
        .ok();
}

fn accept_loop(listener: TcpListener, ctx: Arc<ProxyCtx>) {
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else { break };
        let Some(guard) = ctx.track_client(&stream) else {
            stream.shutdown(Shutdown::Both).ok();
            break;
        };
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("browser-tunnel-conn".into())
            .spawn(move || handle_client(stream, ctx, guard))
            .ok();
    }
}

fn handle_client(stream: TcpStream, ctx: Arc<ProxyCtx>, mut guard: ConnGuard) {
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(HEADER_TIMEOUT)).ok();
    let mut reader = BufReader::new(stream);
    let Some(req) = read_http_head(&mut reader) else {
        write_status(reader.get_mut(), "400 Bad Request").ok();
        return;
    };
    if !ctx.authorized(req.header("proxy-authorization")) {
        reader
            .get_mut()
            .write_all(
                format!(
                    "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"{PROXY_REALM}\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .ok();
        return;
    }
    reader.get_mut().set_read_timeout(None).ok();
    let is_connect = req.method.eq_ignore_ascii_case("CONNECT");
    let upgrade_hdr = req.header("upgrade");
    let is_ws = !is_connect && upgrade_hdr.is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if !is_connect && upgrade_hdr.is_some() && !is_ws {
        write_status(reader.get_mut(), "400 Bad Request").ok();
        return;
    }
    let parsed = if is_connect {
        parse_connect_authority(&req.target)
    } else {
        parse_proxy_url(&req.target).filter(|u| {
            if is_ws {
                matches!(u.scheme.as_str(), "http" | "ws")
            } else {
                u.scheme == "http"
            }
        })
    };
    let Some(url) = parsed else {
        write_status(reader.get_mut(), "400 Bad Request").ok();
        return;
    };
    let Some(target) = TunnelTarget::parse(&url.host, url.port) else {
        write_status(reader.get_mut(), "400 Bad Request").ok();
        return;
    };
    let open_abort = AbortHandle::new();
    if !guard.track_abort(&open_abort) {
        return;
    }
    let tunnel_id = match ctx.server.browser_rpc_tunnel_open(
        &ctx.location,
        &ctx.session_id,
        &ctx.connection_id,
        &target,
        &open_abort,
    ) {
        Ok(id) => id,
        Err(_) => {
            write_status(reader.get_mut(), "502 Bad Gateway").ok();
            return;
        }
    };
    if !ctx.admits() {
        close_tunnel(&ctx, &tunnel_id);
        return;
    }
    if is_connect {
        if reader
            .get_mut()
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .is_err()
        {
            close_tunnel(&ctx, &tunnel_id);
            return;
        }
    } else {
        let head = build_forwarded_head(&req, &url, is_ws);
        if ctx
            .server
            .browser_rpc_tunnel_write(
                &ctx.location,
                &ctx.session_id,
                &ctx.connection_id,
                &tunnel_id,
                head.as_bytes(),
                false,
            )
            .is_err()
        {
            write_status(reader.get_mut(), "502 Bad Gateway").ok();
            close_tunnel(&ctx, &tunnel_id);
            return;
        }
    }

    // Flush any bytes already buffered in `BufReader` before pumping raw TCP.
    let prebuffered = reader.buffer().to_vec();
    let mut client_read = reader.into_inner();
    let Ok(mut client_write) = client_read.try_clone() else {
        close_tunnel(&ctx, &tunnel_id);
        return;
    };
    if !prebuffered.is_empty() {
        for chunk in prebuffered.chunks(TUNNEL_CHUNK_BYTES) {
            if ctx
                .server
                .browser_rpc_tunnel_write(
                    &ctx.location,
                    &ctx.session_id,
                    &ctx.connection_id,
                    &tunnel_id,
                    chunk,
                    false,
                )
                .is_err()
            {
                close_tunnel(&ctx, &tunnel_id);
                return;
            }
        }
    }

    let read_abort = AbortHandle::new();
    if !guard.track_abort(&read_abort) {
        close_tunnel(&ctx, &tunnel_id);
        return;
    }

    let ctx_down = ctx.clone();
    let tid_down = tunnel_id.clone();
    let abort_down = read_abort.clone();
    let plain_http = !is_connect && !is_ws;
    let down_thread = std::thread::Builder::new()
        .name("browser-tunnel-down".into())
        .spawn(move || {
            let mut state = ReadState::Idle;
            while state.step(ReadEvent::Pull) {
                match ctx_down.server.browser_rpc_tunnel_read(
                    &ctx_down.location,
                    &ctx_down.session_id,
                    &ctx_down.connection_id,
                    &tid_down,
                    &abort_down,
                ) {
                    Ok(chunk) => {
                        if chunk.eof {
                            state.step(ReadEvent::Eof);
                            client_write.shutdown(Shutdown::Write).ok();
                            // Plain HTTP sends `Connection: close`; bound the client read side
                            // so a client ignoring FIN cannot pin the handler thread.
                            if plain_http {
                                client_write.set_read_timeout(Some(POST_EOF_DRAIN_TIMEOUT)).ok();
                            }
                            break;
                        }
                        if !chunk.data.is_empty() && client_write.write_all(&chunk.data).is_err() {
                            state.step(ReadEvent::Destroyed);
                            client_write.shutdown(Shutdown::Both).ok();
                            break;
                        }
                        state.step(ReadEvent::Chunk);
                    }
                    Err(_) => {
                        state.step(ReadEvent::Destroyed);
                        client_write.shutdown(Shutdown::Both).ok();
                        break;
                    }
                }
            }
        })
        .ok();

    let mut clean_eof = false;
    let mut buf = vec![0u8; TUNNEL_CHUNK_BYTES];
    loop {
        match client_read.read(&mut buf) {
            Ok(0) => {
                ctx.server
                    .browser_rpc_tunnel_write(
                        &ctx.location,
                        &ctx.session_id,
                        &ctx.connection_id,
                        &tunnel_id,
                        &[],
                        true,
                    )
                    .ok();
                clean_eof = true;
                break;
            }
            Ok(n) => {
                if ctx
                    .server
                    .browser_rpc_tunnel_write(
                        &ctx.location,
                        &ctx.session_id,
                        &ctx.connection_id,
                        &tunnel_id,
                        &buf[..n],
                        false,
                    )
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    // Half-open TCP (`allowHalfOpen: true`): when the client cleanly half-closes its write side,
    // wait for the upstream response pump to deliver the reply and hit EOF before tearing down.
    if clean_eof {
        if let Some(t) = down_thread {
            t.join().ok();
        }
        client_read.shutdown(Shutdown::Both).ok();
        close_tunnel(&ctx, &tunnel_id);
        return;
    }

    read_abort.abort();
    client_read.shutdown(Shutdown::Both).ok();
    close_tunnel(&ctx, &tunnel_id);
    if let Some(t) = down_thread {
        t.join().ok();
    }
}

struct ParsedHead {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl ParsedHead {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn read_http_head(reader: &mut BufReader<TcpStream>) -> Option<ParsedHead> {
    let mut total = 0usize;
    let mut first = String::new();
    let n = reader.read_line(&mut first).ok()?;
    if n == 0 || n > MAX_HEADER_BYTES {
        return None;
    }
    total += n;
    let mut parts = first.trim_end_matches(['\r', '\n']).split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let _version = parts.next()?;
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        total += n;
        if n == 0 || total > MAX_HEADER_BYTES {
            return None;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    Some(ParsedHead { method, target, headers })
}

struct ProxyUrl {
    scheme: String,
    host: String,
    host_header: String,
    port: u16,
    path_and_query: String,
}

fn parse_connect_authority(target: &str) -> Option<ProxyUrl> {
    if target.is_empty()
        || target.contains(['/', '?', '#', '@'])
        || target.chars().any(char::is_whitespace)
    {
        return None;
    }
    parse_proxy_url(&format!("https://{target}"))
}

fn parse_proxy_url(raw: &str) -> Option<ProxyUrl> {
    let (scheme_raw, rest) = raw.split_once("://")?;
    let scheme = scheme_raw.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" | "ws" => 80u16,
        "https" | "wss" => 443u16,
        _ => return None,
    };
    let rest_no_frag = rest.split('#').next().unwrap_or(rest);
    let (authority, path_and_query) = match rest_no_frag.find(['/', '?']) {
        Some(idx) => {
            let tail = &rest_no_frag[idx..];
            let pq = if tail.starts_with('?') {
                format!("/{tail}")
            } else {
                tail.to_owned()
            };
            (&rest_no_frag[..idx], pq)
        }
        None => (rest_no_frag, "/".to_owned()),
    };
    if authority.is_empty() || authority.contains('@') || authority.chars().any(char::is_whitespace) {
        return None;
    }
    let (host, port) = if let Some(after_open) = authority.strip_prefix('[') {
        let close = after_open.find(']')?;
        let host = &after_open[..close];
        let tail = &after_open[close + 1..];
        let port = if tail.is_empty() {
            default_port
        } else {
            tail.strip_prefix(':')?.parse::<u16>().ok()?
        };
        (host.to_owned(), port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        let port = p.parse::<u16>().ok()?;
        (h.to_owned(), port)
    } else {
        (authority.to_owned(), default_port)
    };
    if host.is_empty() || port == 0 {
        return None;
    }
    Some(ProxyUrl {
        scheme,
        host,
        host_header: authority.to_owned(),
        port,
        path_and_query,
    })
}

fn build_forwarded_head(req: &ParsedHead, url: &ProxyUrl, is_ws: bool) -> String {
    let mut hop_names: Vec<String> = req
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
            "host",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let mut out = format!("{} {} HTTP/1.1\r\nHost: {}\r\n", req.method, url.path_and_query, url.host_header);
    if is_ws {
        out.push_str("Connection: Upgrade\r\nUpgrade: websocket\r\n");
    } else {
        out.push_str("Connection: close\r\n");
    }
    for (k, v) in &req.headers {
        let lower = k.to_ascii_lowercase();
        if hop_names.iter().any(|h| h == &lower) {
            continue;
        }
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out
}

fn write_status(stream: &mut TcpStream, status: &str) -> std::io::Result<()> {
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
