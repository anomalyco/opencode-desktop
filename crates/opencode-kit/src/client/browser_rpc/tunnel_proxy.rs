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
//!   (with `SO_EXCLUSIVEADDRUSE` on Windows) and OS-CSPRNG (`getrandom::fill`, fail-closed) 16 B
//!   username + 32 B password per proxy instance (never reused across recycled ports).
//! - **Mandatory auth & header stripping:** Constant-time `Proxy-Authorization` check on every
//!   request and every `CONNECT` (`407 Proxy Authentication Required` otherwise); strips
//!   `Proxy-Authorization`, `Transfer-Encoding`, and hop-by-hop headers before forwarding.
//! - **Pool isolation & active watchdog deadlines:** Unauthenticated sockets use a separate smaller
//!   16-slot pool so unauthenticated floods cannot starve the 64 authenticated tunnel slots. A
//!   `Condvar` deadline watchdog actively shuts down (`Shutdown::Both`) and aborts expired sockets
//!   (pre-auth head timeout, post-EOF drain for stalled POST bodies and half-open CONNECT/WS
//!   clients, and idle body/stream bounds) rather than relying on `SO_RCVTIMEO` waking blocked reads.

use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use base64::Engine;
use rune::time::Instant;
use socket2::{Domain, Protocol, Socket, Type};

use super::framing::{
    build_forwarded_head, constant_time_eq, parse_connect_authority, parse_content_length,
    parse_proxy_url, read_http_head, write_status,
};
use super::http::AbortHandle;
use super::proxy_machine::{ProxyEffect, ProxyEvent, ProxyState};
use super::tunnel_pump::{
    WsHandshake, await_ws_upgrade, drain_remote_to_client, pump_bidirectional_stream,
    pump_exact_http_body, write_all_chunks,
};
use super::types::{LocationRef, MAX_TUNNELS, TunnelTarget, try_random_hex};
use crate::client::Server;

pub const PROXY_REALM: &str = "OpenCode Browser Proxy";
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const MAX_WS_RESP_HEAD: usize = 64 * 1024;
pub(super) const POST_EOF_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const CLIENT_POLL_SLICE: Duration = Duration::from_millis(500);
const MAX_UNAUTH_CONNS: usize = 16;
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

struct TrackedClient {
    stream: TcpStream,
    deadline: Instant,
    post_eof: bool,
    expired: bool,
    aborts: Vec<AbortHandle>,
}

struct SharedProxy {
    state: ProxyState,
    suspended: bool,
    next_id: u64,
    unauth_clients: BTreeMap<u64, TrackedClient>,
    auth_clients: BTreeMap<u64, TrackedClient>,
}

impl SharedProxy {
    fn drain_all(&mut self) -> (Vec<TcpStream>, Vec<AbortHandle>) {
        let mut streams = Vec::with_capacity(self.unauth_clients.len() + self.auth_clients.len());
        let mut aborts = Vec::new();
        for c in std::mem::take(&mut self.unauth_clients)
            .into_values()
            .chain(std::mem::take(&mut self.auth_clients).into_values())
        {
            streams.push(c.stream);
            aborts.extend(c.aborts);
        }
        (streams, aborts)
    }

    fn client_mut(&mut self, id: u64) -> Option<&mut TrackedClient> {
        self.auth_clients.get_mut(&id).or_else(|| self.unauth_clients.get_mut(&id))
    }
}

/// Running loopback tunnel proxy for one remote-server browser attachment.
pub struct TunnelProxy {
    pub host: &'static str,
    pub port: u16,
    pub url: String,
    pub credentials: ProxyCredentials,
    shared: Arc<Mutex<SharedProxy>>,
    watchdog_cv: Arc<Condvar>,
    listener_addr: std::net::SocketAddr,
    accept_thread: Option<JoinHandle<()>>,
    watchdog_thread: Option<JoinHandle<()>>,
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
            unauth_clients: BTreeMap::new(),
            auth_clients: BTreeMap::new(),
        }));
        let watchdog_cv = Arc::new(Condvar::new());
        let ctx = Arc::new(ProxyCtx {
            server,
            location,
            session_id,
            connection_id,
            expected_auth: expected_auth.into_bytes(),
            shared: shared.clone(),
            watchdog_cv: watchdog_cv.clone(),
        });
        let wd_shared = shared.clone();
        let wd_cv = watchdog_cv.clone();
        let watchdog_thread = std::thread::Builder::new()
            .name("browser-tunnel-watchdog".into())
            .spawn(move || watchdog_loop(wd_shared, wd_cv))
            .map_err(|e| format!("Failed to spawn browser tunnel watchdog: {e}"))?;
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
            watchdog_cv,
            listener_addr,
            accept_thread: Some(accept_thread),
            watchdog_thread: Some(watchdog_thread),
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
            if suspended { g.drain_all() } else { (Vec::new(), Vec::new()) }
        };
        if suspended {
            self.watchdog_cv.notify_all();
        }
        for abort in aborts {
            abort.abort();
        }
        for stream in clients {
            stream.shutdown(Shutdown::Both).ok();
        }
    }

    /// Closes the proxy immediately: stops accepting, aborts all in-flight `tunnel.{open,read,write}`
    /// HTTP calls, shuts down every live client socket, and joins accept and watchdog threads.
    pub fn close(&mut self) {
        let (clients, aborts, effect) = {
            let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            let effect = g.state.step(ProxyEvent::Close);
            let (clients, aborts) = g.drain_all();
            (clients, aborts, effect)
        };
        self.watchdog_cv.notify_all();
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
        if let Some(handle) = self.watchdog_thread.take() {
            handle.join().ok();
        }
    }
}

