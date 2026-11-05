//! Latest-page prefetch and a bounded, server-scoped disk cache. Two read lanes keep a
//! foreground click independent of speculative reads. Neither lane can write to the server.
use crate::pickers::PickerHost as _;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, mpsc::{Receiver, Sender, channel}};
use std::time::{Duration, Instant};

use opencode_kit::{client::Server, model::{self, Message}};
use rune::Waker;
use serde_json::{Value, json};

const STILL_TRYING: &str = "OpenCode isn't answering. Still trying.";
pub const LATEST: usize = 12;
const ENTRIES: usize = 12;
const DISK_BYTES: u64 = 32 << 20;
const MEMORY_BYTES: usize = 64 << 20;
const PAGE_BYTES: usize = 4 << 20;
pub const WARM: usize = 6;
const HOLD: Duration = Duration::from_millis(150);
/// Older pages as you scroll up: Desktop's `messagePageLimit`, asked for once the timeline
/// stands within its 200 px of the top (`interaction.ts` `onHistoryScroll`).
const OLDER: usize = 20;
const NEAR_TOP: f32 = 200.0;
/// A failed or stuck older page isn't asked for again sooner (the pool's own backoff).
const RETRY: Duration = Duration::from_secs(30);

#[cfg(test)]
mod bench_tests;
#[cfg(test)]
mod prefetch_tests;
#[cfg(test)]
mod typing_tests;

/// Session-page prefetch pool, delayed history-cursor expansion, and older pages on scroll.
#[derive(Default)]
pub struct State {
    pool: Option<Pages>,
    history: Option<(Instant, Key, String)>,
    /// Sessions whose first history page landed, with the cursor to older messages (None at
    /// the session's start).
    older: HashMap<String, Option<String>>,
    fetching: Option<Fetching>,
    /// The session whose timeline stands near its top, told by its scroll viewport.
    near_top: std::rc::Rc<std::cell::RefCell<String>>,
    wake: Option<Waker>,
    /// The calm "still trying" toast's id while it is up, and whether this silence showed one.
    waiting: Option<u64>,
    told: bool,
    stall_after: Option<Duration>,
}

/// How long the server may stay silent before the app says so.
const STALL_AFTER: Duration = Duration::from_secs(30);

/// The older page being read.
struct Fetching { id: String, at: Instant, slow: bool, failed: bool }

impl State {
    pub fn set(&mut self, pages: Pages) {
        self.wake = pages.wake.clone();
        self.older.clear();
        self.fetching = None;
        self.pool = Some(pages);
    }

    pub fn has_pool(&self) -> bool {
        self.pool.is_some()
    }

    /// For session `id`'s timeline scroll viewport: standing near its top, the next poll reads
    /// the page before it.
    pub fn watch_top(&self, id: &str) -> impl Fn(rune::ScrollPos) + 'static {
        let (near, wake, id) = (self.near_top.clone(), self.wake.clone(), id.to_owned());
        move |at| {
            let is = at.offset[1] < NEAR_TOP;
            let mut n = near.borrow_mut();
            if is && *n != id {
                *n = id.clone();
                drop(n);
                // The scroll may have stopped on this frame; poll without waiting for another.
                if let Some(wake) = &wake { wake(); }
            } else if !is && *n == id { n.clear(); }
        }
    }

    /// An older page for `id` has taken longer than the opening hold: show that it's coming.
    pub fn loading_older(&self, id: &str) -> bool {
        self.fetching.as_ref().is_some_and(|f| f.id == id && f.slow && !f.failed)
    }

    /// Whether a page before `id`'s oldest loaded message is still to read.
    pub fn more_above(&self, id: &str) -> bool {
        self.older.get(id).is_some_and(Option::is_some)
    }

    #[cfg(test)]
    pub fn stall_after(&mut self, after: Duration) {
        self.stall_after = Some(after);
    }

    #[cfg(test)]
    pub fn older(&self, id: &str) -> Option<&Option<String>> {
        self.older.get(id)
    }

    /// As though `id`'s first history page had landed, leaving `cursor` to read before it.
    #[cfg(test)]
    pub fn set_older(&mut self, id: &str, cursor: Option<String>) {
        self.older.insert(id.into(), cursor);
    }
}

/// Only the transcript remains `from` until content can paint or the loading deadline expires.
pub struct Opening {
    pub id: String,
    from: super::Route,
    started: Instant,
    clock: f64,
    pub loading: bool,
    received: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key { pub id: String, pub updated: i64 }

#[derive(Clone)]
pub struct Page {
    pub key: Key,
    pub messages: Vec<Message>,
    pub cursor: Option<String>,
    pub cached: bool,
    pub history: bool,
    pub images: Vec<super::attachments::PreparedImage>,
    pub(crate) bytes: usize,
    /// How the reader got it, for the open's trace.
    pub read: crate::telemetry::Read,
}

#[derive(Clone, PartialEq, Eq)]
struct Job { key: Key, cursor: Option<String>, limit: usize, foreground: bool }

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    active: HashSet<(Key, Option<String>)>,
    ready: VecDeque<Arc<Page>>,
    failed: HashMap<Key, Instant>,
    /// When the open lane's reads began failing to get an answer; cleared by one that does.
    stalled: Option<Instant>,
    stopped: bool,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;
pub type Fetch = Arc<dyn Fn(&str) -> Result<Value, String> + Send + Sync>;

pub struct Pages {
    shared: Shared,
    pub results: Receiver<Result<Arc<Page>, (Key, String)>>,
    wake: Option<Waker>,
}

impl Drop for Pages {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap().stopped = true;
        self.shared.1.notify_all();
    }
}

