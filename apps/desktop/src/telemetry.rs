//! OpenTelemetry traces for a local Motel: OTLP/HTTP JSON to 127.0.0.1 only, posted by one
//! background thread. The UI thread only pushes finished spans onto a channel; when nothing
//! listens the batch is dropped. Attributes carry ids, counts, sizes and timings, never
//! message text or image data. `OPENCODE_RUNE_OTEL=0` turns it off; tests send only with `=1`.
//!
//! `session.open` runs from the click to the first frame that paints the session's messages.
//! Its children come from the page reader (`fetch`, `parse`, `images`), the opening gate
//! (`hold`), the frame that paints it (`first_paint`, `layout`), and after it the older page
//! (`history`) and image decodes (`images.decode`).
use std::cell::RefCell;
use std::hash::{BuildHasher, Hasher};
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}, mpsc::{Sender, channel}};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Motel's OTLP/HTTP receiver.
const ADDR: &str = "127.0.0.1:27686";
const PATH: &str = "/v1/traces";
/// After this, a late older page or decode no longer joins the open's trace.
const FOLLOW: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ctx { trace: u128, span: u64 }

pub struct Span {
    ctx: Ctx,
    parent: Option<u64>,
    name: &'static str,
    start: Instant,
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

    pub fn end(self) { self.end_at(Instant::now()) }

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
        #[cfg(test)]
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

#[cfg(test)]
thread_local! {
    /// Spans this thread ends, instead of sending them (`tests::capture`).
    static CAPTURE: RefCell<Option<Vec<Value>>> = const { RefCell::new(None) };
}

fn on() -> bool {
    #[cfg(test)]
    if CAPTURE.with(|c| c.borrow().is_some()) { return true; }
    exporter().is_some()
}

pub fn enabled() -> bool {
    match std::env::var("OPENCODE_RUNE_OTEL").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => !cfg!(test),
    }
}

fn exporter() -> Option<&'static Sender<Value>> {
    static OUT: OnceLock<Option<Sender<Value>>> = OnceLock::new();
    OUT.get_or_init(|| {
        if !enabled() { return None; }
        let (send, spans) = channel::<Value>();
        std::thread::Builder::new().name("otel-export".into()).spawn(move || {
            while let Ok(first) = spans.recv() {
                // A short wait gathers an open's spans into one request.
                std::thread::sleep(Duration::from_millis(200));
                let batch: Vec<Value> = std::iter::once(first).chain(spans.try_iter()).collect();
                let body = json!({"resourceSpans": [{
                    "resource": {"attributes": [
                        {"key": "service.name", "value": {"stringValue": "opencode-rune"}},
                        {"key": "service.version", "value": {"stringValue": env!("CARGO_PKG_VERSION")}},
                    ]},
                    "scopeSpans": [{"scope": {"name": "opencode-rune"}, "spans": batch}],
                }]});
                post(&body.to_string());
            }
        }).ok()?;
        Some(send)
    }).as_ref()
}

/// One HTTP/1.1 POST to Motel on loopback; a refused or slow connection drops the batch.
fn post(body: &str) {
    use std::io::{Read as _, Write as _};
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&ADDR.parse().unwrap(), Duration::from_millis(250)) else { return };
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let head = format!("POST {PATH} HTTP/1.1\r\nHost: {ADDR}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    if s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.as_bytes())).is_ok() {
        let _ = s.read(&mut [0; 256]);
    }
}

/// One page read, measured on the reader thread.
#[derive(Clone, Debug, Default)]
pub struct Read {
    /// `disk`, or `server`.
    pub source: &'static str,
    pub fetch: Option<(Instant, Instant)>,
    /// Request to the response's first byte, and its body.
    pub first_byte: Option<Instant>,
    pub bytes: usize,
    pub json: Option<(Instant, Instant)>,
    pub parse: Option<(Instant, Instant)>,
    pub images: Option<(Instant, Instant)>,
    pub messages: usize,
    pub inline_images: usize,
    pub inline_image_bytes: usize,
    pub prepared: Vec<rune::Image>,
    pub thread: String,
}