impl Drop for TunnelProxy {
    fn drop(&mut self) {
        self.close();
    }
}

fn watchdog_loop(shared: Arc<Mutex<SharedProxy>>, cv: Arc<Condvar>) {
    let mut g = shared.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        if !g.state.admits() {
            break;
        }
        let now = Instant::now();
        let mut kill_streams = Vec::new();
        let mut kill_aborts = Vec::new();
        let mut next_deadline: Option<Instant> = None;
        let SharedProxy { unauth_clients, auth_clients, .. } = &mut *g;
        for c in unauth_clients.values_mut().chain(auth_clients.values_mut()) {
            if c.expired {
                continue;
            }
            if c.deadline <= now {
                c.expired = true;
                if let Ok(s) = c.stream.try_clone() {
                    kill_streams.push(s);
                }
                kill_aborts.extend(c.aborts.iter().cloned());
            } else {
                next_deadline = Some(next_deadline.map_or(c.deadline, |cur| cur.min(c.deadline)));
            }
        }
        if !kill_streams.is_empty() || !kill_aborts.is_empty() {
            drop(g);
            for abort in kill_aborts {
                abort.abort();
            }
            for stream in kill_streams {
                stream.shutdown(Shutdown::Both).ok();
            }
            g = shared.lock().unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        g = match next_deadline {
            Some(at) => {
                let wait = at.saturating_duration_since(Instant::now()).max(Duration::from_millis(5));
                cv.wait_timeout(g, wait).unwrap_or_else(PoisonError::into_inner).0
            }
            None => cv.wait(g).unwrap_or_else(PoisonError::into_inner),
        };
    }
}

pub(super) struct ProxyCtx {
    pub(super) server: Server,
    pub(super) location: LocationRef,
    pub(super) session_id: String,
    pub(super) connection_id: String,
    expected_auth: Vec<u8>,
    shared: Arc<Mutex<SharedProxy>>,
    watchdog_cv: Arc<Condvar>,
}

struct ConnGuard {
    ctx: Arc<ProxyCtx>,
    client_id: u64,
    authenticated: bool,
}

impl ConnGuard {
    fn promote_authenticated(&mut self, idle_timeout: Duration) -> Result<(), AdmitError> {
        let mut g = self.ctx.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.state.admits() {
            return Err(AdmitError::Closed);
        }
        if g.suspended {
            return Err(AdmitError::Suspended);
        }
        let Some(mut entry) = g.unauth_clients.remove(&self.client_id) else {
            return Err(AdmitError::Closed);
        };
        if entry.expired {
            return Err(AdmitError::Closed);
        }
        if g.auth_clients.len() >= MAX_CONCURRENT_CONNS {
            return Err(AdmitError::AtCapacity);
        }
        entry.deadline = Instant::now() + idle_timeout;
        g.auth_clients.insert(self.client_id, entry);
        self.authenticated = true;
        drop(g);
        self.ctx.watchdog_cv.notify_one();
        Ok(())
    }