impl Pages {
    /// In-process delivery for deterministic stories/tests; no server or worker is started.
    pub(crate) fn fixture(ready: Option<Arc<Page>>) -> (Self, Sender<Result<Arc<Page>, (Key, String)>>) {
        let (send, results) = channel();
        let mut q = Queue::default();
        q.ready.extend(ready);
        (Self { shared: Arc::new((Mutex::new(q), Condvar::new())), results, wake: None }, send)
    }
    pub fn live(server: Server, wake: Waker) -> Self {
        let dir = disk_dir(&server.base);
        Self::new(Arc::new(move |path| server.read_timed(path).map(|(value, t)| {
            crate::telemetry::note_read(t.first_byte, t.bytes, t.parsing);
            value
        })), dir, wake)
    }

    pub fn new(fetch: Fetch, dir: Option<PathBuf>, wake: Waker) -> Self {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (out, results) = channel();
        for foreground in [true, false] {
            let (q, fetch, dir, out, wake) = (shared.clone(), fetch.clone(), dir.clone(), out.clone(), wake.clone());
            std::thread::Builder::new().name(format!("session-{}", if foreground { "open" } else { "prefetch" }))
                .spawn(move || worker(q, fetch, dir, out, wake, foreground)).expect("session reader");
        }
        Self { shared, results, wake: Some(wake) }
    }

    /// How long the open lane has gone without an answer from the server, if it has.
    pub fn stalled_for(&self) -> Option<Duration> {
        self.shared.0.lock().unwrap().stalled.map(|at| at.elapsed())
    }

    /// Starts the silence over, as when the event stream comes back.
    pub fn forgive(&self) {
        self.shared.0.lock().unwrap().stalled = None;
    }

    /// Memory is ready before the worker wakes the app, so a click racing that wake can paint.
    pub fn peek(&self, id: &str) -> Option<Arc<Page>> {
        let mut q = self.shared.0.lock().unwrap();
        let at = q.ready.iter().position(|p| p.key.id == id)?;
        let page = q.ready.remove(at)?;
        q.ready.push_back(page.clone());
        Some(page)
    }

    pub fn request(&self, key: Key, foreground: bool) {
        self.enqueue(Job { key, foreground, cursor: None, limit: LATEST });
    }

    /// The `limit` messages before `cursor`.
    pub fn history(&self, key: Key, cursor: String, limit: usize) {
        self.enqueue(Job { key, foreground: false, cursor: Some(cursor), limit });
    }

    fn enqueue(&self, job: Job) {
        let mut q = self.shared.0.lock().unwrap();
        q.failed.retain(|_, until| *until > Instant::now());
        if !job.foreground && q.failed.contains_key(&job.key) { return; }
        if job.cursor.is_none() && !job.foreground && q.ready.iter().any(|p| p.key == job.key) { return; }
        if q.active.contains(&(job.key.clone(), job.cursor.clone())) { return; }
        if let Some(old) = q.jobs.iter_mut().find(|j| j.key.id == job.key.id && j.cursor == job.cursor) {
            old.foreground |= job.foreground;
            if job.key.updated > old.key.updated { old.key = job.key; }
        } else {
            // A pointer sweeping the list cannot leave an unbounded speculative backlog.
            if !job.foreground && q.jobs.iter().filter(|j| !j.foreground).count() >= ENTRIES {
                if let Some(i) = q.jobs.iter().position(|j| !j.foreground) { q.jobs.remove(i); }
            }
            q.jobs.push_back(job);
        }
        self.shared.1.notify_all();
    }
}

