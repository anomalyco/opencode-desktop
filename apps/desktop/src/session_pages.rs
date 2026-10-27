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
    bytes: usize,
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
        if !self.transcripts.contains_key(id) && let Some(page) = ready { self.show_page(&page, tx); }
    }

    fn show_page(&mut self, page: &Page, tx: &mut rune::Tx) {
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
        if first { tx.settle(); }
        if page.history { self.page_history_loaded.insert(id.clone()); }
        if !page.history && !page.cached && !self.page_history_loaded.contains(id) && let Some(cursor) = &page.cursor {
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
                Ok(page) if self.route == super::Route::Session(page.key.id.clone()) || (page.history && self.transcripts.contains_key(&page.key.id)) => { self.show_page(&page, tx); changed = true; }
                Err((key, error)) if self.route == super::Route::Session(key.id.clone()) => { self.status = error; changed = true; }
                _ => {}
            }
        }
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
}
