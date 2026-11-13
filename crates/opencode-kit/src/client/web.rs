//! The connection in a browser. Requests are synchronous `XMLHttpRequest`s (through
//! `ureq_web`), so the same calls work in the same places as natively; they run from `rune::task`'s main-thread tasks, between
//! frames, never inside one. The event stream is a streamed `fetch`, read asynchronously, so a
//! quiet stream costs the page nothing. A page may call a server only when the server's CORS
//! rules allow its origin (a local page on `http://localhost:*` or `http://127.0.0.1:*` always).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use rune::Waker;
use rune::time::Instant;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::{Incoming, Server};

impl Server {
    /// A browser can't open a socket to ask; the first request says.
    pub fn reachable(&self) -> bool {
        true
    }

    /// Starts GETs of `paths` now, all at once, for this server's `get`s from task work to take
    /// up (`ureq_web::start_get`): a page's reads otherwise go one after another.
    pub fn start_gets(&self, paths: &[&str]) {
        let auth = self.auth.as_deref().map(|a| ("Authorization", a));
        for path in paths {
            ureq_web::start_get(&format!("{}{path}", self.base), auth.as_slice());
        }
    }

    /// Whether every GET of `paths` was started and its answer is in, so `get`s of them return
    /// at once, outside task work too (`ureq_web::started_in`).
    pub fn gets_in(&self, paths: &[&str]) -> bool {
        let auth = self.auth.as_deref().map(|a| ("Authorization", a));
        let urls: Vec<String> = paths.iter().map(|path| format!("{}{path}", self.base)).collect();
        ureq_web::started_in(&urls, auth.as_slice())
    }

    /// Drops what [`Server::start_gets`] began that no `get` took up.
    pub fn forget_gets(&self) {
        ureq_web::forget_started();
    }
}

static RETRY: AtomicU64 = AtomicU64::new(0);

pub(super) fn wake_reconnects() {
    RETRY.fetch_add(1, Ordering::Relaxed);
}

/// The event stream, reopened with backoff after each drop, until `inbox` closes: a task on the
/// page's event loop that waits on the network, never on the main thread.
pub(super) fn read_events(server: &Server, idle: Duration, inbox_tx: &Sender<Incoming>, wake: &Waker) {
    let (server, inbox_tx, wake) = (server.clone(), inbox_tx.clone(), wake.clone());
    wasm_bindgen_futures::spawn_local(async move {
        let mut resumed = false;
        let mut backoff = 250u64;
        loop {
            let mut opened = false;
            let generation = RETRY.load(Ordering::Relaxed);
            match events(&server, idle, &inbox_tx, &wake, resumed, &mut opened).await {
                Ok(()) => return,
                Err(e) => {
                    if inbox_tx.send(Incoming::Status(e)).is_err() {
                        return;
                    }
                    wake();
                }
            }
            if opened {
                backoff = 250;
            }
            resumed = true;
            // Wait out the backoff, or less when a reconnect is asked for.
            let until = Instant::now() + Duration::from_millis(backoff);
            while Instant::now() < until && RETRY.load(Ordering::Relaxed) == generation {
                sleep(Duration::from_millis(100)).await;
            }
            backoff = (backoff * 2).min(3000);
        }
    });
}

/// One open of `/api/event`: `Ok` when the inbox closed, else why the stream ended.
async fn events(server: &Server, idle: Duration, inbox: &Sender<Incoming>, wake: &Waker, resumed: bool, opened: &mut bool) -> Result<(), String> {
    let window = web_sys::window().ok_or("events: no window")?;
    let abort = web_sys::AbortController::new().map_err(|_| "events: no AbortController")?;
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    init.set_signal(Some(&abort.signal()));
    let headers = web_sys::Headers::new().map_err(|_| "events: no Headers")?;
    let _ = headers.set("Accept", "text/event-stream");
    if let Some(a) = &server.auth {
        let _ = headers.set("Authorization", a);
    }
    init.set_headers(&headers);
    let request = web_sys::Request::new_with_str_and_init(&format!("{}/api/event", server.base), &init).map_err(|_| "events: bad request")?;
    let response = with_idle(window.fetch_with_request(&request), idle, &abort)
        .await
        .map_err(|e| format!("events: {e}"))?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| "events: not a response")?;
    if !response.ok() {
        return Err(format!("events: http status: {}", response.status()));
    }
    let content_type = response.headers().get("content-type").ok().flatten().unwrap_or_default();
    if !content_type.trim_start().to_ascii_lowercase().starts_with("text/event-stream") {
        let what = super::http::kind_of(&content_type, false);
        return Err(format!("events: {}", crate::wire_error::WireKind::NotOpenCode { status: response.status(), what }));
    }
    let reader: web_sys::ReadableStreamDefaultReader =
        response.body().ok_or("events: no body")?.get_reader().dyn_into().map_err(|_| "events: no reader")?;
    // Connected only once OpenCode's first event (`server.connected`) is in.
    let mut connected = false;
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let chunk = with_idle(reader.read(), idle, &abort).await.map_err(|e| format!("events: {e}"))?;
        if js_sys::Reflect::get(&chunk, &"done".into()).ok().and_then(|d| d.as_bool()).unwrap_or(true) {
            return Err("events: stream ended".into());
        }
        let bytes = js_sys::Reflect::get(&chunk, &"value".into()).map(|v| js_sys::Uint8Array::new(&v).to_vec()).unwrap_or_default();
        pending.extend_from_slice(&bytes);
        let mut woke = false;
        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(message) = super::event_line(line.trim_end_matches(['\n', '\r'])) else { continue };
            if !connected {
                if !matches!(&message, Incoming::Event { ty, .. } if ty == "server.connected") {
                    let what = "an event stream that didn't start with server.connected".to_owned();
                    return Err(format!("events: {}", crate::wire_error::WireKind::NotOpenCode { status: 200, what }));
                }
                connected = true;
                *opened = true;
                if resumed && inbox.send(Incoming::Reconnected).is_err() {
                    return Ok(());
                }
            }
            if inbox.send(message).is_err() {
                return Ok(());
            }
            woke = true;
        }
        if woke {
            wake();
        }
    }
}

/// `future`, or a timeout error (aborting the request) after `idle` without an answer.
async fn with_idle(promise: js_sys::Promise, idle: Duration, abort: &web_sys::AbortController) -> Result<JsValue, String> {
    let timer = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_1(&resolve, idle.as_millis() as i32, &JsValue::from_str("idle"));
        }
    });
    let raced = js_sys::Promise::race(&js_sys::Array::of2(&promise, &timer));
    match JsFuture::from(raced).await {
        Ok(v) if v.as_string().as_deref() == Some("idle") => {
            abort.abort();
            Err("timeout: no data".into())
        }
        Ok(v) => Ok(v),
        Err(e) => {
            abort.abort();
            Err(e.as_string().or_else(|| js_sys::Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string())).unwrap_or_else(|| "connection failed".into()))
        }
    }
}

async fn sleep(wait: Duration) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, wait.as_millis() as i32);
        }
    });
    let _ = JsFuture::from(promise).await;
}