fn worker(q: Shared, fetch: Fetch, dir: Option<PathBuf>, out: Sender<Result<Arc<Page>, (Key, String)>>, wake: Waker, foreground: bool) {
    let images = rune::ImageCache::new(wake.clone());
    loop {
        let job = {
            let mut state = q.0.lock().unwrap();
            loop {
                if state.stopped { return; }
                // Newest hover first; foreground has its own lane and never queues behind history.
                if let Some(i) = state.jobs.iter().rposition(|j| j.foreground == foreground) {
                    let job = state.jobs.remove(i).unwrap();
                    state.active.insert((job.key.clone(), job.cursor.clone()));
                    break job;
                }
                state = q.1.wait(state).unwrap();
            }
        };
        let publish = |value: &Value, cached, mut read: crate::telemetry::Read| {
            let parsing = Instant::now();
            let messages = model::parse_messages(value);
            let preparing = Instant::now();
            let prepared = if job.cursor.is_none() { super::attachments::prepare_images(&messages, &images) } else { vec![] };
            let done = Instant::now();
            (read.parse, read.images, read.messages) = (Some((parsing, preparing)), Some((preparing, done)), messages.len());
            (read.inline_images, read.inline_image_bytes) = super::attachments::inline_images(&messages);
            read.prepared = prepared.iter().map(|p| p.handle()).collect();
            read.thread = std::thread::current().name().unwrap_or("").to_owned();
            let page = Arc::new(Page {
                key: job.key.clone(), messages,
                cursor: value["cursor"]["next"].as_str().map(str::to_owned), cached, history: job.cursor.is_some(),
                bytes: serde_json::to_vec(value).map_or(0, |v| v.len()) + prepared.iter().map(|p| p.weight()).sum::<usize>(),
                images: prepared,
                read,
            });
            if !page.history {
                let mut state = q.0.lock().unwrap();
                if state.ready.iter().all(|p| p.key.id != page.key.id || p.key.updated <= page.key.updated) {
                    state.ready.retain(|p| p.key.id != page.key.id);
                    state.ready.push_back(page.clone());
                    while state.ready.len() > ENTRIES || state.ready.iter().map(|p| p.bytes).sum::<usize>() > MEMORY_BYTES {
                        state.ready.pop_front();
                    }
                }
            }
            if out.send(Ok(page)).is_ok() { wake(); }
        };
        if job.cursor.is_none() && let Some(dir) = &dir {
            let start = Instant::now();
            if let Some(value) = disk_read(dir, &job.key) {
                publish(&value, true, crate::telemetry::Read { source: "disk", fetch: Some((start, Instant::now())), ..Default::default() });
            }
        }
        let path = match &job.cursor {
            Some(cursor) => format!("/api/session/{}/message?limit={}&cursor={}", job.key.id, job.limit, super::switcher::encode(cursor)),
            None => format!("/api/session/{}/message?limit={}&order=desc", job.key.id, job.limit),
        };
        let mut start = Instant::now();
        crate::telemetry::take_read();
        let mut answer = fetch(&path);
        // A server that is frozen or restarting gets asked again, quietly, while the open
        // session stays as it is; the app speaks up only if the silence lasts (`State::stalled`).
        for attempt in 0.. {
            let Err(error) = &answer else { break };
            if !foreground || !opencode_kit::plain_error::is_transient(error) { break; }
            eprintln!("opencode-rune: session read failed, retrying: {error}");
            q.0.lock().unwrap().stalled.get_or_insert_with(Instant::now);
            wake();
            if !pause(&q, backoff(attempt)) { return; }
            start = Instant::now();
            crate::telemetry::take_read();
            answer = fetch(&path);
        }
        if answer.is_ok() { q.0.lock().unwrap().stalled = None; }
        match answer {
            Ok(value) => {
                let end = Instant::now();
                let mut read = crate::telemetry::Read { source: "server", fetch: Some((start, end)), ..Default::default() };
                if let Some((first_byte, bytes, parsing)) = crate::telemetry::take_read() {
                    (read.first_byte, read.bytes, read.json) = (Some(first_byte), bytes, Some((parsing, end)));
                    read.fetch = Some((start, parsing));
                }
                publish(&value, false, read);
                if job.cursor.is_none() && let Some(dir) = &dir { disk_write(dir, &job.key, &value); }
            }
            Err(error) => {
                q.0.lock().unwrap().failed.insert(job.key.clone(), Instant::now() + Duration::from_secs(30));
                if out.send(Err((job.key.clone(), error))).is_ok() { wake(); }
            }
        }
        q.0.lock().unwrap().active.remove(&(job.key, job.cursor));
    }
}

/// 2 s, 4 s, then 8 s between reads (tens of milliseconds under test).
fn backoff(attempt: u32) -> Duration {
    let unit = if cfg!(test) { Duration::from_millis(10) } else { Duration::from_secs(1) };
    unit * (2u32 << attempt.min(2))
}

/// Sleeps `total`; false if the pool stopped meanwhile.
fn pause(q: &Shared, total: Duration) -> bool {
    let until = Instant::now() + total;
    while Instant::now() < until {
        if q.0.lock().unwrap().stopped { return false; }
        std::thread::sleep(Duration::from_millis(10).min(until - Instant::now()));
    }
    !q.0.lock().unwrap().stopped
}