thread_local! {
    static READ: RefCell<Option<(Instant, usize, Instant)>> = const { RefCell::new(None) };
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static FRAME: RefCell<Option<(Instant, Option<f64>, Option<f64>)>> = const { RefCell::new(None) };
}

/// The live reader's first byte, body size and JSON parse start, for the page worker's `Read`.
pub fn note_read(first_byte: Instant, bytes: usize, json: Instant) {
    READ.with(|r| *r.borrow_mut() = Some((first_byte, bytes, json)));
}

pub fn take_read() -> Option<(Instant, usize, Instant)> { READ.with(|r| r.borrow_mut().take()) }

struct Open {
    id: String,
    ctx: Ctx,
    started: Instant,
    /// Until the first paint ends it.
    root: Option<Span>,
    committed: Option<Instant>,
    history: bool,
    pages: usize,
    /// Restored at launch: its disk copy may be up from frame 0, so it ends on a server page.
    restored: bool,
    live: bool,
}

/// A click on session `id` (UI thread). `held`: the opening gate keeps the old page up.
pub fn open_begin(id: &str, held: bool) {
    if !on() { return; }
    OPEN.with(|o| {
        if let Some(root) = o.borrow_mut().take().and_then(|old| old.root) { root.attr("outcome", "replaced").end(); }
        let root = Span::root("session.open").attr("session.id", id).attr("held", held);
        *o.borrow_mut() = Some(Open { id: id.to_owned(), ctx: root.ctx(), started: root.start, root: Some(root), committed: None, history: false, pages: 0, restored: false, live: false });
    });
}

fn with_open<R>(id: &str, f: impl FnOnce(&mut Open) -> R) -> Option<R> {
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        let open = o.as_mut().filter(|x| x.id == id && x.started.elapsed() < FOLLOW)?;
        Some(f(open))
    })
}

