//! Client MCP over WebSocket (`GET /api/experimental/client-mcp?location[directory]=…&name=…`,
//! `packages/client/src/mcp.ts` and `packages/server/src/handlers/mcp.ts`).
//!
//! A client app opens a WebSocket to the OpenCode server and serves MCP JSON-RPC 2.0 (`initialize`,
//! `ping`, `tools/list`, `tools/call`) over text frames while the OpenCode server acts as the MCP
//! client. Reconnects with exponential backoff (`500 ms`..`10 s`, reset on
//! `notifications/initialized`) and backs off quietly when the server lacks the experimental route.

#[cfg(not(target_arch = "wasm32"))]
use std::io::ErrorKind;
use std::net::{Shutdown, TcpStream};
#[cfg(not(target_arch = "wasm32"))]
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rune::Waker;
use rune::time::Instant;
use serde_json::{Value, json};
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::client::IntoClientRequest;
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::http::HeaderValue;
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::protocol::CloseFrame;
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::protocol::frame::coding::CloseCode;
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::stream::MaybeTlsStream;
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::{Message, WebSocket};

use crate::client::{Server, encode_component};

#[cfg(target_arch = "wasm32")]
mod web;

pub const PROTOCOL_VERSION: &str = "2025-11-25";
pub const SESSION_ID_META_KEY: &str = "ai.opencode/sessionID";
pub const RECONNECT_MIN: Duration = Duration::from_millis(500);
pub const RECONNECT_MAX: Duration = Duration::from_secs(10);
pub const UNSUPPORTED_RETRY: Duration = Duration::from_secs(60);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a read waits for the rest of a frame once the socket said it has data.
#[cfg(not(target_arch = "wasm32"))]
const READ_TIMEOUT: Duration = Duration::from_millis(200);

/// One tool advertised on `tools/list`.
#[derive(Clone, Debug, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    /// JSON Schema for tool arguments; `"type": "object"` is ensured on `tools/list`.
    pub input_schema: Value,
}

impl McpTool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, input_schema: Value) -> Self {
        Self { name: name.into(), description: description.into(), input_schema }
    }
}

/// One content block in a `tools/call` result (`ClientMcpToolResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpContent {
    Text { text: String },
    Image { data: String, mime_type: String },
}

impl McpContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self::Image { data: data.into(), mime_type: mime_type.into() }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Text { text } => json!({ "type": "text", "text": text }),
            Self::Image { data, mime_type } => json!({ "type": "image", "data": data, "mimeType": mime_type }),
        }
    }
}

/// The payload returned from a `tools/call` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpResult {
    pub content: Vec<McpContent>,
    pub is_error: bool,
}

impl McpResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self { content: vec![McpContent::text(text)], is_error: false }
    }

    pub fn json(value: Value) -> Self {
        Self::text(value.to_string())
    }

    pub fn with_image(text: impl Into<String>, base64_data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            content: vec![McpContent::text(text), McpContent::image(base64_data, mime_type)],
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self { content: vec![McpContent::text(message)], is_error: true }
    }

    fn to_json(&self) -> Value {
        let content: Vec<Value> = self.content.iter().map(McpContent::to_json).collect();
        if self.is_error { json!({ "content": content, "isError": true }) } else { json!({ "content": content }) }
    }
}

/// Lifecycle state of an attached client MCP WebSocket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientMcpStatus {
    /// Connecting or waiting on reconnect backoff.
    Connecting,
    /// Socket open; waiting for `initialize` and `tools/list` to complete.
    Handshaking,
    /// `tools/list` has been answered on the active socket.
    Ready,
    /// Server answered HTTP 404/405/501 on the upgrade; sleeping quietly before retrying.
    Unsupported,
    /// Explicitly closed or dropped.
    Closed,
}

/// Configuration for [`Server::connect_client_mcp`].
#[derive(Clone, Debug)]
pub struct ClientMcpOptions {
    pub name: String,
    pub version: String,
    pub directory: String,
    pub instructions: Option<String>,
    pub tools: Vec<McpTool>,
    pub reconnect_min: Duration,
    pub reconnect_max: Duration,
    pub unsupported_retry: Duration,
}