fn disk_dir(server: &str) -> Option<PathBuf> {
    // Stable server namespace, including port; session IDs alone do not identify a server.
    let namespace = server.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let cache = if cfg!(target_os = "macos") { home.join("Library/Caches") }
        else { std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cache")) };
    Some(cache.join(format!("rune-opencode/pages-v1/{namespace:016x}")))
}

fn file(dir: &std::path::Path, key: &Key) -> Option<PathBuf> {
    if !key.id.starts_with("ses_") || !key.id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') { return None; }
    Some(dir.join(format!("{}-{}.json", key.id, key.updated)))
}

fn disk_read(dir: &std::path::Path, key: &Key) -> Option<Value> {
    let path = file(dir, key)?;
    if std::fs::metadata(&path).ok()?.len() > PAGE_BYTES as u64 { return None; }
    let mut v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (v["id"] == key.id && v["updated"] == key.updated && v["page"]["data"].is_array()).then(|| v["page"].take())
}

/// The newest cached page on disk for `id` on `server`, read once before the first frame so the
/// restored session shows its last-known messages while `on_ready` connects.
pub fn load_cached_latest(server: &str, id: &str) -> Option<Vec<Message>> {
    if !id.starts_with("ses_") || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') { return None; }
    let dir = disk_dir(server)?;
    let prefix = format!("{id}-");
    let updated = std::fs::read_dir(&dir).ok()?.flatten().filter_map(|e| {
        let name = e.file_name();
        let rest = name.to_str()?.strip_prefix(&prefix)?.strip_suffix(".json")?;
        rest.parse::<i64>().ok()
    }).max()?;
    let value = disk_read(&dir, &Key { id: id.to_owned(), updated })?;
    let messages = model::parse_messages(&value);
    (!messages.is_empty()).then_some(messages)
}

fn disk_write(dir: &std::path::Path, key: &Key, value: &Value) {
    let Some(path) = file(dir, key) else { return };
    let Ok(bytes) = serde_json::to_vec(&json!({"id": key.id, "updated": key.updated, "page": value})) else { return };
    if bytes.len() > PAGE_BYTES || std::fs::create_dir_all(dir).is_err() { return; }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_err() { return; }
    }
    if !crate::disk::replace(&path, &bytes) { return; }
    // Count and byte bounds include obsolete updated-time versions. All work is off-thread.
    let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| {
        let m = e.metadata().ok()?;
        (e.path().extension()?.to_str()? == "json").then_some((m.modified().ok()?, m.len(), e.path()))
    }).collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut bytes = 0;
    for (i, (_, size, path)) in files.into_iter().enumerate() {
        bytes += size;
        if i >= ENTRIES || bytes > DISK_BYTES { let _ = std::fs::remove_file(path); }
    }
}

impl super::App {
    pub fn cancel_open(&mut self, tx: &mut rune::Tx) {
        if let Some(opening) = self.opening.take() {
            if let super::Route::Session(id) = &opening.from {
                self.sync_route_draft(&self.route.clone(), id);
            }
            self.route = opening.from;
            tx.settle();
        }
    }

    pub fn begin_open(&mut self, id: &str, tx: &mut rune::Tx) {
        self.close_picker(false);
        self.activity_open = false;
        self.todo_open = false;
        let from = self.opening.as_ref().filter(|o| !o.loading).map_or_else(|| self.route.clone(), |o| o.from.clone());
        self.opening = Some(Opening { id: id.into(), from, started: Instant::now(), clock: self.clock, loading: false, received: self.transcripts.contains_key(id), error:None });
        self.open_now(id, tx);
        self.open_page(id, tx);
        self.poll_open(tx);
        if self.opening.is_some() { tx.wake_after(HOLD); }
    }

    fn poll_open(&mut self, tx: &mut rune::Tx) -> bool {
        let Some(opening) = &self.opening else { return false };
        if self.route != opening.from && self.route != super::Route::Session(opening.id.clone()) {
            self.opening = None;
            return false;
        }
        let id = opening.id.clone();
        let ready = opening.received && self.transcripts.get(&id).is_some_and(|t| self.attach.transcript_ready(t));
        if ready {
            self.opening = None;
            // Mount the finished transcript at its end without resetting input or focus.
            tx.settle();
            if let Some(page) = self.pages.pool.as_ref().and_then(|p| p.peek(&id)) { self.schedule_history(&page, tx); }
            return true;
        }
        // The fixture/story clock is deterministic; an idle live window must use wall time,
        // because its last poll may have been minutes before this click.
        let expired = if self.live() { opening.started.elapsed() >= HOLD } else { self.clock >= opening.clock + HOLD.as_secs_f64() };
        if expired && !opening.loading {
            self.opening.as_mut().unwrap().loading = true;
            tx.settle();
            return true;
        }
        false
    }

    /// The session on screen with its messages painted (not held, not loading), for traces.
    pub fn painted_session(&self) -> Option<(&str, usize)> {
        let super::Route::Session(id) = &self.route else { return None };
        if self.opening.is_some() { return None; }
        Some((id, self.transcripts.get(id).map_or(0, |t| t.messages.len())))
    }

    pub fn loading_session(&self, id: &str) -> bool {
        self.opening.as_ref().is_some_and(|o| o.id == id && o.loading)
    }

    /// The held transcript is read-only; the route and composer already belong to the target.
    pub(crate) fn displayed_session<'a>(&'a self, id: &'a str) -> Option<&'a str> {
        match self.opening.as_ref().filter(|o| o.id == id && !o.loading) {
            Some(o) => match &o.from { super::Route::Session(from) => Some(from), _ => None },
            None => Some(id),
        }
    }

    /// Reads `id`'s latest page from the server again, ahead of anything speculative.
    pub(crate) fn read_latest(&self, id: &str) {
        if let Some(pool) = &self.pages.pool { pool.request(self.page_key(id), true); }
    }

    pub(crate) fn page_key(&self, id: &str) -> Key {
        let updated = self.session(id).or_else(|| self.switcher.loaded.iter().find(|s| s.id == id)).map_or(0, |s| s.updated);
        Key { id: id.to_owned(), updated }
    }