static LAUNCHED: OnceLock<Instant> = OnceLock::new();
static LAUNCH_READS: std::sync::Mutex<Vec<(&'static str, Instant, Instant)>> = std::sync::Mutex::new(Vec::new());

/// First thing in `main`: a restored session's open is timed from here.
pub fn launched() { LAUNCHED.get_or_init(Instant::now); }

/// One of the first server reads at launch (any thread), for the restored session's trace.
pub fn launch_read(path: &'static str, start: Instant) {
    LAUNCH_READS.lock().unwrap().push((path, start, Instant::now()));
}

/// Launch reconnected to the session open at last quit; `cached` messages painted from disk.
pub fn open_restored(id: &str, cached: usize) {
    if !on() { return; }
    let start = LAUNCHED.get().copied().unwrap_or_else(Instant::now);
    let root = Span::root("session.open").at(start).attr("session.id", id).attr("restored", true).attr("cached_rows", cached as i64);
    let ctx = root.ctx();
    for (path, a, b) in LAUNCH_READS.lock().unwrap().drain(..) { record(ctx, "launch.read", (a, b), vec![("path", path.into())]); }
    OPEN.with(|o| *o.borrow_mut() = Some(Open { id: id.to_owned(), ctx, started: start, root: Some(root), committed: None, history: false, pages: 0, restored: true, live: false }));
}

/// A page for `id` reached the app; `shown` is the UI thread's time merging it.
pub fn page(id: &str, history: bool, read: &Read, shown: (Instant, Instant)) {
    with_open(id, |open| {
        let parent = if history {
            let span = Span::child(open.ctx, "history").at(read.fetch.map_or(shown.0, |f| f.0))
                .attr("messages", read.messages as i64).attr("inline_images", read.inline_images as i64)
                .attr("inline_image_bytes", read.inline_image_bytes as i64);
            let ctx = span.ctx();
            span.end_at(shown.1);
            open.history = true;
            ctx
        } else {
            open.pages += 1;
            open.live |= read.source == "server";
            open.ctx
        };
        let n = |v: usize| Value::from(v as i64);
        let mut fetch = vec![("source", read.source.into()), ("bytes", n(read.bytes)), ("thread", read.thread.clone().into()),
            ("before_click", read.fetch.is_some_and(|f| f.0 < open.started).into())];
        if let (Some((start, _)), Some(first)) = (read.fetch, read.first_byte) { fetch.push(("server_ms", ms(first - start).into())); }
        if let Some(f) = read.fetch { record(parent, "fetch", f, fetch); }
        if let Some(j) = read.json { record(parent, "json", j, vec![("bytes", n(read.bytes))]); }
        if let Some(p) = read.parse { record(parent, "parse", p, vec![("messages", n(read.messages))]); }
        if let Some(i) = read.images {
            record(parent, "images", i, vec![("prepared", n(read.prepared.len())), ("inline_images", n(read.inline_images)),
                ("inline_image_bytes", n(read.inline_image_bytes)), ("thread", read.thread.clone().into())]);
        }
        record(parent, "show", shown, vec![("messages", n(read.messages)), ("thread", "main".into())]);
        if !read.prepared.is_empty() {
            let (images, since) = (read.prepared.clone(), read.images.map_or(shown.0, |i| i.0));
            // Decodes run on Rune's image worker; this thread only waits on their handles.
            let _ = std::thread::Builder::new().name("otel-decode-wait".into()).spawn(move || {
                let decoded = images.iter().filter(|i| i.wait().is_ok()).count();
                record(parent, "images.decode", (since, Instant::now()), vec![("count", n(images.len())), ("decoded", n(decoded)), ("thread", "rune-images".into())]);
            });
        }
    });
}

/// The opening gate let the route change: `loading` when its hold ran out first.
pub fn committed(id: &str, loading: bool) {
    with_open(id, |open| {
        if open.committed.is_some() { return; }
        let now = Instant::now();
        record(open.ctx, if open.restored { "launch" } else { "hold" }, (open.started, now), vec![("loading", loading.into()), ("pages", (open.pages as i64).into())]);
        open.committed = Some(now);
    });
}

/// `on_frame_encoded`: the newest drawn frame's timing.
pub fn encoded(f: &rune::FrameEncoded) {
    if f.outcome() == rune::FrameOutcome::Drawn { FRAME.with(|x| *x.borrow_mut() = Some((f.at(), f.update_ms(), f.encode_ms()))); }
}

/// After a drawn frame: `shown` is the session on screen with its messages, if any. Ends
/// `session.open` on the first that paints the opening session, then times the older page's.
pub fn drawn(shown: Option<(&str, usize)>, (at, update_ms, encode_ms): (Instant, Option<f64>, Option<f64>)) {
    let Some((id, rows)) = shown else { return };
    let finished = with_open(id, |open| {
        let frame = |name: &'static str| {
            let (update, encode) = (update_ms.unwrap_or(0.0), encode_ms.unwrap_or(0.0));
            let start = at.checked_sub(Duration::from_secs_f64((update + encode) / 1000.0)).unwrap_or(at);
            let span = Span::child(open.ctx, name).at(start).attr("rows", rows as i64).attr("update_ms", update).attr("encode_ms", encode);
            record(span.ctx(), "layout", (start, start + Duration::from_secs_f64(update / 1000.0)), vec![("rows", (rows as i64).into())]);
            span.end_at(at);
        };
        if let Some(root) = open.root.take() {
            if rows == 0 || open.committed.is_none() || (open.restored && !open.live) {
                open.root = Some(root);
                return false;
            }
            frame("first_paint");
            root.attr("rows", rows as i64).attr("outcome", "painted").end_at(at);
            return false;
        }
        if open.history {
            frame("history.paint");
            return true;
        }
        false
    });
    if finished == Some(true) { OPEN.with(|o| *o.borrow_mut() = None); }
}

/// The last `encoded` frame, for `drawn`.
pub fn last_frame() -> Option<(Instant, Option<f64>, Option<f64>)> { FRAME.with(|x| x.borrow_mut().take()) }

pub fn ms(d: Duration) -> f64 { (d.as_secs_f64() * 1e6).round() / 1e3 }

#[cfg(test)]
mod tests;