impl ClientMcpOptions {
    pub fn new(name: impl Into<String>, directory: impl Into<String>, tools: Vec<McpTool>) -> Self {
        Self {
            name: name.into(),
            version: "1.0.0".into(),
            directory: directory.into(),
            instructions: None,
            tools,
            reconnect_min: RECONNECT_MIN,
            reconnect_max: RECONNECT_MAX,
            unsupported_retry: UNSUPPORTED_RETRY,
        }
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        let s = instructions.into();
        self.instructions = (!s.trim().is_empty()).then_some(s);
        self
    }
}

enum Outbound {
    Reply { id: Value, result: McpResult },
    ToolsChanged,
}

/// Where replies and notices for the open socket go: the worker's queue and a nudge that wakes the
/// worker from its wait on the socket (natively a poller, in a page the task's bell), so it sleeps
/// until there's something to do.
#[derive(Clone)]
struct Outbox {
    tx: Sender<Outbound>,
    #[cfg(not(target_arch = "wasm32"))]
    wake: Arc<polling::Poller>,
    #[cfg(target_arch = "wasm32")]
    bell: u64,
}

impl Outbox {
    fn send(&self, out: Outbound) {
        if self.tx.send(out).is_ok() {
            #[cfg(not(target_arch = "wasm32"))]
            self.wake.notify().ok();
            #[cfg(target_arch = "wasm32")]
            web::ring(self.bell);
        }
    }
}

/// An incoming `tools/call` invocation delivered to the host application.
///
/// Call [`McpCall::respond`] once finished (immediately or asynchronously across frames). If dropped
/// without responding, a cancellation error is sent automatically so the server call never stalls.
pub struct McpCall {
    pub directory: String,
    pub name: String,
    pub arguments: Value,
    pub session_id: Option<String>,
    reply: Option<(Value, Outbox)>,
}

impl std::fmt::Debug for McpCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCall")
            .field("directory", &self.directory)
            .field("name", &self.name)
            .field("arguments", &self.arguments)
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl McpCall {
    pub fn respond(mut self, result: McpResult) {
        if let Some((id, tx)) = self.reply.take() {
            tx.send(Outbound::Reply { id, result });
        }
    }
}

impl Drop for McpCall {
    fn drop(&mut self) {
        if let Some((id, tx)) = self.reply.take() {
            tx.send(Outbound::Reply { id, result: McpResult::error("Tool call cancelled") });
        }
    }
}

struct Shared {
    state: Mutex<InnerState>,
    cv: Condvar,
}

struct InnerState {
    conn: Conn,
    tools: Vec<McpTool>,
    tools_gen: u64,
}

/// The socket's life, one state at a time (`ClientMcpStatus` reads it). Official
/// `packages/client/src/mcp.ts` keeps the same lifecycle in `disposed`, `delay`, `timer` and
/// `socket`.
enum Conn {
    /// The worker is opening a socket.
    Dialing,
    /// Waiting out a backoff before the next dial; `unsupported`: the server lacks the route.
    Waiting { unsupported: bool },
    /// The socket is open; `ready` once `tools/list` was answered on it, `list_changed` when the
    /// client's `initialize` negotiated an MCP version with `notifications/tools/list_changed`.
    Open { tcp: Option<TcpStream>, out: Outbox, ready: Option<Instant>, list_changed: bool },
    /// Closed for good (`close`, or the call receiver went away).
    Closed,
}

impl Conn {
    fn status(&self) -> ClientMcpStatus {
        match self {
            Conn::Dialing | Conn::Waiting { unsupported: false } => ClientMcpStatus::Connecting,
            Conn::Waiting { unsupported: true } => ClientMcpStatus::Unsupported,
            Conn::Open { ready: None, .. } => ClientMcpStatus::Handshaking,
            Conn::Open { ready: Some(_), .. } => ClientMcpStatus::Ready,
            Conn::Closed => ClientMcpStatus::Closed,
        }
    }