    pub(crate) fn reload_session(&self, id: &str) {
        let super::Source::Live { requests: Some(r), .. } = &self.source else { return };
        match &self.pages.pool {
            Some(pages) => pages.request(self.page_key(id), true),
            None => {
                let _ = r.send(opencode_kit::client::Request::Session(id.to_owned()));
            }
        }
    }

    pub fn prefetch_session(&self, id: &str) {
        if let Some(pages) = &self.pages.pool { pages.request(self.page_key(id), false); }
    }

    /// Runs before Rune's list focus traversal; start the next highlighted row's read now.
    pub fn prefetch_navigation(&self, ids: &[String], index: usize, ev: &rune::KeyEvent) -> bool {
        use rune::Key;
        if ids.is_empty() { return false; }
        let next = match ev.key {
            Key::Down | Key::Tab if !ev.mods.shift => (index + 1).min(ids.len() - 1),
            Key::Up | Key::Tab => index.saturating_sub(1),
            Key::Home => 0,
            Key::End => ids.len() - 1,
            _ => index,
        };
        self.prefetch_session(&ids[next]);
        false
    }

    pub fn open_page(&mut self, id: &str, tx: &mut rune::Tx) {
        let Some(pages) = &self.pages.pool else { return };
        let ready = pages.peek(id);
        pages.request(self.page_key(id), true);
        if let Some(page) = ready {
            if !self.transcripts.contains_key(id) { self.show_page(&page, tx); }
            else { self.schedule_history(&page, tx); }
        }
    }

    pub(crate) fn show_page(&mut self, page: &Page, tx: &mut rune::Tx) {
        let shown = Instant::now();
        self.merge_page(page, tx);
        crate::telemetry::page(&page.key.id, page.history, &page.read, (shown, Instant::now()));
    }

    fn merge_page(&mut self, page: &Page, tx: &mut rune::Tx) {
        if let Some(o) = &mut self.opening && o.id == page.key.id && !page.history { o.received = true; }
        self.attach.adopt(&page.images);
        let id = &page.key.id;
        let stale = self.page_key(id).updated > page.key.updated;
        let first = !self.transcripts.contains_key(id);
        let t = self.transcripts.entry(id.clone()).or_insert_with(|| model::Transcript { session: id.clone(), messages: vec![] });
        let mut messages = page.messages.clone();
        // Disk, memory and older history must never rewind a live delta or local echo.
        if page.cached || page.history || stale { messages.retain(|m| !t.messages.iter().any(|old| old.id() == m.id())); }
        super::attachments::reconcile(&mut self.attach.echoes, t, &messages);
        let delivered: Vec<_> = messages.iter().map(|m| m.id().to_owned()).collect();
        if page.history {
            // Cursor order, not ID order: a steered prompt can have an older ID than history.
            messages.append(&mut t.messages);
            t.messages = messages;
        } else { t.merge(messages); }
        self.settle_inbox(id, &delivered);
        let cold = !page.cached && self.launch.cold_transcript.as_deref() == Some(id.as_str());
        if cold { self.launch.cold_transcript = None; }
        // Not for older pages read while scrolled back: a settle pins the timeline to its end.
        let older = page.history && self.pages.older.contains_key(id);
        if first || (page.history && !older) || cold { tx.settle(); }
        if page.history {
            // An empty page ends it too, so a server that keeps handing out cursors can't loop.
            let older = page.cursor.clone().filter(|_| !page.messages.is_empty());
            self.pages.older.insert(id.clone(), older);
            if self.pages.fetching.as_ref().is_some_and(|f| f.id == *id) { self.pages.fetching = None; }
            // Where the page leaves the reader is reported next frame; read on only from there.
            self.pages.near_top.borrow_mut().clear();
        }
        self.schedule_history(page, tx);
    }

    fn schedule_history(&mut self, page: &Page, tx: &mut rune::Tx) {
        let id = &page.key.id;
        if self.route == super::Route::Session(id.clone()) && !page.history && !page.cached && !self.pages.older.contains_key(id) && let Some(cursor) = &page.cursor {
            // Give the latest page its first frame before expanding history.
            self.pages.history = Some((Instant::now() + Duration::from_millis(50), page.key.clone(), cursor.clone()));
            tx.wake_after(Duration::from_millis(50));
        }
    }

