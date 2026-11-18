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
use std::sync::OnceLock;
use std::time::Duration;

use otlp_lite::{Ctx, Span, ms, record};
use rune::time::Instant;
use serde_json::Value;

/// After this, a late older page or decode no longer joins the open's trace.
const FOLLOW: Duration = Duration::from_secs(30);

fn on() -> bool {
    otlp_lite::start("opencode-rune", env!("CARGO_PKG_VERSION"), enabled().then(|| otlp_lite::endpoint("1")).flatten());
    otlp_lite::on()
}

pub fn enabled() -> bool {
    match std::env::var_os("OPENCODE_RUNE_OTEL").and_then(|v| v.into_string().ok()).as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => true,
    }
}

/// Correlate a lost reply with server admission without recording prompt text or raw URLs.
pub fn send_receipt(session: &str, id: &str, outcome: &'static str, status: u16) {
    if on() {
        Span::root("prompt.delivery").attr("session.id", session).attr("message.id", id)
            .attr("outcome", outcome).attr("http.status_code", status as i64).end_at(Instant::now());
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
    static PAINTED: RefCell<Option<(String, usize)>> = const { RefCell::new(None) };
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
    /// A restored session's disk copy was on screen: a server page that redraws nothing ends it.
    painted: bool,
}

/// A click on session `id` (UI thread). `held`: the opening gate keeps the old page up.
pub fn open_begin(id: &str, held: bool) {
    if !on() { return; }
    OPEN.with(|o| {
        if let Some(old) = o.borrow_mut().take() && let Some(root) = old.root {
            let late = old.started.elapsed() >= FOLLOW;
            root.attr("outcome", if late { "unpainted" } else { "replaced" }).end_at(if late { old.started + FOLLOW } else { Instant::now() });
        }
        let root = Span::root("session.open").attr("session.id", id).attr("held", held);
        *o.borrow_mut() = Some(Open { id: id.to_owned(), ctx: root.ctx(), started: root.start, root: Some(root), committed: None, history: false, pages: 0, restored: false, live: false, painted: false });
    });
}

fn with_open<R>(id: &str, f: impl FnOnce(&mut Open) -> R) -> Option<R> {
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        if o.as_ref().is_some_and(|x| x.started.elapsed() >= FOLLOW) {
            // Never painted within FOLLOW: it ends there rather than waiting for the next open.
            if let Some(open) = o.take() && let Some(root) = open.root { root.attr("outcome", "unpainted").end_at(open.started + FOLLOW); }
            return None;
        }
        let open = o.as_mut().filter(|x| x.id == id)?;
        Some(f(open))
    })
}

