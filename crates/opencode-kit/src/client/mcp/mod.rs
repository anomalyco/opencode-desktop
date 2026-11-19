//! Client MCP over WebSocket (`GET /api/experimental/client-mcp?location[directory]=…&name=…`,
//! `packages/client/src/mcp.ts` and `packages/server/src/handlers/mcp.ts`).
//!
//! A client app opens a WebSocket to the OpenCode server and serves MCP JSON-RPC 2.0 (`initialize`,
//! `ping`, `tools/list`, `tools/call`) over text frames while the OpenCode server acts as the MCP
//! client. Reconnects with exponential backoff (`500 ms`..`10 s`, reset on
//! `notifications/initialized`) and backs off quietly when the server lacks the experimental route.

use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rune::Waker;
use rune::time::Instant;
use serde_json::{Value, json};

use crate::client::{Server, encode_component};

#[cfg(not(target_arch = "wasm32"))]
mod native;
mod protocol;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use protocol::{handle_frame, outbound_message};

pub const PROTOCOL_VERSION: &str = "2025-11-25";
pub const SESSION_ID_META_KEY: &str = "ai.opencode/sessionID";
pub const RECONNECT_MIN: Duration = Duration::from_millis(500);
pub const RECONNECT_MAX: Duration = Duration::from_secs(10);
pub const UNSUPPORTED_RETRY: Duration = Duration::from_secs(60);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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

    pub(super) fn to_json(&self) -> Value {
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

pub(super) enum Outbound {
    Reply { id: Value, result: McpResult },
    ToolsChanged,
}

/// Where replies and notices for the open socket go: the worker's queue and a nudge that wakes the
/// worker from its wait on the socket (natively a poller, in a page the task's bell), so it sleeps
/// until there's something to do.
#[derive(Clone)]
pub(super) struct Outbox {
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

pub(super) struct Shared {
    state: Mutex<InnerState>,
    cv: Condvar,
}

pub(super) struct InnerState {
    conn: Conn,
    tools: Vec<McpTool>,
    tools_gen: u64,
}

/// The socket's life, one state at a time (`ClientMcpStatus` reads it). Official
/// `packages/client/src/mcp.ts` keeps the same lifecycle in `disposed`, `delay`, `timer` and
/// `socket`.
pub(super) enum Conn {
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

    pub(super) fn closed(&self) -> bool { matches!(self, Conn::Closed) }
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
            .spawn(move || native::run_worker(server, options, calls, waker, worker_shared))
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

pub(super) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Moves the socket's state to `next` unless it was closed; false when it was. Wakes the app when
/// the status changed.
pub(super) fn transition(shared: &Shared, next: impl FnOnce(&mut Conn) -> Option<Conn>, waker: &Waker) -> bool {
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
pub(super) fn mark_ready(shared: &Shared, listed_gen: u64, waker: &Waker) {
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