    pub fn poll_pages(&mut self, tx: &mut rune::Tx) -> bool {
        let results: Vec<_> = self.pages.pool.as_ref().map(|p| p.results.try_iter().collect()).unwrap_or_default();
        let mut changed = false;
        for result in results {
            if let Ok(page) = &result && !page.cached {
                self.judge_sends_by_page(&page.key.id, &page.messages);
                self.check_sends();
                changed = true;
            }
            match result {
                Ok(page) if self.route == super::Route::Session(page.key.id.clone()) || self.opening.as_ref().is_some_and(|o| o.id == page.key.id) || (page.history && self.transcripts.contains_key(&page.key.id)) => { self.show_page(&page, tx); changed = true; }
                Err((key, error)) if self.opening.as_ref().is_some_and(|o| o.id == key.id) => {
                    self.status = error.clone();
                    self.opening.as_mut().unwrap().error = Some(error);
                    changed = true;
                }
                Err((key, error)) if self.route == super::Route::Session(key.id.clone()) => {
                    if let Some(f) = &mut self.pages.fetching && f.id == key.id { f.failed = true; }
                    // A busy server is waited out; only silence that lasts is told (below).
                    if opencode_kit::plain_error::is_transient(&error) { eprintln!("opencode-rune: session read failed: {error}"); continue; }
                    crate::status_toast::fail(self, "Couldn't load this session", crate::status_toast::plain(error.split_once(": ").map_or(error.as_str(), |(_, why)| why)));
                    changed = true;
                }
                _ => {}
            }
        }
        changed |= self.poll_open(tx);
        changed |= self.tell_stall();
        if self.pages.history.as_ref().is_some_and(|(at, _, _)| *at <= Instant::now()) {
            let (_, key, cursor) = self.pages.history.take().unwrap();
            if self.route == super::Route::Session(key.id.clone()) && let Some(pages) = &self.pages.pool { pages.history(key, cursor, 80 - LATEST); }
        }
        changed | self.load_older(tx)
    }

    /// Says once, calmly, that the server has stayed silent, and takes it back when it answers.
    fn tell_stall(&mut self) -> bool {
        let silent = self.pages.pool.as_ref().and_then(Pages::stalled_for);
        let after = self.pages.stall_after.unwrap_or(STALL_AFTER);
        match (silent, self.pages.waiting) {
            (Some(for_how_long), None) if for_how_long >= after && !self.pages.told => {
                self.pages.told = true;
                self.pages.waiting = Some(crate::status_toast::waiting(self, STILL_TRYING));
                true
            }
            (None, waiting) => {
                self.pages.told = false;
                let Some(id) = waiting else { return false };
                self.pages.waiting = None;
                crate::status_toast::dismiss_id(self, id);
                true
            }
            _ => false,
        }
    }

    /// The event stream is back: the silence is over, whatever the open session's read says.
    pub(crate) fn stream_back(&mut self) {
        if let Some(pool) = &self.pages.pool { pool.forgive(); }
        self.tell_stall();
    }

    /// Reads the page before the open session's oldest message once its timeline nears the
    /// top. It lands above what's on screen without moving it: the virtual list holds its
    /// first visible row (Rune `VirtualList::anchor_delta`).
    fn load_older(&mut self, tx: &mut rune::Tx) -> bool {
        if let Some(f) = &mut self.pages.fetching {
            let waited = f.at.elapsed();
            if waited < RETRY {
                if f.failed || f.slow || waited < HOLD { return false; }
                f.slow = true;
                return true;
            }
            self.pages.fetching = None;
        }
        let super::Route::Session(id) = &self.route else { return false };
        if *self.pages.near_top.borrow() != *id { return false; }
        let (Some(Some(cursor)), Some(pages)) = (self.pages.older.get(id), &self.pages.pool) else { return false };
        pages.history(self.page_key(id), cursor.clone(), OLDER);
        self.pages.fetching = Some(Fetching { id: id.clone(), at: Instant::now(), slow: false, failed: false });
        tx.wake_after(HOLD);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{App, Route, view};
    use rune::{Env, Runtime, Tx};

    fn key(id: &str) -> Key { Key { id: id.into(), updated: 7 } }
    fn page(text: &str) -> Value {
        json!({"data":[{"id":"msg_last", "type":"user", "text":text,"time":{"created":1}}], "cursor":{"next":"older+/="}})
    }
    fn receive(p: &Pages) -> Arc<Page> { p.results.recv_timeout(Duration::from_secs(5)).unwrap().unwrap() }
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            Self(std::env::temp_dir().join(format!("session-pages-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))))
        }
    }
    impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn a_prefetched_click_has_messages_on_its_first_frame() {
        let pages = Pages::new(Arc::new(|path| {
            assert!(path.ends_with("?limit=12&order=desc"));
            Ok(page("Ready before the click"))
        }), None, Arc::new(|| {}));
        pages.request(key("ses_warm"), false);
        receive(&pages);
        let mut app = App::empty();
        app.pages.set(pages);
        app.open("ses_warm", &mut Tx::default());
        assert_eq!(app.transcripts["ses_warm"].messages.len(), 1);
        let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(!rt.elements().iter().any(|e| e.path.ends_with("/loading")));
    }

    #[test]
    fn foreground_does_not_wait_for_a_blocked_background_read() {
        let (started, start) = channel();
        let (release, wait) = channel();
        let wait = Mutex::new(wait);
        let pages = Pages::new(Arc::new(move |path| {
            if path.contains("ses_slow/") { started.send(()).unwrap(); wait.lock().unwrap().recv().unwrap(); }
            Ok(page("ready"))
        }), None, Arc::new(|| {}));
        pages.request(key("ses_slow"), false);
        start.recv_timeout(Duration::from_secs(5)).unwrap();
        pages.request(key("ses_click"), true);
        let first = receive(&pages);
        release.send(()).unwrap();
        assert_eq!(first.key.id, "ses_click");
        assert_eq!(receive(&pages).key.id, "ses_slow");
    }

    #[test]
    fn disk_paints_before_network_and_updated_time_invalidates_it() {
        let dir = Scratch::new();
        let k = key("ses_disk");
        disk_write(&dir.0, &k, &page("cached"));
        assert!(disk_read(&dir.0, &Key { updated: 8, ..k.clone() }).is_none());
        let (release, wait) = channel();
        let wait = Mutex::new(wait);
        let pages = Pages::new(Arc::new(move |_| { wait.lock().unwrap().recv().unwrap(); Ok(page("fresh")) }), Some(dir.0.clone()), Arc::new(|| {}));
        pages.request(k, true);
        let cached = receive(&pages);
        assert!(cached.cached);
        release.send(()).unwrap();
        assert!(!receive(&pages).cached);
    }

    #[test]
    fn cached_page_and_history_do_not_rewind_live_messages() {
        let mut app = App::empty();
        app.route = Route::Session("ses_a".into());
        let mut p = Page { key: key("ses_a"), messages: model::parse_messages(&page("new")), cursor: None, cached: false, history: false, images: vec![], bytes: 0, read: Default::default() };
        app.show_page(&p, &mut Tx::default());
        p.messages = model::parse_messages(&page("old"));
        p.cached = true;
        app.show_page(&p, &mut Tx::default());
        assert!(matches!(&app.transcripts["ses_a"].messages[0], Message::User { text, .. } if text == "new"));
        p.cached = false;
        p.history = true;
        p.messages.insert(0, Message::User { id: "msg_first".into(), text: "history".into() });
        app.show_page(&p, &mut Tx::default());
        assert_eq!(app.transcripts["ses_a"].messages.len(), 2);
        assert!(matches!(&app.transcripts["ses_a"].messages[1], Message::User { text, .. } if text == "new"));
    }

    #[test]
    fn history_uses_cursor_without_order_and_cache_is_bounded() {
        let pages = Pages::new(Arc::new(|path| {
            assert!(path.contains("?limit=68&cursor=older%2B%2F%3D"));
            assert!(!path.contains("order="));
            Ok(page("history"))
        }), None, Arc::new(|| {}));
        pages.history(key("ses_a"), "older+/=".into(), 68);
        assert!(receive(&pages).history);
        let dir = Scratch::new();
        for i in 0..16 { disk_write(&dir.0, &key(&format!("ses_{i}")), &page("cached")); }
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), ENTRIES);
        std::fs::write(file(&dir.0, &key("ses_bad")).unwrap(), "broken").unwrap();
        assert!(disk_read(&dir.0, &key("ses_bad")).is_none());
        assert!(file(&dir.0, &key("ses_../../bad")).is_none());
    }

