//! OpenTelemetry traces for a local collector (Motel): OTLP/HTTP JSON to a loopback address,
//! posted by one background thread. The caller's thread only pushes finished spans onto a
//! channel; when nothing listens the batch is dropped. Callers put ids, counts, sizes and
//! timings on spans, never user text.
//!
//! An app calls [`start`] once with its service name and where to send (none: off), then
//! makes spans. [`capture`] collects one thread's spans instead of sending them, for tests.
use std::cell::RefCell;
use std::hash::{BuildHasher, Hasher};
use std::net::SocketAddr;
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}, mpsc::{Sender, channel}};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Motel's OTLP/HTTP receiver.
pub const MOTEL: &str = "127.0.0.1:27686";
const PATH: &str = "/v1/traces";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ctx { trace: u128, span: u64 }

pub struct Span {
    ctx: Ctx,
    parent: Option<u64>,
    name: &'static str,
    pub start: Instant,
    attrs: Vec<(&'static str, Value)>,
}

impl Span {
    pub fn root(name: &'static str) -> Self {
        Self { ctx: Ctx { trace: (u128::from(id()) << 64) | u128::from(id()), span: id() }, parent: None, name, start: Instant::now(), attrs: vec![] }
    }

    pub fn child(parent: Ctx, name: &'static str) -> Self {
        Self { ctx: Ctx { trace: parent.trace, span: id() }, parent: Some(parent.span), name, start: Instant::now(), attrs: vec![] }
    }

    pub fn ctx(&self) -> Ctx { self.ctx }

    pub fn at(mut self, start: Instant) -> Self {
        self.start = start;
        self
    }

    pub fn attr(mut self, key: &'static str, value: impl Into<Value>) -> Self {
        self.attrs.push((key, value.into()));
        self
    }

    pub fn end_at(self, end: Instant) {
        if !on() { return; }
        let attrs: Vec<Value> = self.attrs.into_iter().map(|(k, v)| json!({"key": k, "value": any(v)})).collect();
        let mut span = json!({
            "traceId": format!("{:032x}", self.ctx.trace), "spanId": format!("{:016x}", self.ctx.span),
            "name": self.name, "kind": 1,
            "startTimeUnixNano": unix_nanos(self.start).to_string(), "endTimeUnixNano": unix_nanos(end.max(self.start)).to_string(),
            "attributes": attrs,
        });
        if let Some(p) = self.parent { span["parentSpanId"] = format!("{p:016x}").into(); }
        if CAPTURE.with(|c| c.borrow_mut().as_mut().map(|c| c.push(span.clone())).is_some()) { return; }
        if let Some(out) = exporter() { let _ = out.send(span); }
    }
}

/// A finished interval as a child of `parent`.
pub fn record(parent: Ctx, name: &'static str, (start, end): (Instant, Instant), attrs: Vec<(&'static str, Value)>) {
    let mut s = Span::child(parent, name).at(start);
    s.attrs = attrs;
    s.end_at(end);
}

pub fn ms(d: Duration) -> f64 { (d.as_secs_f64() * 1e6).round() / 1e3 }

fn any(v: Value) -> Value {
    match v {
        Value::Bool(b) => json!({"boolValue": b}),
        Value::Number(n) if n.is_i64() || n.is_u64() => json!({"intValue": n.to_string()}),
        Value::Number(n) => json!({"doubleValue": n.as_f64()}),
        Value::String(s) => json!({"stringValue": s}),
        other => json!({"stringValue": other.to_string()}),
    }
}

fn id() -> u64 {
    static N: AtomicU64 = AtomicU64::new(0);
    static SEED: OnceLock<std::collections::hash_map::RandomState> = OnceLock::new();
    let mut h = SEED.get_or_init(Default::default).build_hasher();
    h.write_u64(N.fetch_add(1, Ordering::Relaxed));
    h.finish().max(1)
}

fn unix_nanos(t: Instant) -> u128 {
    static ANCHOR: OnceLock<(Instant, u128)> = OnceLock::new();
    let (at, unix) = *ANCHOR.get_or_init(|| (Instant::now(), SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    if t >= at { unix + (t - at).as_nanos() } else { unix.saturating_sub((at - t).as_nanos()) }
}

thread_local! {
    /// Spans this thread ends, instead of sending them (`capture`).
    static CAPTURE: RefCell<Option<Vec<Value>>> = const { RefCell::new(None) };
}

/// Runs `f` with this thread's spans collected instead of sent, and returns them.
pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<Value>) {
    CAPTURE.with(|c| *c.borrow_mut() = Some(vec![]));
    let r = f();
    (r, CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default()))
}

/// Whether spans go anywhere: an exporter started with an address, or a `capture` running.
pub fn on() -> bool {
    CAPTURE.with(|c| c.borrow().is_some()) || exporter().is_some()
}

/// Where a setting sends: `1` is Motel, otherwise `host:port` or `http://host:port[/v1/traces]`
/// on loopback. Anything else (`0`, empty, another host) is off.
pub fn endpoint(setting: &str) -> Option<SocketAddr> {
    let v = setting.trim();
    if v == "1" { return MOTEL.parse().ok(); }
    let rest = v.strip_prefix("http://").unwrap_or(v);
    let host = rest.split('/').next()?.replace("localhost", "127.0.0.1");
    host.parse::<SocketAddr>().ok().filter(|a| a.ip().is_loopback())
}

struct Exporter { service: &'static str, version: &'static str, to: Option<SocketAddr> }

static CONFIG: OnceLock<Exporter> = OnceLock::new();

/// Names the service and where its spans go (`None`: nowhere). The first call wins.
pub fn start(service: &'static str, version: &'static str, to: Option<SocketAddr>) {
    CONFIG.get_or_init(|| Exporter { service, version, to });
}

fn exporter() -> Option<&'static Sender<Value>> {
    static OUT: OnceLock<Option<Sender<Value>>> = OnceLock::new();
    OUT.get_or_init(|| {
        let &Exporter { service, version, to } = CONFIG.get()?;
        let to = to?;
        let (send, spans) = channel::<Value>();
        std::thread::Builder::new().name("otel-export".into()).spawn(move || {
            while let Ok(first) = spans.recv() {
                // A short wait gathers one trace's spans into one request.
                std::thread::sleep(Duration::from_millis(200));
                let batch: Vec<Value> = std::iter::once(first).chain(spans.try_iter()).collect();
                let body = json!({"resourceSpans": [{
                    "resource": {"attributes": [
                        {"key": "service.name", "value": {"stringValue": service}},
                        {"key": "service.version", "value": {"stringValue": version}},
                    ]},
                    "scopeSpans": [{"scope": {"name": service}, "spans": batch}],
                }]});
                post(to, &body.to_string());
            }
        }).ok()?;
        Some(send)
    }).as_ref()
}

/// One HTTP/1.1 POST on loopback; a refused or slow connection drops the batch.
fn post(to: SocketAddr, body: &str) {
    use std::io::{Read as _, Write as _};
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&to, Duration::from_millis(250)) else { return };
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let head = format!("POST {PATH} HTTP/1.1\r\nHost: {to}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    if s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.as_bytes())).is_ok() {
        let _ = s.read(&mut [0; 256]);
    }
}

#[cfg(test)]
mod tests;
