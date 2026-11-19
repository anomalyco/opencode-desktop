//! Client MCP from a page. A page's `WebSocket` can't carry the `Authorization` header, so it
//! first asks for a one-use ticket (`POST /api/experimental/client-mcp/token`, with the header
//! and `x-opencode-ticket: 1`, as official `packages/client` does from a browser) and opens the
//! socket with it; then it serves the same JSON-RPC as natively (`handle_frame`), on a task of the
//! page's event loop that waits on the socket and never on the main thread.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use rune::Waker;
use serde_json::Value;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;

use super::{ClientMcpOptions, Conn, McpCall, Outbox, Shared, handle_frame, lock, outbound_message, transition};
use crate::client::{Server, encode_component};

/// WebSocket lifecycle from the page's event listeners (`onopen`, `onclose`, `onerror`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SocketState {
    /// Dialed; waiting for `onopen` (or a connect failure via `onerror`/`onclose`).
    #[default]
    Connecting,
    /// `onopen` fired and the socket is live.
    Open,
    /// Closed before `onopen` ever fired.
    Failed,
    /// Closed after `onopen` fired; any buffered `frames` still drain before `run` exits.
    Closed,
}

enum SocketEvent {
    Open,
    Close,
    Message(String),
}

/// What the socket's listeners have handed the task since it last looked.
#[derive(Default)]
struct Events {
    state: SocketState,
    frames: VecDeque<String>,
    /// Resolves the task's current wait.
    wake: Option<js_sys::Function>,
}

impl Events {
    fn step(&mut self, event: SocketEvent) -> Option<js_sys::Function> {
        match event {
            SocketEvent::Open => {
                if self.state == SocketState::Connecting {
                    self.state = SocketState::Open;
                }
            }
            SocketEvent::Close => {
                self.state = match self.state {
                    SocketState::Connecting | SocketState::Failed => SocketState::Failed,
                    SocketState::Open | SocketState::Closed => SocketState::Closed,
                };
            }
            SocketEvent::Message(frame) => {
                self.frames.push_back(frame);
            }
        }
        self.wake.take()
    }
}

fn dispatch(events: &Rc<RefCell<Events>>, event: SocketEvent) {
    let wake = events.borrow_mut().step(event);
    if let Some(wake) = wake {
        drop(wake.call0(&JsValue::NULL));
    }
}

thread_local! {
    /// Each open socket's events, by its outbox's bell: a reply, a changed tool list or a close
    /// from the app wakes the task, which otherwise sleeps until the socket hands it something.
    static BELLS: RefCell<HashMap<u64, Weak<RefCell<Events>>>> = RefCell::default();
    static NEXT_BELL: Cell<u64> = const { Cell::new(0) };
}

/// Wakes the task serving the socket behind ell, if it's waiting.
pub(super) fn ring(bell: u64) {
    let Some(events) = BELLS.with(|b| b.borrow().get(&bell).and_then(Weak::upgrade)) else { return };
    let wake = events.borrow_mut().wake.take();
    if let Some(wake) = wake {
        let _ = wake.call0(&JsValue::NULL);
    }
}

enum Ticket {
    Got(String),
    /// The server has no client MCP route.
    Unsupported,
    Failed,
}

/// Dials, serves and redials with backoff until the handle closes or the app stops taking calls.
pub(super) fn start(server: Server, options: ClientMcpOptions, calls: Sender<McpCall>, waker: Waker, shared: Arc<Shared>) {
    wasm_bindgen_futures::spawn_local(async move {
        let mut delay = options.reconnect_min;
        loop {
            if !transition(&shared, |_| Some(Conn::Dialing), &waker) {
                return;
            }
            let ticket = match ticket(&server, &options).await {
                Ticket::Got(ticket) => ticket,
                Ticket::Unsupported => {
                    if !backoff(&shared, options.unsupported_retry, true, &waker).await {
                        return;
                    }
                    continue;
                }
                Ticket::Failed => {
                    if !backoff(&shared, delay, false, &waker).await {
                        return;
                    }
                    delay = delay.saturating_mul(2).min(options.reconnect_max);
                    continue;
                }
            };
            let url = format!("{}&ticket={}", server.client_mcp_url(&options.directory, &options.name), encode_component(&ticket));
            let (initialized, stop) = serve(&url, &options, &calls, &waker, &shared).await;
            if stop {
                lock(&shared.state).conn = Conn::Closed;
                waker();
                return;
            }
            if initialized {
                delay = options.reconnect_min;
            }
            if !backoff(&shared, delay, false, &waker).await {
                return;
            }
            delay = delay.saturating_mul(2).min(options.reconnect_max);
        }
    });
}

/// One socket's life: `(initialized, stop)`, `stop` when the handle closed or the app stopped
/// taking calls.
async fn serve(url: &str, options: &ClientMcpOptions, calls: &Sender<McpCall>, waker: &Waker, shared: &Shared) -> (bool, bool) {
    let Ok(ws) = web_sys::WebSocket::new(url) else { return (false, false) };
    let events = Rc::new(RefCell::new(Events::default()));
    let on_open = { let events = events.clone(); Closure::<dyn FnMut()>::new(move || dispatch(&events, SocketEvent::Open)) };
    let on_close = { let events = events.clone(); Closure::<dyn FnMut()>::new(move || dispatch(&events, SocketEvent::Close)) };
    let on_message = {
        let events = events.clone();
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |m: web_sys::MessageEvent| {
            if let Some(text) = m.data().as_string() {
                dispatch(&events, SocketEvent::Message(text));
            }
        })
    };
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    ws.set_onerror(Some(on_close.as_ref().unchecked_ref()));
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    let bell = NEXT_BELL.with(|n| n.replace(n.get() + 1));
    BELLS.with(|b| b.borrow_mut().insert(bell, Rc::downgrade(&events)));
    let result = run(&ws, &events, bell, options, calls, waker, shared).await;
    BELLS.with(|b| b.borrow_mut().remove(&bell));
    ws.set_onopen(None);
    ws.set_onclose(None);
    ws.set_onerror(None);
    ws.set_onmessage(None);
    if result.1 {
        let _ = ws.close_with_code(1000);
    }
    result
}

