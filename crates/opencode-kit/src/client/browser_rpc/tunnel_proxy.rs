//! Authenticated local loopback HTTP/1.1 + CONNECT + WebSocket proxy forwarding remote-server
//! browser traffic strictly through `experimental.browser/tunnel.{open,read,write,close}`
//! (`packages/plugin-browser/src/proxy.ts`, `packages/gui-plugins/src/browser/network.ts`).
//!
//! Security invariants:
//! - **Remote-only egress:** Never dials any destination on the user's machine (zero local sockets
//!   for user loopback `127.0.0.1`/`localhost`/`[::1]`, RFC 1918 private ranges, link-local
//!   `169.254.0.0/16`/`fe80::/10`, ULA `fc00::/7`, or the app's own OpenCode ports). Every
//!   accepted HTTP request and `CONNECT` tunnel dials exclusively via the remote server's
//!   `experimental.browser/tunnel.open` RPC so loopback targets resolve on the remote server.
//! - **Loopback IP literal bind & fresh 384-bit credentials:** Binds strictly to `127.0.0.1:0`
//!   with OS-CSPRNG (`getrandom::fill`, fail-closed) 16 B username + 32 B password per proxy
//!   instance (never reused across recycled ports).
//! - **Mandatory auth & header stripping:** Constant-time `Proxy-Authorization` check on every
//!   request and every `CONNECT` (`407 Proxy Authentication Required` otherwise); strips
//!   `Proxy-Authorization`, `Transfer-Encoding`, and hop-by-hop headers before forwarding.
//! - **Strict HTTP/1.1 framing & concurrency bounds:** Bounded 64 KiB header read, rejects bare
//!   `\r`/`\n`, control bytes, obs-fold, `Expect`, `Transfer-Encoding`, conflicting/invalid
//!   `Content-Length`, and `CONNECT` without explicit `host:port` (`1..=65535`); plain HTTP
//!   forwards exact `Content-Length` bytes (no raw pipelined splicing); hard cap of 64 concurrent
//!   connections (`503` beyond cap); abortable open/read/write RPCs on shutdown.

