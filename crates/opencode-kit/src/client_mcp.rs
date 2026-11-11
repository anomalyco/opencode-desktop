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
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
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

pub const PROTOCOL_VERSION: &str = "2025-11-25";
pub const SESSION_ID_META_KEY: &str = "ai.opencode/sessionID";
pub const RECONNECT_MIN: Duration = Duration::from_millis(500);
pub const RECONNECT_MAX: Duration = Duration::from_secs(10);
pub const UNSUPPORTED_RETRY: Duration = Duration::from_secs(60);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_READ_TIMEOUT: Duration = Duration::from_millis(200);
const BUSY_READ_TIMEOUT: Duration = Duration::from_millis(5);

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

/// An incoming `tools/call` invocation delivered to the host application.
///
/// Call [`McpCall::respond`] once finished (immediately or asynchronously across frames). If dropped
/// without responding, a cancellation error is sent automatically so the server call never stalls.
pub struct McpCall {
    pub directory: String,
    pub name: String,
    pub arguments: Value,
    pub session_id: Option<String>,
    reply: Option<(Value, Sender<Outbound>)>,
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
            tx.send(Outbound::Reply { id, result }).ok();
        }
    }
}

impl Drop for McpCall {
    fn drop(&mut self) {
        if let Some((id, tx)) = self.reply.take() {
            tx.send(Outbound::Reply { id, result: McpResult::error("Tool call cancelled") }).ok();
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
}

/// The socket's life, one state at a time (`ClientMcpStatus` reads it). Official
/// `packages/client/src/mcp.ts` keeps the same lifecycle in `disposed`, `delay`, `timer` and
/// `socket`.
enum Conn {
    /// The worker is opening a socket.
    Dialing,
    /// Waiting out a backoff before the next dial; `unsupported`: the server lacks the route.
    Waiting { unsupported: bool },
    /// The socket is open; `ready` once `tools/list` was answered on it, since then.
    Open { tcp: Option<TcpStream>, out: Sender<Outbound>, ready: Option<Instant> },
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

    /// Replaces the advertised tool list and notifies the server (`notifications/tools/list_changed`).
    pub fn set_tools(&self, tools: Vec<McpTool>) {
        let mut st = lock(&self.shared.state);
        st.tools = tools;
        if let Conn::Open { out, .. } = &st.conn {
            out.send(Outbound::ToolsChanged).ok();
        }
    }

    /// Closes the WebSocket cleanly (`1000 Normal`) and stops reconnecting without blocking on slow dials.
    pub fn close(&mut self) {
        let can_join = {
            let mut st = lock(&self.shared.state);
            // A dial in flight can take its whole timeout: don't wait for it.
            let dialing = matches!(st.conn, Conn::Dialing);
            if let Conn::Open { tcp: Some(tcp), .. } = std::mem::replace(&mut st.conn, Conn::Closed) {
                tcp.shutdown(Shutdown::Read).ok();
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
            state: Mutex::new(InnerState { conn: Conn::Dialing, tools: options.tools.clone() }),
            cv: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let server = self.clone();
        // TODO(web): a WebSocket from the page. Until then the page says the route is missing.
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (worker_shared, server, calls, waker);
            lock(&shared.state).conn = Conn::Waiting { unsupported: true };
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
    pub fn get_timeout(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        let mut req = Self::agent(Some(timeout)).get(format!("{}{path}", self.base));
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let mut resp = req.call().map_err(|e| format!("GET {path}: {e}"))?;
        let text = resp
            .body_mut()
            .with_config()
            .limit(4 << 20)
            .read_to_string()
            .map_err(|e| format!("GET {path}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("GET {path}: {e}"))
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

/// `tools/list` was answered on the open socket: it's ready from now.
fn mark_ready(shared: &Shared, waker: &Waker) {
    transition(shared, |conn| {
        if let Conn::Open { ready: ready @ None, .. } = conn {
            *ready = Some(Instant::now());
        }
        None
    }, waker);
}

/// Waits out `delay` before the next dial; false when the socket was closed meanwhile.
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

        let (out_tx, out_rx) = channel::<Outbound>();
        let tcp = tcp_ref(&ws).try_clone().ok();
        let out = out_tx.clone();
        if !transition(&shared, move |_| Some(Conn::Open { tcp, out, ready: None }), &waker) {
            send_normal_close(&mut ws);
            return;
        }

        let mut initialized = false;
        let disconnected = serve_socket(&mut ws, &options, &calls, &waker, &shared, &out_tx, &out_rx, &mut initialized);
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
    out_tx: &Sender<Outbound>,
    out_rx: &Receiver<Outbound>,
    initialized: &mut bool,
) -> LoopExit {
    let mut in_flight: usize = 0;
    let mut current_timeout = Duration::ZERO;
    tcp_ref(ws).set_write_timeout(Some(CONNECT_TIMEOUT)).ok();

    loop {
        if lock(&shared.state).conn.closed() || !drain_outbound(ws, out_rx, &mut in_flight) {
            return LoopExit::SocketClosed;
        }

        let target_timeout = if in_flight > 0 { BUSY_READ_TIMEOUT } else { IDLE_READ_TIMEOUT };
        if current_timeout != target_timeout {
            tcp_ref(ws).set_read_timeout(Some(target_timeout)).ok();
            current_timeout = target_timeout;
        }

        match ws.read() {
            Ok(Message::Text(text)) => {
                if !handle_frame(ws, text.as_str(), options, calls, waker, shared, out_tx, &mut in_flight, initialized) {
                    return LoopExit::ReceiverDropped;
                }
            }
            Ok(Message::Binary(bytes)) => {
                if let Ok(text) = std::str::from_utf8(&bytes)
                    && !handle_frame(ws, text, options, calls, waker, shared, out_tx, &mut in_flight, initialized)
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
                if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(_) => return LoopExit::SocketClosed,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn drain_outbound(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, out_rx: &Receiver<Outbound>, in_flight: &mut usize) -> bool {
    let mut wrote = false;
    loop {
        let msg = match out_rx.try_recv() {
            Ok(Outbound::Reply { id, result }) => {
                *in_flight = in_flight.saturating_sub(1);
                json!({ "jsonrpc": "2.0", "id": id, "result": result.to_json() })
            }
            Ok(Outbound::ToolsChanged) => json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        };
        if ws.write(Message::Text(msg.to_string().into())).is_err() {
            return false;
        }
        wrote = true;
    }
    !wrote || ws.flush().is_ok()
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
fn handle_frame(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    raw: &str,
    options: &ClientMcpOptions,
    calls: &Sender<McpCall>,
    waker: &Waker,
    shared: &Shared,
    out_tx: &Sender<Outbound>,
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
            let proto = params.and_then(|p| p.get("protocolVersion")).and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
            let mut result = json!({
                "protocolVersion": proto,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": options.name, "version": options.version },
            });
            if let Some(inst) = &options.instructions {
                result["instructions"] = Value::String(inst.clone());
            }
            send_json(ws, &json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
        "ping" => {
            send_json(ws, &json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
        }
        "tools/list" => {
            let tools_json: Vec<Value> = lock(&shared.state).tools.iter().map(tool_descriptor_json).collect();
            if send_json(ws, &json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools_json } })) {
                mark_ready(shared, waker);
            }
        }
        "tools/call" => {
            let tool_name = params.and_then(|p| p.get("name")).and_then(Value::as_str);
            let known = tool_name.is_some_and(|name| lock(&shared.state).tools.iter().any(|t| t.name == name));
            let Some(name) = tool_name.filter(|_| known) else {
                let raw_name = params.and_then(|p| p.get("name")).map(display_param_name).unwrap_or_else(|| "undefined".into());
                send_json(
                    ws,
                    &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": format!("Unknown tool: {raw_name}") } }),
                );
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
                reply: Some((id, out_tx.clone())),
            };
            if calls.send(call).is_err() {
                return false;
            }
            *in_flight += 1;
            waker();
        }
        _ => {
            send_json(
                ws,
                &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Method not found: {method}") } }),
            );
        }
    }
    true
}

#[cfg(not(target_arch = "wasm32"))]
fn display_param_name(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
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

