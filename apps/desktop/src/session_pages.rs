//! Latest-page prefetch and a bounded, server-scoped disk cache. Two read lanes keep a
//! foreground click independent of speculative reads. Neither lane can write to the server.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, mpsc::{Receiver, Sender, channel}};
use std::time::{Duration, Instant};

use opencode_kit::{client::Server, model::{self, Message}};
use rune::Waker;
use serde_json::{Value, json};

pub const LATEST: usize = 12;
const ENTRIES: usize = 12;
const DISK_BYTES: u64 = 32 << 20;
const MEMORY_BYTES: usize = 64 << 20;
const PAGE_BYTES: usize = 4 << 20;
pub const WARM: usize = 6;
const HOLD: Duration = Duration::from_millis(150);

/// The visible route remains `from` until content can paint, or the loading deadline expires.
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
}

#[derive(Clone, PartialEq, Eq)]
struct Job { key: Key, cursor: Option<String>, foreground: bool }

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    active: HashSet<(Key, Option<String>)>,
    ready: VecDeque<Arc<Page>>,
    failed: HashMap<Key, Instant>,
    stopped: bool,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;
pub type Fetch = Arc<dyn Fn(&str) -> Result<Value, String> + Send + Sync>;

pub struct Pages {
    shared: Shared,
    pub results: Receiver<Result<Arc<Page>, (Key, String)>>,
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
        (Self { shared: Arc::new((Mutex::new(q), Condvar::new())), results }, send)
    }
    pub fn live(server: Server, wake: Waker) -> Self {
        let dir = disk_dir(&server.base);
        Self::new(Arc::new(move |path| server.read(path)), dir, wake)
    }

    pub fn new(fetch: Fetch, dir: Option<PathBuf>, wake: Waker) -> Self {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (out, results) = channel();
        for foreground in [true, false] {
            let (q, fetch, dir, out, wake) = (shared.clone(), fetch.clone(), dir.clone(), out.clone(), wake.clone());
            std::thread::Builder::new().name(format!("session-{}", if foreground { "open" } else { "prefetch" }))
                .spawn(move || worker(q, fetch, dir, out, wake, foreground)).expect("session reader");
        }
        Self { shared, results }
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
        self.enqueue(Job { key, foreground, cursor: None });
    }

    pub fn history(&self, key: Key, cursor: String) {
        self.enqueue(Job { key, foreground: false, cursor: Some(cursor) });
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
        let publish = |value: &Value, cached| {
            let messages = model::parse_messages(value);
            let prepared = if job.cursor.is_none() { super::attachments::prepare_images(&messages, &images) } else { vec![] };
            let page = Arc::new(Page {
                key: job.key.clone(), messages,
                cursor: value["cursor"]["next"].as_str().map(str::to_owned), cached, history: job.cursor.is_some(),
                bytes: serde_json::to_vec(value).map_or(0, |v| v.len()) + prepared.iter().map(|p| p.weight()).sum::<usize>(),
                images: prepared,
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
        if job.cursor.is_none() && let Some(dir) = &dir && let Some(value) = disk_read(dir, &job.key) {
            publish(&value, true);
        }
        let path = match &job.cursor {
            Some(cursor) => format!("/api/session/{}/message?limit={}&cursor={}", job.key.id, 80 - LATEST, super::switcher::encode(cursor)),
            None => format!("/api/session/{}/message?limit={LATEST}&order=desc", job.key.id),
        };
        match fetch(&path) {
            Ok(value) => {
                publish(&value, false);
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

fn disk_write(dir: &std::path::Path, key: &Key, value: &Value) {
    let Some(path) = file(dir, key) else { return };
    let Ok(bytes) = serde_json::to_vec(&json!({"id": key.id, "updated": key.updated, "page": value})) else { return };
    if bytes.len() > PAGE_BYTES || std::fs::create_dir_all(dir).is_err() { return; }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_err() { return; }
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_err() || std::fs::rename(&tmp, &path).is_err() { return; }
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
            self.route = opening.from;
            tx.settle();
        }
    }

    pub fn begin_open(&mut self, id: &str, tx: &mut rune::Tx) {
        self.close_picker(false);
        self.activity_open = false;
        self.opening = Some(Opening { id: id.into(), from: self.route.clone(), started: Instant::now(), clock: self.clock, loading: false, received: self.transcripts.contains_key(id), error:None });
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
            self.open_now(&id, tx);
            if let Some(page) = self.pages.as_ref().and_then(|p| p.peek(&id)) { self.schedule_history(&page, tx); }
            return true;
        }
        // The fixture/story clock is deterministic; an idle live window must use wall time,
        // because its last poll may have been minutes before this click.
        let expired = if self.live() { opening.started.elapsed() >= HOLD } else { self.clock >= opening.clock + HOLD.as_secs_f64() };
        if expired && !opening.loading {
            self.opening.as_mut().unwrap().loading = true;
            self.open_now(&id, tx);
            return true;
        }
        false
    }

    pub fn loading_session(&self, id: &str) -> bool {
        self.opening.as_ref().is_some_and(|o| o.id == id && o.loading)
    }

    fn page_key(&self, id: &str) -> Key {
        let updated = self.session(id).or_else(|| self.switcher.loaded.iter().find(|s| s.id == id)).map_or(0, |s| s.updated);
        Key { id: id.to_owned(), updated }
    }

    pub fn prefetch_session(&self, id: &str) {
        if let Some(pages) = &self.pages { pages.request(self.page_key(id), false); }
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
        let Some(pages) = &self.pages else { return };
        let ready = pages.peek(id);
        pages.request(self.page_key(id), true);
        if let Some(page) = ready {
            if !self.transcripts.contains_key(id) { self.show_page(&page, tx); }
            else { self.schedule_history(&page, tx); }
        }
    }

    pub(crate) fn show_page(&mut self, page: &Page, tx: &mut rune::Tx) {
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
        if first || page.history { tx.settle(); }
        if page.history { self.page_history_loaded.insert(id.clone()); }
        self.schedule_history(page, tx);
    }

    fn schedule_history(&mut self, page: &Page, tx: &mut rune::Tx) {
        let id = &page.key.id;
        if self.route == super::Route::Session(id.clone()) && !page.history && !page.cached && !self.page_history_loaded.contains(id) && let Some(cursor) = &page.cursor {
            // Give the latest page its first frame before expanding history.
            self.page_history = Some((Instant::now() + Duration::from_millis(50), page.key.clone(), cursor.clone()));
            tx.wake_after(Duration::from_millis(50));
        }
    }

    pub fn poll_pages(&mut self, tx: &mut rune::Tx) -> bool {
        let results: Vec<_> = self.pages.as_ref().map(|p| p.results.try_iter().collect()).unwrap_or_default();
        let mut changed = false;
        for result in results {
            match result {
                Ok(page) if self.route == super::Route::Session(page.key.id.clone()) || self.opening.as_ref().is_some_and(|o| o.id == page.key.id) || (page.history && self.transcripts.contains_key(&page.key.id)) => { self.show_page(&page, tx); changed = true; }
                Err((key, error)) if self.opening.as_ref().is_some_and(|o| o.id == key.id) => {
                    self.status = error.clone();
                    self.opening.as_mut().unwrap().error = Some(error);
                    changed = true;
                }
                Err((key, error)) if self.route == super::Route::Session(key.id.clone()) => { self.status = error; changed = true; }
                _ => {}
            }
        }
        changed |= self.poll_open(tx);
        if self.page_history.as_ref().is_some_and(|(at, _, _)| *at <= Instant::now()) {
            let (_, key, cursor) = self.page_history.take().unwrap();
            if self.route == super::Route::Session(key.id.clone()) && let Some(pages) = &self.pages { pages.history(key, cursor); }
        }
        changed
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
        app.pages = Some(pages);
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
        let mut p = Page { key: key("ses_a"), messages: model::parse_messages(&page("new")), cursor: None, cached: false, history: false, images: vec![], bytes: 0 };
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
        pages.history(key("ses_a"), "older+/=".into());
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
        app.pages = Some(pages);
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
        let result = |id: &str| Arc::new(Page { key:key(id), messages:model::parse_messages(&page("All ready")), cursor:None, cached:false, history:false, images:vec![], bytes:0 });
        send.send(Ok(result("ses_slow"))).unwrap();
        rt.update(|s, tx| { s.poll_pages(tx); });
        rt.frame(1.0/60.0);
        assert!(rt.state.opening.is_none());
        assert!(!rt.elements().iter().any(|e| e.text.as_deref() == Some("Loading messages…")));

        rt.update(|s, tx| { s.open("ses_b", tx); s.open("ses_c", tx); });
        send.send(Ok(result("ses_b"))).unwrap();
        rt.update(|s, tx| { s.poll_pages(tx); });
        assert_eq!(rt.state.route, Route::Session("ses_slow".into()));
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
        app.pages = Some(pages);
        if prefetched {
            app.prefetch_session("ses_target");
            release.send(()).unwrap();
            let p = receive(app.pages.as_ref().unwrap());
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
                let p = receive(rt.state.pages.as_ref().unwrap());
                rt.update(|s, tx| s.show_page(&p, tx));
            }
            if frame == 10 {
                let history = Page { key: key("ses_target"), messages: (0..24).map(|i| Message::User {
                    id:format!("old_{i}"), text:format!("Earlier discussion {i}. {}", "A long note about the prior revision. ".repeat(12)),
                }).collect(), cursor:None, cached:false, history:true, images:vec![], bytes:0 };
                rt.update(|s, tx| s.show_page(&history, tx));
            }
            if frame == 6 { for image in rt.state.attach.image_handles() { image.wait().unwrap(); } }
            rt.update(|s, tx| { s.clock = frame as f64 / 60.0; s.poll_pages(tx); });
            rt.frame(1.0/60.0);
            let elements = rt.elements();
            let loading = elements.iter().any(|e| e.text.as_deref() == Some("Loading messages…"));
            let target = rt.state.route == Route::Session("ses_target".into());
            let grey = target && rt.state.attach.image_handles().iter().any(|img| !img.is_ready());
            let zero = target && elements.iter().any(|e| e.path.contains("/usage/") && e.label.as_deref() == Some("0"));
            let at_bottom = rt.scroll_at_bottom("scroll-ses_target");
            let image = elements.iter().find(|e| e.path.ends_with("/img-0") && !e.exiting).map(|e| e.rect);
            println!("OPEN_FRAME warm={prefetched} frame={frame} target={target} loading={loading} grey={grey} zero={zero} bottom={at_bottom:?} image={image:?}");
            if loading || grey || zero || (target && at_bottom == Some(false)) { bad.push(frame); }
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