    /// The whole click-to-paint sequence, not just the frame after recv() has completed.
    #[test]
    fn cold_open_frames_never_flash() {
        frame_sequence(false, None);
    }

    #[test]
    fn prefetched_open_frames_never_flash() {
        frame_sequence(true, None);
    }

    #[test]
    #[ignore = "private latest-page capture; COLD_OPEN_CAPTURE=/path/page.json --release"]
    fn captured_open_frames_never_flash() {
        let data = std::fs::read_to_string(std::env::var("COLD_OPEN_CAPTURE").unwrap()).unwrap();
        for warm in [false, true] { frame_sequence(warm, Some(serde_json::from_str(&data).unwrap())); }
    }

    #[test]
    fn opening_deadline_and_navigation() {
        let mut app = App::empty();
        let (pages, send) = Pages::fixture(None);
        app.pages.set(pages);
        let mut rt = Runtime::new(app, view::app, (1280.0,820.0), Env::new(true,1.0));
        rt.frame(0.0);
        rt.update(|s, tx| s.open("ses_slow", tx));
        // A live delta before the first page must not bypass the opening gate.
        rt.state.transcripts.insert("ses_slow".into(), model::Transcript { session:"ses_slow".into(), messages:vec![] });
        for (at, loading) in [(0.0,false), (0.149,false), (0.150,true), (0.2,true)] {
            rt.update(|s, tx| { s.clock = at; s.poll_pages(tx); });
            rt.frame(1.0/60.0);
            assert_eq!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Loading messages…")), loading, "at {at}");
            assert!(!rt.elements().iter().any(|e| e.path.contains("/usage/") && e.text.as_deref() == Some("0")));
        }
        let result = |id: &str| Arc::new(Page { key:key(id), messages:model::parse_messages(&page("All ready")), cursor:None, cached:false, history:false, images:vec![], bytes:0, read:Default::default() });
        send.send(Ok(result("ses_slow"))).unwrap();
        rt.update(|s, tx| { s.poll_pages(tx); });
        rt.frame(1.0/60.0);
        assert!(rt.state.opening.is_none());
        assert!(!rt.elements().iter().any(|e| e.text.as_deref() == Some("Loading messages…")));

        rt.update(|s, tx| { s.open("ses_b", tx); s.open("ses_c", tx); });
        send.send(Ok(result("ses_b"))).unwrap();
        rt.update(|s, tx| { s.poll_pages(tx); });
        assert_eq!(rt.state.route, Route::Session("ses_c".into()), "input follows the newest click immediately");
        send.send(Ok(result("ses_c"))).unwrap();
        rt.update(|s, tx| { s.poll_pages(tx); });
        assert_eq!(rt.state.route, Route::Session("ses_c".into()));

        rt.update(|s, tx| { s.open("ses_cancel", tx); s.route = Route::Home; s.poll_pages(tx); });
        send.send(Ok(result("ses_cancel"))).unwrap();
        rt.update(|s, tx| { s.poll_pages(tx); });
        assert_eq!(rt.state.route, Route::Home);
        assert!(rt.state.opening.is_none());

        rt.update(|s, tx| s.open("ses_failed", tx));
        send.send(Err((key("ses_failed"), "Session unavailable".into()))).unwrap();
        rt.update(|s, tx| { s.clock += 0.150; s.poll_pages(tx); });
        rt.frame(1.0/60.0);
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Session unavailable")));
        rt.update(|s, tx| s.cancel_open(tx));
        assert_eq!(rt.state.route, Route::Home);
    }