    fn closed(&self) -> bool { matches!(self, Conn::Closed) }
}

/// Handle to a background client MCP WebSocket worker. Closes cleanly when dropped.
pub struct ClientMcp {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl ClientMcp {
    pub fn status(&self) -> ClientMcpStatus {
        lock(&self.shared.state).conn.status()
    }

    /// True only after `tools/list` has been answered and flushed on the current socket.
    pub fn is_ready(&self) -> bool {
        self.status() == ClientMcpStatus::Ready
    }

    /// True when `tools/list` has been answered for at least `min_age` (useful for waiting past the
    /// server's 100 ms tool-registry debounce in tests or automated launches).
    pub fn ready_for(&self, min_age: Duration) -> bool {
        matches!(lock(&self.shared.state).conn, Conn::Open { ready: Some(at), .. } if at.elapsed() >= min_age)
    }

    /// Replaces the advertised tool list and notifies the server (`notifications/tools/list_changed`)
    /// once this socket has listed tools. False only when an open socket negotiated a client
    /// protocol without `notifications/tools/list_changed`, so the caller can reconnect it.
    pub fn set_tools(&self, tools: Vec<McpTool>) -> bool {
        let mut st = lock(&self.shared.state);
        st.tools = tools;
        st.tools_gen = st.tools_gen.wrapping_add(1);
        match &st.conn {
            Conn::Open { list_changed: false, .. } => false,
            Conn::Open { out, ready: Some(_), list_changed: true, .. } => {
                out.send(Outbound::ToolsChanged);
                true
            }
            _ => true,
        }
    }

    /// Closes the WebSocket cleanly (`1000 Normal`) and stops reconnecting without blocking on slow dials.
    pub fn close(&mut self) {
        let can_join = {
            let mut st = lock(&self.shared.state);
            // A dial in flight can take its whole timeout: don't wait for it.
            let dialing = matches!(st.conn, Conn::Dialing);
            if let Conn::Open { tcp, out, .. } = std::mem::replace(&mut st.conn, Conn::Closed) {
                if let Some(tcp) = tcp {
                    tcp.shutdown(Shutdown::Read).ok();
                }
                // The worker may be asleep on the socket; it sees `Closed` once woken.
                #[cfg(not(target_arch = "wasm32"))]
                out.wake.notify().ok();
                #[cfg(target_arch = "wasm32")]
                web::ring(out.bell);
            }
            !dialing
        };
        self.shared.cv.notify_all();
        if let Some(h) = self.handle.take()
            && can_join
        {
            drop(h.join());
        }
    }
}

impl Drop for ClientMcp {
    fn drop(&mut self) {
        self.close();
    }
}

impl Server {
    /// Builds the `ws://` or `wss://` upgrade URL for `/api/experimental/client-mcp`.
    pub fn client_mcp_url(&self, directory: &str, name: &str) -> String {
        let (base_no_query, existing_query) = match self.base.split_once('?') {
            Some((b, q)) => (b.trim_end_matches('/'), q),
            None => (self.base.trim_end_matches('/'), ""),
        };
        let ws_base = if let Some(rest) = base_no_query.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base_no_query.strip_prefix("http://") {
            format!("ws://{rest}")
        } else if base_no_query.starts_with("ws://") || base_no_query.starts_with("wss://") {
            base_no_query.to_owned()
        } else {
            format!("ws://{base_no_query}")
        };
        let mut params = Vec::with_capacity(3);
        if !existing_query.is_empty() {
            params.push(existing_query.to_owned());
        }
        if !directory.is_empty() {
            params.push(format!("location%5Bdirectory%5D={}", encode_component(directory)));
        }
        params.push(format!("name={}", encode_component(name)));
        format!("{ws_base}/api/experimental/client-mcp?{}", params.join("&"))
    }

    /// Connects a client MCP WebSocket for `options.directory` and returns its handle and call receiver.
    pub fn connect_client_mcp(&self, options: ClientMcpOptions, waker: Waker) -> (ClientMcp, Receiver<McpCall>) {
        let (tx, rx) = channel();
        let mcp = self.connect_client_mcp_with(options, tx, waker);
        (mcp, rx)
    }