async fn run(ws: &web_sys::WebSocket, events: &Rc<RefCell<Events>>, bell: u64, options: &ClientMcpOptions, calls: &Sender<McpCall>, waker: &Waker, shared: &Shared) -> (bool, bool) {
    let dialed = rune::time::Instant::now();
    loop {
        match events.borrow().state {
            SocketState::Open | SocketState::Closed => break,
            SocketState::Failed => return (false, false),
            SocketState::Connecting => {
                if dialed.elapsed() > super::CONNECT_TIMEOUT {
                    return (false, false);
                }
            }
        }
        wait(events, Some(1000)).await;
    }
    let (tx, out_rx) = channel();
    let out_tx = Outbox { tx, bell };
    let out = out_tx.clone();
    if !transition(shared, move |_| Some(Conn::Open { tcp: None, out, ready: None, list_changed: true }), waker) {
        return (false, true);
    }
    let (mut in_flight, mut initialized) = (0usize, false);
    loop {
        if lock(&shared.state).conn.closed() {
            return (initialized, true);
        }
        while let Ok(reply) = out_rx.try_recv() {
            if ws.send_with_str(&outbound_message(reply, &mut in_flight).to_string()).is_err() {
                return (initialized, false);
            }
        }
        let frames: Vec<String> = events.borrow_mut().frames.drain(..).collect();
        for raw in frames {
            let mut send = |m: &Value| ws.send_with_str(&m.to_string()).is_ok();
            if !handle_frame(&mut send, &raw, options, calls, waker, shared, &out_tx, &mut in_flight, &mut initialized) {
                return (initialized, true);
            }
        }
        if matches!(events.borrow().state, SocketState::Closed | SocketState::Failed) {
            return (initialized, false);
        }
        // Idle, nothing but the socket or the app's outbox (its bell) wakes the task: no timer.
        wait(events, None).await;
    }
}

/// Until the socket or the outbox hands over something, or `ms` passes.
async fn wait(events: &Rc<RefCell<Events>>, ms: Option<i32>) {
    if !events.borrow().frames.is_empty() {
        return;
    }
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let (Some(window), Some(ms)) = (web_sys::window(), ms) {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
        events.borrow_mut().wake = Some(resolve);
    });
    let _ = JsFuture::from(promise).await;
}

/// Waits out `delay` before the next dial; false when the handle closed meanwhile.
async fn backoff(shared: &Shared, delay: Duration, unsupported: bool, waker: &Waker) -> bool {
    if !transition(shared, |_| Some(Conn::Waiting { unsupported }), waker) {
        return false;
    }
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, delay.as_millis().min(i32::MAX as u128) as i32);
        }
    });
    let _ = JsFuture::from(promise).await;
    !lock(&shared.state).conn.closed()
}

/// A one-use ticket for the socket, from `POST /api/experimental/client-mcp/token`.
async fn ticket(server: &Server, options: &ClientMcpOptions) -> Ticket {
    let Some(window) = web_sys::window() else { return Ticket::Failed };
    let (base, query) = server.base.split_once('?').unwrap_or((server.base.as_str(), ""));
    let mut params: Vec<String> = Vec::new();
    if !query.is_empty() {
        params.push(query.to_owned());
    }
    if !options.directory.is_empty() {
        params.push(format!("location%5Bdirectory%5D={}", encode_component(&options.directory)));
    }
    params.push(format!("name={}", encode_component(&options.name)));
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    let Ok(headers) = web_sys::Headers::new() else { return Ticket::Failed };
    let _ = headers.set("x-opencode-ticket", "1");
    if let Some(auth) = &server.auth {
        let _ = headers.set("Authorization", auth);
    }
    init.set_headers(&headers);
    let url = format!("{}/api/experimental/client-mcp/token?{}", base.trim_end_matches('/'), params.join("&"));
    let Ok(request) = web_sys::Request::new_with_str_and_init(&url, &init) else { return Ticket::Failed };
    let Ok(response) = JsFuture::from(window.fetch_with_request(&request)).await else { return Ticket::Failed };
    let Ok(response) = response.dyn_into::<web_sys::Response>() else { return Ticket::Failed };
    if matches!(response.status(), 404 | 405 | 501) {
        return Ticket::Unsupported;
    }
    if !response.ok() {
        return Ticket::Failed;
    }
    let Ok(text) = response.text() else { return Ticket::Failed };
    let Ok(text) = JsFuture::from(text).await else { return Ticket::Failed };
    let body: Value = text.as_string().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    match body["data"]["ticket"].as_str().or_else(|| body["ticket"].as_str()) {
        Some(ticket) => Ticket::Got(ticket.to_owned()),
        None => Ticket::Failed,
    }
}