    fn frame_sequence(prefetched: bool, capture: Option<Value>) {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1600, 600);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&vec![140; 1600 * 600 * 3]).unwrap();
        }
        let mut data = json!({"data": [
            {"id":format!("msg_image_{prefetched}"), "type":"user", "text":"Review this layout", "time":{"created":100},
             "files":[{"mime":"image/png", "name":"layout.png", "uri":opencode_kit::client::data_url("image/png", &png)}]},
            {"id":"msg_answer", "type":"assistant", "time":{"created":101,"completed":102},
             "tokens":{"input":42000}, "content":[{"type":"text","text":"The image and this answer should arrive together."}]}
        ]});
        for i in (0..24).rev() {
            data["data"].as_array_mut().unwrap().insert(0, json!({"id":format!("msg_prefix_{i:02}"),"type":"user","text":format!("Earlier layout note {i}: preserve the final position when this session opens."),"time":{"created":i}}));
        }
        let data = capture.unwrap_or(data);
        let (release, wait) = channel();
        let wait = Mutex::new(wait);
        let pages = Pages::new(Arc::new(move |_| {
            wait.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(data.clone())
        }), None, Arc::new(|| {}));
        let mut app = App::empty();
        app.route = Route::Session("ses_previous".into());
        app.transcripts.insert("ses_previous".into(), model::Transcript { session:"ses_previous".into(), messages:model::parse_messages(&page("Previous session stays here")) });
        app.pages.set(pages);
        if prefetched {
            app.prefetch_session("ses_target");
            release.send(()).unwrap();
            let p = receive(app.pages.pool.as_ref().unwrap());
            for image in &p.images { image.wait(); }
        }
        let mut rt = Runtime::new(app, view::app, (1280.0,820.0), Env::new(true,2.0));
        rt.frame(0.0);
        rt.update(|s, tx| s.open("ses_target", tx));
        let mut bad = Vec::new();
        let mut bottom = None;
        for frame in 0..20 {
            if frame == 3 {
                release.send(()).unwrap();
                let p = receive(rt.state.pages.pool.as_ref().unwrap());
                rt.update(|s, tx| s.show_page(&p, tx));
            }
            if frame == 10 {
                let history = Page { key: key("ses_target"), messages: (0..24).map(|i| Message::User {
                    id:format!("old_{i}"), text:format!("Earlier discussion {i}. {}", "A long note about the prior revision. ".repeat(12)),
                }).collect(), cursor:None, cached:false, history:true, images:vec![], bytes:0, read:Default::default() };
                rt.update(|s, tx| s.show_page(&history, tx));
            }
            if frame == 6 { for image in rt.state.attach.image_handles() { image.wait().unwrap(); } }
            rt.update(|s, tx| { s.clock = frame as f64 / 60.0; s.poll_pages(tx); });
            rt.frame(1.0/60.0);
            let elements = rt.elements();
            let loading = elements.iter().any(|e| e.text.as_deref() == Some("Loading messages…"));
            let target = rt.state.route == Route::Session("ses_target".into());
            let grey = rt.state.opening.is_none() && target && rt.state.attach.image_handles().iter().any(|img| !img.is_ready());
            let zero = target && elements.iter().any(|e| e.path.contains("/usage/") && e.label.as_deref() == Some("0"));
            let at_bottom = rt.scroll_at_bottom("scroll-ses_target");
            let image = elements.iter().find(|e| e.path.ends_with("/img-0") && !e.exiting).map(|e| e.rect);
            if loading || grey || zero || (rt.state.opening.is_none() && target && at_bottom == Some(false)) { bad.push(frame); }
            if target && let Some(rect) = image {
                if let Some(old) = bottom { if old != rect { bad.push(frame); } } else { bottom = Some(rect); }
            }
        }
        // Release the refresh requested by a prefetched open.
        let _ = release.send(());
        assert!(bad.is_empty(), "intermediate frames flashed: {bad:?}");
        assert_eq!(rt.state.route, Route::Session("ses_target".into()));
    }
}