    /// Connects a client MCP WebSocket forwarding incoming `tools/call` requests into `calls`.
    pub fn connect_client_mcp_with(&self, options: ClientMcpOptions, calls: Sender<McpCall>, waker: Waker) -> ClientMcp {
        let shared = Arc::new(Shared {
            state: Mutex::new(InnerState { conn: Conn::Dialing, tools: options.tools.clone(), tools_gen: 0 }),
            cv: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let server = self.clone();
        // A page's own WebSocket, on a task of the page's event loop (`client_mcp_web.rs`).
        #[cfg(target_arch = "wasm32")]
        {
            web::start(server, options, calls, waker, worker_shared);
            return ClientMcp { shared, handle: None };
        }
        #[cfg(not(target_arch = "wasm32"))]
        let thread_name = format!("opencode-mcp-{}", options.name);
        #[cfg(not(target_arch = "wasm32"))]
        let handle = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || run_worker(server, options, calls, waker, worker_shared))
            .ok();
        #[cfg(not(target_arch = "wasm32"))]
        ClientMcp { shared, handle }
    }

    /// True when both [`Server`] handles point at the same base URL with the same auth credentials.
    pub fn same_target(&self, other: &Server) -> bool {
        self.base == other.base && self.auth == other.auth
    }

    /// Opaque `(base, auth_hash)` key suitable for deduplicating connections without exposing credentials.
    pub fn endpoint_key(&self) -> (String, u64) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        self.auth.hash(&mut h);
        (self.base.clone(), h.finish())
    }

    /// Performs a JSON `GET` with an explicit timeout instead of the default 15s read timeout.
    pub fn get_timeout(&self, path: &str, timeout: Duration) -> Result<Value, crate::wire_error::WireError> {
        Ok(self.get_json(path, None, timeout)?.0)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Moves the socket's state to `next` unless it was closed; false when it was. Wakes the app when
/// the status changed.
fn transition(shared: &Shared, next: impl FnOnce(&mut Conn) -> Option<Conn>, waker: &Waker) -> bool {
    let mut st = lock(&shared.state);
    if st.conn.closed() {
        return false;
    }
    let before = st.conn.status();
    if let Some(next) = next(&mut st.conn) {
        st.conn = next;
    }
    let changed = st.conn.status() != before;
    drop(st);
    if changed {
        waker();
    }
    true
}

/// `tools/list` was answered on the open socket: it's ready from now; if `set_tools` ran while
/// the reply went out, notify once so the client lists the newer tools too.
fn mark_ready(shared: &Shared, listed_gen: u64, waker: &Waker) {
    let mut st = lock(&shared.state);
    if st.conn.closed() {
        return;
    }
    let before = st.conn.status();
    let stale = st.tools_gen != listed_gen;
    if let Conn::Open { out, ready, list_changed, .. } = &mut st.conn {
        if ready.is_none() {
            *ready = Some(Instant::now());
        }
        if stale && *list_changed {
            out.send(Outbound::ToolsChanged);
        }
    }
    let changed = st.conn.status() != before;
    drop(st);
    if changed {
        waker();
    }
}

/// Waits out `delay` before the next dial; false when the socket was closed meanwhile.
#[cfg(not(target_arch = "wasm32"))]
fn wait_backoff(shared: &Shared, delay: Duration, unsupported: bool, waker: &Waker) -> bool {
    if !transition(shared, |_| Some(Conn::Waiting { unsupported }), waker) {
        return false;
    }
    let st = lock(&shared.state);
    let (st, _) = shared.cv.wait_timeout(st, delay).unwrap_or_else(|e| e.into_inner());
    !st.conn.closed()
}

#[cfg(not(target_arch = "wasm32"))]
enum ConnectOutcome {
    Connected(Box<WebSocket<MaybeTlsStream<TcpStream>>>),
    Unsupported,
    Failed,
}

#[cfg(not(target_arch = "wasm32"))]
fn dial(server: &Server, url: &str) -> ConnectOutcome {
    let Ok(mut req) = url.into_client_request() else { return ConnectOutcome::Failed };
    if let Some(auth) = &server.auth
        && let Ok(val) = HeaderValue::from_str(auth)
    {
        req.headers_mut().insert("Authorization", val);
    }
    let Some(addr) = resolve_ws_addr(url) else { return ConnectOutcome::Failed };
    let Ok(tcp) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) else { return ConnectOutcome::Failed };
    tcp.set_nodelay(true).ok();
    tcp.set_read_timeout(Some(CONNECT_TIMEOUT)).ok();
    tcp.set_write_timeout(Some(CONNECT_TIMEOUT)).ok();
    match tungstenite::client_tls(req, tcp) {
        Ok((ws, _)) => ConnectOutcome::Connected(Box::new(ws)),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp)))
            if matches!(resp.status().as_u16(), 404 | 405 | 501) =>
        {
            ConnectOutcome::Unsupported
        }
        Err(_) => ConnectOutcome::Failed,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_ws_addr(url: &str) -> Option<SocketAddr> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?']).next()?;
    let default_port = if scheme == "wss" || scheme == "https" { 443 } else { 80 };
    let host_port = if authority.starts_with('[') {
        if authority.contains("]:") { authority.to_owned() } else { format!("{authority}:{default_port}") }
    } else if authority.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        authority.replace("localhost", "127.0.0.1")
    } else {
        format!("{}:{default_port}", authority.replace("localhost", "127.0.0.1"))
    };
    if let Ok(a) = host_port.parse() {
        return Some(a);
    }
    host_port.to_socket_addrs().ok()?.next()
}