static LAUNCHED: OnceLock<Instant> = OnceLock::new();
static FIRST_FRAME: OnceLock<(Instant, Option<String>, usize)> = OnceLock::new();
static PHASES: std::sync::Mutex<Vec<(&'static str, Instant, Instant)>> = std::sync::Mutex::new(Vec::new());
static LAUNCH_READS: std::sync::Mutex<Vec<(&'static str, Instant, Instant)>> = std::sync::Mutex::new(Vec::new());

pub fn startup<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    let start = Instant::now();
    let result = f();
    if FIRST_FRAME.get().is_none() { PHASES.lock().unwrap_or_else(|e| e.into_inner()).push((name, start, Instant::now())); }
    result
}

/// First thing in `main`: a restored session's open is timed from here.
pub fn launched() { LAUNCHED.get_or_init(Instant::now); }

/// One of the first server reads at launch (any thread), for the restored session's trace.
pub fn launch_read(path: &'static str, start: Instant) {
    LAUNCH_READS.lock().unwrap_or_else(|e| e.into_inner()).push((path, start, Instant::now()));
}

/// Launch reconnected to the session open at last quit; `cached` messages painted from disk.
pub fn open_restored(id: &str, cached: usize) {
    if !on() { return; }
    let start = LAUNCHED.get().copied().unwrap_or_else(Instant::now);
    let root = Span::root("session.open").at(start).attr("session.id", id).attr("restored", true).attr("cached_rows", cached as i64);
    let ctx = root.ctx();
    for &(name, a, b) in PHASES.lock().unwrap_or_else(|e| e.into_inner()).iter() { record(ctx, name, (a, b), vec![]); }
    if let Some((end, shown, rows)) = FIRST_FRAME.get() {
        record(ctx, "launch.first_frame", (start, *end), vec![("restored_session", (shown.as_deref() == Some(id)).into()), ("rows", (*rows as i64).into())]);
    }
    for (path, a, b) in LAUNCH_READS.lock().unwrap_or_else(|e| e.into_inner()).drain(..) { record(ctx, "launch.read", (a, b), vec![("path", path.into())]); }
    // on_ready connects after the cached launch frame; an unchanged server page need not draw.
    let painted = PAINTED.with(|p| p.borrow().as_ref().is_some_and(|(shown, rows)| shown == id && *rows > 0));
    OPEN.with(|o| *o.borrow_mut() = Some(Open { id: id.to_owned(), ctx, started: start, root: Some(root), committed: None, history: false, pages: 0, restored: true, live: false, painted }));
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
            if open.live && open.painted && let Some(root) = open.root.take() {
                root.attr("outcome", "cached").end_at(shown.1);
            }
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
            drop(std::thread::Builder::new().name("otel-decode-wait".into()).spawn(move || {
                let decoded = images.iter().filter(|i| i.wait().is_ok()).count();
                record(parent, "images.decode", (since, Instant::now()), vec![("count", n(images.len())), ("decoded", n(decoded)), ("thread", "rune-images".into())]);
            }));
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
    if FIRST_FRAME.get().is_none() {
        let end = Instant::now();
        drop(FIRST_FRAME.set((end, shown.map(|(id, _)| id.to_owned()), shown.map_or(0, |(_, rows)| rows))));
        if std::env::var_os("OPENCODE_RUNE_LAUNCH_REPORT").is_some() {
            for &(name, a, b) in PHASES.lock().unwrap_or_else(|e| e.into_inner()).iter() { eprintln!("{name} {:.2}", ms(b - a)); }
            let launched = LAUNCHED.get().copied().unwrap_or(end);
            eprintln!("FIRST_FRAME rows={} main_ms={:.2}", shown.map_or(0, |(_, rows)| rows), ms(end - launched));
        }
    }
    PAINTED.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_ref().map(|(id, rows)| (id.as_str(), *rows)) != shown {
            *p = shown.map(|(id, rows)| (id.to_owned(), rows));
        }
    });
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
                // A restored session's disk copy is up from the first frames, before its commit.
                open.painted |= open.restored && rows > 0;
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

thread_local! {
    /// By window id: the oldest streamed text it shows that no frame has drawn yet, when the
    /// hub heard it, and its session.
    static STREAMED: RefCell<std::collections::HashMap<String, (Instant, String)>> = RefCell::default();
}

/// Window `window` took a text delta of session `sid` that its view shows (`data`, a stream
/// event the hub stamped).
pub fn streamed(window: &str, sid: &str, data: &Value) {
    let Some(heard) = opencode_kit::client::heard_at(data).filter(|_| on()) else { return };
    STREAMED.with(|s| { s.borrow_mut().entry(window.to_owned()).or_insert_with(|| (heard, sid.to_owned())); });
}

/// After window `window` drew a frame at `at`: `stream.paint` runs from the hub hearing the
/// oldest delta the frame shows to the frame, so a window that draws late shows as late.
pub fn stream_drawn(window: &str, at: Instant) {
    let Some((heard, sid)) = STREAMED.with(|s| s.borrow_mut().remove(window)) else { return };
    Span::root("stream.paint").at(heard).attr("window", window).attr("session.id", sid.as_str())
        .attr("latency_ms", ms(at.saturating_duration_since(heard))).end_at(at);
}

/// The last `encoded` frame, for `drawn`.
pub fn last_frame() -> Option<(Instant, Option<f64>, Option<f64>)> { FRAME.with(|x| x.borrow_mut().take()) }