    fn track_abort(&mut self, abort: &AbortHandle) -> bool {
        let mut g = self.ctx.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.state.admits() || g.suspended {
            abort.abort();
            return false;
        }
        let Some(entry) = g.client_mut(self.client_id) else {
            abort.abort();
            return false;
        };
        if entry.expired {
            abort.abort();
            return false;
        }
        entry.aborts.push(abort.clone());
        true
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        let mut g = self.ctx.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if self.authenticated {
            g.auth_clients.remove(&self.client_id);
        } else {
            g.unauth_clients.remove(&self.client_id);
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

    fn track_unauth(self: &Arc<Self>, stream: &TcpStream) -> Result<ConnGuard, AdmitError> {
        let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.state.admits() {
            return Err(AdmitError::Closed);
        }
        if g.suspended {
            return Err(AdmitError::Suspended);
        }
        if g.unauth_clients.len() >= MAX_UNAUTH_CONNS {
            return Err(AdmitError::AtCapacity);
        }
        let id = g.next_id;
        g.next_id = g.next_id.saturating_add(1);
        let cloned = stream.try_clone().map_err(|_| AdmitError::Closed)?;
        g.unauth_clients.insert(
            id,
            TrackedClient {
                stream: cloned,
                deadline: Instant::now() + HEADER_TIMEOUT,
                post_eof: false,
                expired: false,
                aborts: Vec::new(),
            },
        );
        drop(g);
        self.watchdog_cv.notify_one();
        Ok(ConnGuard { ctx: self.clone(), client_id: id, authenticated: false })
    }

    fn set_idle_timeout(&self, client_id: u64, timeout: Duration) {
        let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = g.auth_clients.get_mut(&client_id)
            && !c.post_eof
            && !c.expired
        {
            c.deadline = Instant::now() + timeout;
            drop(g);
            self.watchdog_cv.notify_one();
        }
    }

    pub(super) fn bump_idle(&self, client_id: u64, timeout: Duration) {
        let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = g.auth_clients.get_mut(&client_id)
            && !c.post_eof
            && !c.expired
        {
            let next = Instant::now() + timeout;
            if next > c.deadline {
                c.deadline = next;
            }
        }
    }

    pub(super) fn arm_post_eof(&self, client_id: u64, timeout: Duration) {
        let mut g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = g.auth_clients.get_mut(&client_id)
            && !c.expired
        {
            c.post_eof = true;
            let next = Instant::now() + timeout;
            if next < c.deadline {
                c.deadline = next;
            }
            drop(g);
            self.watchdog_cv.notify_one();
        }
    }

    pub(super) fn is_expired(&self, client_id: u64) -> bool {
        let g = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.state.admits() || g.suspended {
            return true;
        }
        match g.auth_clients.get(&client_id) {
            Some(c) => c.expired || Instant::now() >= c.deadline,
            None => true,
        }
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
        match ctx.track_unauth(&stream) {
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
    match guard.promote_authenticated(BODY_IDLE_TIMEOUT) {
        Ok(()) => {}
        Err(AdmitError::AtCapacity | AdmitError::Suspended) => {
            write_status(reader.get_mut(), "503 Service Unavailable").ok();
            return;
        }
        Err(AdmitError::Closed) => return,
    }

    let is_connect = req.method.eq_ignore_ascii_case("CONNECT");
    let upgrade_hdr = req.header("upgrade");
    let is_ws = !is_connect && upgrade_hdr.is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if is_ws && (!req.method.eq_ignore_ascii_case("GET") || req.version != "HTTP/1.1") {
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

    let client_id = guard.client_id;
    let plain_http = !is_connect && !is_ws;
    let idle_timeout = if plain_http { BODY_IDLE_TIMEOUT } else { STREAM_IDLE_TIMEOUT };
    ctx.set_idle_timeout(client_id, idle_timeout);

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
    // Bounded read slice so client read loops also poll watchdog expiry promptly on every platform.
    client_read.set_read_timeout(Some(CLIENT_POLL_SLICE)).ok();
    let Ok(mut client_write) = client_read.try_clone() else {
        close_tunnel(&ctx, &tunnel_id);
        return;
    };

    let read_abort = AbortHandle::new();
    if !guard.track_abort(&read_abort) {
        close_tunnel(&ctx, &tunnel_id);
        return;
    }
    // For WebSocket upgrades (`GET ... HTTP/1.1 Upgrade: websocket`), wait for remote `101` BEFORE
    // forwarding any client bytes so a rejected upgrade can never smuggle a second HTTP/1.1 request.
    if is_ws {
        match await_ws_upgrade(&ctx, &tunnel_id, &read_abort, &mut client_write) {
            WsHandshake::Upgraded => {}
            WsHandshake::RejectedNot101 => {
                let _ = write_all_chunks(&ctx, &tunnel_id, &[], true, &io_abort);
                drain_remote_to_client(
                    &ctx,
                    client_id,
                    &tunnel_id,
                    &read_abort,
                    &mut client_write,
                    BODY_IDLE_TIMEOUT,
                );
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
    let down_thread = std::thread::Builder::new()
        .name("browser-tunnel-down".into())
        .spawn(move || {
            drain_remote_to_client(
                &ctx_down,
                client_id,
                &tid_down,
                &abort_down,
                &mut client_write,
                idle_timeout,
            );
        })
        .ok();

    let clean_eof = if plain_http {
        pump_exact_http_body(
            &ctx,
            client_id,
            &tunnel_id,
            &prebuffered,
            &mut client_read,
            content_length,
            &io_abort,
        )
    } else {
        pump_bidirectional_stream(
            &ctx,
            client_id,
            &tunnel_id,
            &[],
            &mut client_read,
            &io_abort,
        )
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