#[cfg(not(target_arch = "wasm32"))]
fn tcp_ref(ws: &WebSocket<MaybeTlsStream<TcpStream>>) -> &TcpStream {
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        #[allow(unreachable_patterns)]
        _ => unreachable!(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_worker(server: Server, options: ClientMcpOptions, calls: Sender<McpCall>, waker: Waker, shared: Arc<Shared>) {
    let url = server.client_mcp_url(&options.directory, &options.name);
    let mut delay = options.reconnect_min;

    loop {
        if !transition(&shared, |_| Some(Conn::Dialing), &waker) {
            return;
        }
        let outcome = dial(&server, &url);

        let mut ws = match outcome {
            ConnectOutcome::Connected(ws) => *ws,
            ConnectOutcome::Unsupported => {
                if !wait_backoff(&shared, options.unsupported_retry, true, &waker) {
                    return;
                }
                continue;
            }
            ConnectOutcome::Failed => {
                if !wait_backoff(&shared, delay, false, &waker) {
                    return;
                }
                delay = delay.saturating_mul(2).min(options.reconnect_max);
                continue;
            }
        };

        // The worker sleeps on the socket and on this poller's nudge: a reply or notice queued by
        // the app wakes it, as data arriving does; nothing else does.
        let Ok(wake) = polling::Poller::new() else {
            send_normal_close(&mut ws);
            if !wait_backoff(&shared, delay, false, &waker) {
                return;
            }
            continue;
        };
        let wake = Arc::new(wake);
        // SAFETY: the socket is removed from the poller (`delete` below) before `ws` closes it.
        #[allow(unsafe_code)]
        let added = unsafe { wake.add(tcp_ref(&ws), polling::Event::readable(0)) };
        if added.is_err() {
            send_normal_close(&mut ws);
            if !wait_backoff(&shared, delay, false, &waker) {
                return;
            }
            continue;
        }
        let (tx, out_rx) = channel::<Outbound>();
        let out = Outbox { tx, wake: wake.clone() };
        let tcp = tcp_ref(&ws).try_clone().ok();
        let open = out.clone();
        if !transition(&shared, move |_| Some(Conn::Open { tcp, out: open, ready: None, list_changed: true }), &waker) {
            wake.delete(tcp_ref(&ws)).ok();
            send_normal_close(&mut ws);
            return;
        }

        let mut initialized = false;
        let disconnected = serve_socket(&mut ws, &options, &calls, &waker, &shared, &out, &out_rx, &mut initialized);
        wake.delete(tcp_ref(&ws)).ok();
        if initialized {
            delay = options.reconnect_min;
        }

        let stopping = matches!(disconnected, LoopExit::ReceiverDropped) || !transition(&shared, |_| Some(Conn::Waiting { unsupported: false }), &waker);
        if stopping {
            lock(&shared.state).conn = Conn::Closed;
            send_normal_close(&mut ws);
            waker();
            return;
        }

        if !wait_backoff(&shared, delay, false, &waker) {
            return;
        }
        delay = delay.saturating_mul(2).min(options.reconnect_max);
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum LoopExit {
    SocketClosed,
    ReceiverDropped,
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
fn serve_socket(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    options: &ClientMcpOptions,
    calls: &Sender<McpCall>,
    waker: &Waker,
    shared: &Shared,
    out: &Outbox,
    out_rx: &Receiver<Outbound>,
    initialized: &mut bool,
) -> LoopExit {
    let mut in_flight: usize = 0;
    tcp_ref(ws).set_write_timeout(Some(CONNECT_TIMEOUT)).ok();
    tcp_ref(ws).set_read_timeout(Some(READ_TIMEOUT)).ok();
    let mut events = polling::Events::new();

    loop {
        if lock(&shared.state).conn.closed() || !drain_outbound(ws, out_rx, &mut in_flight) {
            return LoopExit::SocketClosed;
        }

        match ws.read() {
            Ok(Message::Text(text)) => {
                if !handle_frame(&mut |m| send_json(ws, m), text.as_str(), options, calls, waker, shared, out, &mut in_flight, initialized) {
                    return LoopExit::ReceiverDropped;
                }
            }
            Ok(Message::Binary(bytes)) => {
                if let Ok(text) = std::str::from_utf8(&bytes)
                    && !handle_frame(&mut |m| send_json(ws, m), text, options, calls, waker, shared, out, &mut in_flight, initialized)
                {
                    return LoopExit::ReceiverDropped;
                }
            }
            Ok(Message::Ping(_)) => {
                if ws.flush().is_err() {
                    return LoopExit::SocketClosed;
                }
            }
            Ok(Message::Pong(_) | Message::Frame(_)) => {}
            Ok(Message::Close(_)) => return LoopExit::SocketClosed,
            Err(tungstenite::Error::Io(ref err))
                if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) =>
            {
                // Everything read: sleep until the socket has data or the app queued a reply.
                // `modify` re-arms the one-shot interest; a nudge sent meanwhile is kept.
                events.clear();
                if out.wake.modify(tcp_ref(ws), polling::Event::readable(0)).is_err() || out.wake.wait(&mut events, None).is_err() {
                    return LoopExit::SocketClosed;
                }
            }
            Err(_) => return LoopExit::SocketClosed,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn drain_outbound(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, out_rx: &Receiver<Outbound>, in_flight: &mut usize) -> bool {
    let mut wrote = false;
    loop {
        let Ok(out) = out_rx.try_recv() else { break };
        let msg = outbound_message(out, in_flight);
        if ws.write(Message::Text(msg.to_string().into())).is_err() {
            return false;
        }
        wrote = true;
    }
    !wrote || ws.flush().is_ok()
}

/// What goes on the socket for an answer or a tool-list change; an answer settles a call.
fn outbound_message(out: Outbound, in_flight: &mut usize) -> Value {
    match out {
        Outbound::Reply { id, result } => {
            *in_flight = in_flight.saturating_sub(1);
            json!({ "jsonrpc": "2.0", "id": id, "result": result.to_json() })
        }
        Outbound::ToolsChanged => json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
    }
}

/// One JSON-RPC frame from the server: answers go out through `send` (false when the socket is
/// gone), a `tools/call` goes to the app. False when the app stopped taking calls.
#[allow(clippy::too_many_arguments)]
fn handle_frame(
    send: &mut dyn FnMut(&Value) -> bool,
    raw: &str,
    options: &ClientMcpOptions,
    calls: &Sender<McpCall>,
    waker: &Waker,
    shared: &Shared,
    out: &Outbox,
    in_flight: &mut usize,
    initialized: &mut bool,
) -> bool {
    let Ok(msg) = serde_json::from_str::<Value>(raw) else { return true };
    let Some(method) = msg.as_object().and_then(|o| o.get("method")).and_then(Value::as_str) else { return true };
    if method == "notifications/initialized" {
        // Match `connectClientMcp`: reset backoff only after the server confirms initialization.
        *initialized = true;
    }
    let Some(id) = msg.get("id").filter(|v| v.is_string() || v.is_number()).cloned() else { return true };
    let params = msg.get("params");

    match method {
        "initialize" => {
            let requested = params.and_then(|p| p.get("protocolVersion")).and_then(Value::as_str);
            let supports = requested.is_some_and(|v| v >= "2024-11-05");
            // 2024-11-05..2025-11-25 deliver `notifications/tools/list_changed` on the socket;
            // 2026-07-28+ moved list-changed onto `subscriptions/listen`, so cap at 2025-11-25.
            let proto = match requested {
                Some(v) if ("2024-11-05".."2026-07-28").contains(&v) => v,
                _ => PROTOCOL_VERSION,
            };
            let mut result = json!({
                "protocolVersion": proto,
                "capabilities": { "tools": { "listChanged": true } },
                "serverInfo": { "name": options.name, "version": options.version },
            });
            if let Some(inst) = &options.instructions {
                result["instructions"] = Value::String(inst.clone());
            }
            if send(&json!({ "jsonrpc": "2.0", "id": id, "result": result })) {
                transition(shared, |conn| {
                    if let Conn::Open { list_changed, .. } = conn {
                        *list_changed = supports;
                    }
                    None
                }, waker);
            }
        }
        "ping" => {
            send(&json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
        }
        "tools/list" => {
            let (listed_gen, tools_json) = {
                let st = lock(&shared.state);
                (st.tools_gen, st.tools.iter().map(tool_descriptor_json).collect::<Vec<Value>>())
            };
            if send(&json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools_json } })) {
                mark_ready(shared, listed_gen, waker);
            }
        }
        "tools/call" => {
            let tool_name = params.and_then(|p| p.get("name")).and_then(Value::as_str);
            let known = tool_name.is_some_and(|name| lock(&shared.state).tools.iter().any(|t| t.name == name));
            let Some(name) = tool_name.filter(|_| known) else {
                let raw_name = params.and_then(|p| p.get("name")).map(display_param_name).unwrap_or_else(|| "undefined".into());
                send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": format!("Unknown tool: {raw_name}") } }));
                return true;
            };
            let arguments = params.and_then(|p| p.get("arguments")).filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
            let session_id = params
                .and_then(|p| p.get("_meta"))
                .and_then(|m| m.get(SESSION_ID_META_KEY))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let call = McpCall {
                directory: options.directory.clone(),
                name: name.to_owned(),
                arguments,
                session_id,
                reply: Some((id, out.clone())),
            };
            if calls.send(call).is_err() {
                return false;
            }
            *in_flight += 1;
            waker();
        }
        _ => {
            send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Method not found: {method}") } }));
        }
    }
    true
}

fn display_param_name(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn tool_descriptor_json(tool: &McpTool) -> Value {
    let mut schema = match &tool.input_schema {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    schema.entry("type".to_owned()).or_insert_with(|| Value::String("object".into()));
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": Value::Object(schema),
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn send_json(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, msg: &Value) -> bool {
    ws.send(Message::Text(msg.to_string().into())).is_ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn send_normal_close(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) {
    tcp_ref(ws).set_write_timeout(Some(Duration::from_millis(250))).ok();
    ws.close(Some(CloseFrame { code: CloseCode::Normal, reason: "".into() })).ok();
    ws.flush().ok();
}