use std::collections::BTreeMap;
use std::io::{BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use base64::Engine;
use rune::time::Instant;
use socket2::{Domain, Protocol, Socket, Type};

use super::framing::{
    build_forwarded_head, is_http_101_response, parse_connect_authority, parse_content_length,
    parse_proxy_url, read_http_head,
};
use super::http::AbortHandle;
use super::proxy_machine::{ProxyEffect, ProxyEvent, ProxyState, ReadEvent, ReadState};
use super::types::{LocationRef, MAX_TUNNELS, TUNNEL_CHUNK_BYTES, TunnelTarget, try_random_hex};
use crate::client::Server;

pub const PROXY_REALM: &str = "OpenCode Browser Proxy";
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_WS_RESP_HEAD: usize = 64 * 1024;
const POST_EOF_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONCURRENT_CONNS: usize = MAX_TUNNELS;

/// Binds an exclusive loopback TCP listener on `127.0.0.1:0`. On Windows, sets
/// `SO_EXCLUSIVEADDRUSE` before `bind` so no same-user process can co-bind/hijack via `SO_REUSEADDR`.
fn bind_exclusive_loopback() -> std::io::Result<TcpListener> {
    let sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{SO_EXCLUSIVEADDRUSE, SOL_SOCKET, setsockopt};
        let one: i32 = 1;
        #[allow(unsafe_code)]
        // SAFETY: `sock` is a valid open Winsock socket and `&one` points to 4 readable bytes.
        let rc = unsafe {
            setsockopt(
                sock.as_raw_socket() as _,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                (&one as *const i32).cast::<u8>(),
                std::mem::size_of::<i32>() as i32,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    sock.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())?;
    sock.listen(128)?;
    Ok(TcpListener::from(sock))
}

#[derive(Clone)]
pub struct ProxyCredentials {
    pub username: String,
    pub password: String,
}

struct SharedProxy {
    state: ProxyState,
    suspended: bool,
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
    /// Starts an authenticated loopback proxy bound strictly to IP literal `127.0.0.1:0` with
    /// fresh 384-bit OS-CSPRNG credentials, forwarding all traffic through `server`'s browser
    /// tunnel RPCs for `(location, session_id, connection_id)`.
    pub fn start(
        server: Server,
        location: LocationRef,
        session_id: String,
        connection_id: String,
    ) -> Result<Self, String> {
        let username = try_random_hex(16)?;
        let password = try_random_hex(32)?;
        let listener = bind_exclusive_loopback()
            .map_err(|e| format!("Failed to bind browser tunnel proxy: {e}"))?;
        let listener_addr = listener
            .local_addr()
            .map_err(|e| format!("Failed to read browser tunnel proxy address: {e}"))?;
        let expected_auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
        );
        let shared = Arc::new(Mutex::new(SharedProxy {
            state: ProxyState::Open,
            suspended: false,
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

    /// Temporarily suspends or resumes forwarding without closing the listener port (used during
    /// transient remote reconnect so requests fail closed with `503` without tearing down tabs).
    pub fn set_suspended(&self, suspended: bool) {
        let (clients, aborts) = {
            let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            if g.suspended == suspended {
                return;
            }
            g.suspended = suspended;
            if suspended {
                (
                    std::mem::take(&mut g.clients).into_values().collect::<Vec<_>>(),
                    std::mem::take(&mut g.aborts).into_values().collect::<Vec<_>>(),
                )
            } else {
                (Vec::new(), Vec::new())
            }
        };
        for abort in aborts {
            abort.abort();
        }
        for stream in clients {
            stream.shutdown(Shutdown::Both).ok();
        }
    }

    /// Closes the proxy immediately: stops accepting, aborts all in-flight `tunnel.{open,read,write}`
    /// HTTP calls, shuts down every live client socket, and joins the accept thread.
    pub fn close(&mut self) {
        let (clients, aborts, effect) = {
            let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            let effect = g.state.step(ProxyEvent::Close);
            let clients: Vec<TcpStream> = std::mem::take(&mut g.clients).into_values().collect();
            let aborts: Vec<AbortHandle> = std::mem::take(&mut g.aborts).into_values().collect();
            (clients, aborts, effect)
        };
        if effect == Some(ProxyEffect::Shutdown) {
            for abort in aborts {
                abort.abort();
            }
            for stream in clients {
                stream.shutdown(Shutdown::Both).ok();
            }
            // Wake blocking `TcpListener::incoming()` so the accept thread exits cleanly.
            let _ = TcpStream::connect_timeout(&self.listener_addr, Duration::from_millis(200));
        }
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

struct ConnGuard {
    ctx: Arc<ProxyCtx>,
    client_id: u64,
    abort_ids: Vec<u64>,
}

impl ConnGuard {
    fn track_abort(&mut self, abort: &AbortHandle) -> bool {
        let mut g = self.ctx.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.state.admits() || g.suspended {
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

enum AdmitError {
    Closed,
    AtCapacity,
    Suspended,
}

impl ProxyCtx {
    fn admits(&self) -> bool {
        let g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        g.state.admits() && !g.suspended
    }

    fn authorized(&self, header: Option<&str>) -> bool {
        let Some(value) = header else { return false };
        constant_time_eq(value.trim().as_bytes(), &self.expected_auth)
    }

    fn track_client(self: &Arc<Self>, stream: &TcpStream) -> Result<ConnGuard, AdmitError> {
        let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.state.admits() {
            return Err(AdmitError::Closed);
        }
        if g.suspended {
            return Err(AdmitError::Suspended);
        }
        if g.clients.len() >= MAX_CONCURRENT_CONNS {
            return Err(AdmitError::AtCapacity);
        }
        let id = g.next_id;
        g.next_id = g.next_id.saturating_add(1);
        let cloned = stream.try_clone().map_err(|_| AdmitError::Closed)?;
        g.clients.insert(id, cloned);
        Ok(ConnGuard { ctx: self.clone(), client_id: id, abort_ids: Vec::new() })
    }
}

fn close_tunnel(ctx: &ProxyCtx, tunnel_id: &str) {
    ctx.server
        .browser_rpc_tunnel_close(&ctx.location, &ctx.session_id, &ctx.connection_id, tunnel_id)
        .ok();
}

fn accept_loop(listener: TcpListener, ctx: Arc<ProxyCtx>) {
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else { break };
        match ctx.track_client(&stream) {
            Ok(guard) => {
                let ctx = ctx.clone();
                std::thread::Builder::new()
                    .name("browser-tunnel-conn".into())
                    .spawn(move || handle_client(stream, ctx, guard))
                    .ok();
            }
            Err(AdmitError::AtCapacity | AdmitError::Suspended) => {
                write_status(&mut stream, "503 Service Unavailable").ok();
                stream.shutdown(Shutdown::Write).ok();
            }
            Err(AdmitError::Closed) => {
                stream.shutdown(Shutdown::Both).ok();
                break;
            }
        }
    }
}

fn handle_client(stream: TcpStream, ctx: Arc<ProxyCtx>, mut guard: ConnGuard) {
    stream.set_nodelay(true).ok();
    let head_deadline = Instant::now() + HEADER_TIMEOUT;
    stream.set_read_timeout(Some(HEADER_TIMEOUT)).ok();
    let mut reader = BufReader::new(stream);
    let req = match read_http_head(&mut reader, head_deadline) {
        Ok(r) => r,
        Err(status) => {
            write_status(reader.get_mut(), status).ok();
            return;
        }
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
    if is_ws && !req.method.eq_ignore_ascii_case("GET") {
        write_status(reader.get_mut(), "400 Bad Request").ok();
        return;
    }
    if !is_connect && upgrade_hdr.is_some() && !is_ws {
        write_status(reader.get_mut(), "400 Bad Request").ok();
        return;
    }
    if req.has_header("transfer-encoding") {
        write_status(reader.get_mut(), "501 Not Implemented").ok();
        return;
    }
    if req.has_header("expect") {
        write_status(reader.get_mut(), "417 Expectation Failed").ok();
        return;
    }
    let content_length = match parse_content_length(&req, is_connect || is_ws) {
        Ok(n) => n,
        Err(status) => {
            write_status(reader.get_mut(), status).ok();
            return;
        }
    };
    // Refuse pipelined bytes trailing a `CONNECT` or `Upgrade: websocket` request head so a client
    // cannot smuggle bytes into a tunnel before `200`/`101` confirmation.
    if (is_connect || is_ws) && !reader.buffer().is_empty() {
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

    let io_abort = AbortHandle::new();
    if !guard.track_abort(&io_abort) {
        return;
    }
    let tunnel_id = match ctx.server.browser_rpc_tunnel_open(
        &ctx.location,
        &ctx.session_id,
        &ctx.connection_id,
        &target,
        &io_abort,
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
        let head = build_forwarded_head(&req, &url, is_ws, content_length);
        if write_all_chunks(&ctx, &tunnel_id, head.as_bytes(), false, &io_abort).is_err() {
            write_status(reader.get_mut(), "502 Bad Gateway").ok();
            close_tunnel(&ctx, &tunnel_id);
            return;
        }
    }

    let prebuffered = reader.buffer().to_vec();
    let mut client_read = reader.into_inner();
    let Ok(mut client_write) = client_read.try_clone() else {
        close_tunnel(&ctx, &tunnel_id);
        return;
    };

    let read_abort = AbortHandle::new();
    if !guard.track_abort(&read_abort) {
        close_tunnel(&ctx, &tunnel_id);
        return;
    }
    // For WebSocket upgrades (`GET ... Upgrade: websocket`), wait for remote `101 Switching
    // Protocols` BEFORE forwarding any client bytes so a rejected upgrade can never smuggle a
    // second HTTP/1.1 request onto the remote keep-alive connection.
    if is_ws {
        match await_ws_upgrade(&ctx, &tunnel_id, &read_abort, &mut client_write) {
            WsHandshake::Upgraded => {}
            WsHandshake::RejectedNot101 => {
                let _ = write_all_chunks(&ctx, &tunnel_id, &[], true, &io_abort);
                drain_remote_to_client(&ctx, &tunnel_id, &read_abort, &mut client_write, true);
                client_read.shutdown(Shutdown::Both).ok();
                close_tunnel(&ctx, &tunnel_id);
                return;
            }
            WsHandshake::Closed => {
                client_read.shutdown(Shutdown::Both).ok();
                close_tunnel(&ctx, &tunnel_id);
                return;
            }
        }
    }

    let ctx_down = ctx.clone();
    let tid_down = tunnel_id.clone();
    let abort_down = read_abort.clone();
    let plain_http = !is_connect && !is_ws;
    let down_thread = std::thread::Builder::new()
        .name("browser-tunnel-down".into())
        .spawn(move || {
            drain_remote_to_client(&ctx_down, &tid_down, &abort_down, &mut client_write, plain_http);
        })
        .ok();

    let clean_eof = if plain_http {
        pump_exact_http_body(&ctx, &tunnel_id, &prebuffered, &mut client_read, content_length, &io_abort)
    } else {
        pump_bidirectional_stream(&ctx, &tunnel_id, &[], &mut client_read, &io_abort)
    };

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

fn write_all_chunks(
    ctx: &ProxyCtx,
    tunnel_id: &str,
    data: &[u8],
    end: bool,
    abort: &AbortHandle,
) -> Result<(), ()> {
    if data.is_empty() {
        return ctx
            .server
            .browser_rpc_tunnel_write_abortable(
                &ctx.location,
                &ctx.session_id,
                &ctx.connection_id,
                tunnel_id,
                &[],
                end,
                abort,
            )
            .map_err(|_| ());
    }
    let total = data.chunks(TUNNEL_CHUNK_BYTES).len();
    for (idx, chunk) in data.chunks(TUNNEL_CHUNK_BYTES).enumerate() {
        let is_last = end && idx + 1 == total;
        ctx.server
            .browser_rpc_tunnel_write_abortable(
                &ctx.location,
                &ctx.session_id,
                &ctx.connection_id,
                tunnel_id,
                chunk,
                is_last,
                abort,
            )
            .map_err(|_| ())?;
    }
    Ok(())
}

fn pump_exact_http_body(
    ctx: &ProxyCtx,
    tunnel_id: &str,
    prebuffered: &[u8],
    client_read: &mut TcpStream,
    content_length: u64,
    abort: &AbortHandle,
) -> bool {
    let mut remaining = content_length;
    if remaining > 0 && !prebuffered.is_empty() {
        let take = (prebuffered.len() as u64).min(remaining) as usize;
        remaining -= take as u64;
        if write_all_chunks(ctx, tunnel_id, &prebuffered[..take], remaining == 0, abort).is_err() {
            return false;
        }
        if remaining == 0 {
            return true;
        }
    }
    if remaining == 0 {
        return write_all_chunks(ctx, tunnel_id, &[], true, abort).is_ok();
    }
    let mut buf = vec![0u8; TUNNEL_CHUNK_BYTES];
    while remaining > 0 {
        let want = (buf.len() as u64).min(remaining) as usize;
        match client_read.read(&mut buf[..want]) {
            Ok(0) => return false,
            Ok(n) => {
                remaining -= n as u64;
                if write_all_chunks(ctx, tunnel_id, &buf[..n], remaining == 0, abort).is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

fn pump_bidirectional_stream(
    ctx: &ProxyCtx,
    tunnel_id: &str,
    prebuffered: &[u8],
    client_read: &mut TcpStream,
    abort: &AbortHandle,
) -> bool {
    if !prebuffered.is_empty() && write_all_chunks(ctx, tunnel_id, prebuffered, false, abort).is_err() {
        return false;
    }
    let mut buf = vec![0u8; TUNNEL_CHUNK_BYTES];
    loop {
        match client_read.read(&mut buf) {
            Ok(0) => {
                if !ctx.admits() {
                    return false;
                }
                let _ = write_all_chunks(ctx, tunnel_id, &[], true, abort);
                return true;
            }
            Ok(n) => {
                if write_all_chunks(ctx, tunnel_id, &buf[..n], false, abort).is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

enum WsHandshake {
    Upgraded,
    RejectedNot101,
    Closed,
}

fn await_ws_upgrade(
    ctx: &ProxyCtx,
    tunnel_id: &str,
    abort: &AbortHandle,
    client_write: &mut TcpStream,
) -> WsHandshake {
    let mut buf = Vec::with_capacity(512);
    loop {
        let Ok(chunk) = ctx.server.browser_rpc_tunnel_read(
            &ctx.location,
            &ctx.session_id,
            &ctx.connection_id,
            tunnel_id,
            abort,
        ) else {
            return WsHandshake::Closed;
        };
        if !chunk.data.is_empty() {
            buf.extend_from_slice(&chunk.data);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = &buf[..pos];
                let ok101 = is_http_101_response(head);
                if client_write.write_all(&buf).is_err() {
                    return WsHandshake::Closed;
                }
                if chunk.eof {
                    client_write.shutdown(Shutdown::Write).ok();
                    return WsHandshake::Closed;
                }
                return if ok101 { WsHandshake::Upgraded } else { WsHandshake::RejectedNot101 };
            }
            if buf.len() > MAX_WS_RESP_HEAD {
                let _ = write_status(client_write, "502 Bad Gateway");
                return WsHandshake::Closed;
            }
        }
        if chunk.eof {
            if !buf.is_empty() {
                let _ = client_write.write_all(&buf);
            }
            client_write.shutdown(Shutdown::Write).ok();
            return WsHandshake::Closed;
        }
    }
}

fn drain_remote_to_client(
    ctx: &ProxyCtx,
    tunnel_id: &str,
    abort: &AbortHandle,
    client_write: &mut TcpStream,
    plain_http: bool,
) {
    let mut state = ReadState::Idle;
    while state.step(ReadEvent::Pull) {
        match ctx.server.browser_rpc_tunnel_read(
            &ctx.location,
            &ctx.session_id,
            &ctx.connection_id,
            tunnel_id,
            abort,
        ) {
            Ok(chunk) => {
                if chunk.eof {
                    state.step(ReadEvent::Eof);
                    client_write.shutdown(Shutdown::Write).ok();
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
}

fn write_status(stream: &mut TcpStream, status: &str) -> std::io::Result<()> {
    let resp = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    stream.write_all(resp.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for (i, &expected) in b.iter().enumerate() {
        let got = a.get(i).copied().unwrap_or(0);
        diff |= usize::from(got ^ expected);
    }
    diff == 0
}
