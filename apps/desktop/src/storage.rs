//! The app's own state in one SQLite database, as Desktop keeps its (`packages/desktop/AGENTS.md:22`,
//! `src/main/storage/index.ts`): `opencode_storage` opens `opencode.sqlite` on its own thread as
//! the first thing `main` does, hands the first frame its state in one read (the boot packet),
//! and from then on every read comes from memory and every write goes behind, coalesced on the
//! storage thread. Nothing here names a file: the web build backs the same calls with browser
//! storage.
//!
//! The open's two stages meet the windows in [`Phase`]: the first frame is built from the boot
//! packet; once the first window has presented, stage B (SQLite itself, the cache of everything
//! else) is joined off the UI thread, writes made meanwhile are handed to it, and every window is
//! woken to take what the packet didn't hold (closed tabs, the other drafts, prompt history).

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use opencode_storage::{BootPacket, FlushReason, Lookup, Opening, Storage, WindowFrame};
use rune::Waker;
use serde_json::Value;

/// Where the open is.
enum Phase {
    /// Nothing opened: a run that keeps its state in memory (headless, fixtures, stories).
    Memory(Storage),
    /// `main` started the open; stage A hasn't been taken yet.
    Opening { opening: Opening },
    /// The first frames are built from `boot`; stage B still runs on the open's thread.
    Booted { boot: Arc<BootPacket>, opening: Opening, queued: Queued },
    /// The first window presented: a helper thread waits for stage B.
    Joining { boot: Arc<BootPacket>, queued: Queued },
    Ready(Storage),
}

/// What the phase machine asks for.
enum Effect {
    /// Wait for stage B on a thread of its own, then [`Event::Joined`].
    Join(Opening),
    /// Wait for stage B here (the app quits before it was joined).
    JoinNow(Opening),
    /// Hand the writes made while booting to the storage, tell it the first frame is up, wake every window.
    Adopt(Storage, Queued),
}

enum Event {
    /// A read or write before the first frame wants the packet.
    TakeBoot,
    /// The first window presented its first frame.
    Presented,
    Joined(Storage),
    /// The app quits: stage B is needed now.
    Quit,
}

/// The developer tools' own namespace (the storybook's folds), read once the first frame is up.
pub const DEV: &str = "rune.dev";

static PHASE: Mutex<Option<Phase>> = Mutex::new(None);
static READY: Condvar = Condvar::new();
/// Each window's waker, woken once the storage is ready.
static WAKERS: Mutex<Vec<Waker>> = Mutex::new(Vec::new());

fn step(phase: Phase, event: Event) -> (Phase, Option<Effect>) {
    match (phase, event) {
        (Phase::Opening { opening }, Event::TakeBoot | Event::Presented) => (Phase::Booted { boot: Arc::new(opening.boot()), opening, queued: Queued::default() }, None),
        (Phase::Opening { opening }, Event::Quit) => (Phase::Joining { boot: Arc::default(), queued: Queued::default() }, Some(Effect::JoinNow(opening))),
        (Phase::Booted { boot, mut opening, queued }, Event::Presented) => match opening.try_storage() {
            // Stage B is usually done long before the first frame: no thread to wait on it.
            Some((storage, _)) => (Phase::Ready(storage.clone()), Some(Effect::Adopt(storage, queued))),
            None => (Phase::Joining { boot, queued }, Some(Effect::Join(opening))),
        },
        (Phase::Booted { boot, opening, queued }, Event::Quit) => (Phase::Joining { boot, queued }, Some(Effect::JoinNow(opening))),
        (Phase::Joining { queued, .. }, Event::Joined(storage)) => (Phase::Ready(storage.clone()), Some(Effect::Adopt(storage, queued))),
        (phase, _) => (phase, None),
    }
}

/// Writes made before stage B was in hand, newest last; reads see them.
#[derive(Default)]
struct Queued {
    state: Vec<(String, String, Option<Arc<str>>)>,
    docs: Vec<(String, Option<String>)>,
    frames: Vec<(String, WindowFrame)>,
    forgotten: Vec<String>,
}

impl Queued {
    fn state(&self, name: &str, key: &str) -> Option<Option<Arc<str>>> {
        self.state.iter().rev().find(|(n, k, _)| n == name && k == key).map(|(_, _, v)| v.clone())
    }

    fn doc(&self, key: &str) -> Option<Option<String>> {
        self.docs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn apply(self, s: &Storage) {
        for (name, key, value) in self.state {
            match value {
                Some(v) => s.state().set(&name, &key, v),
                None => s.state().delete(&name, &key),
            };
        }
        for (key, value) in self.docs {
            s.documents().set(&key, value);
        }
        for (id, frame) in self.frames {
            s.windows().set_frame(&id, frame);
        }
        for id in self.forgotten {
            s.windows().forget(&id);
        }
    }
}

fn lock() -> MutexGuard<'static, Option<Phase>> {
    PHASE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Runs the machine on `event` and carries out what it asks.
fn send(event: Event) {
    let effect = {
        let mut guard = lock();
        let Some(phase) = guard.take() else { return };
        let (next, effect) = step(phase, event);
        *guard = Some(next);
        effect
    };
    match effect {
        Some(Effect::Join(opening)) => {
            let spawned = std::thread::Builder::new().name("opencode-storage-join".into()).spawn(move || send(Event::Joined(opening.storage().0)));
            if let Err(e) = spawned {
                eprintln!("opencode-storage: couldn't start the join thread: {e}");
            }
        }
        Some(Effect::JoinNow(opening)) => send(Event::Joined(opening.storage().0)),
        Some(Effect::Adopt(storage, queued)) => {
            queued.apply(&storage);
            storage.state().load(&[DEV], None);
            storage.first_frame_presented(Some(Box::new(wake_all)));
            READY.notify_all();
            wake_all();
        }
        None => {}
    }
}

fn wake_all() {
    for w in WAKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter() {
        w();
    }
}

/// `main`, first thing: opens the database on its own thread, or keeps this run in memory.
pub fn start() {
    let phase = match config() {
        Some(config) => Phase::Opening { opening: Storage::open_async(config) },
        None => Phase::Memory(Storage::memory()),
    };
    *lock() = Some(phase);
}

/// The database for a run with a person at it (or the harness standing in for one, under a
/// scratch root); none keeps the run in memory.
fn config() -> Option<opencode_storage::StorageConfig> {
    use opencode_storage::native::{ImportConfig, ImportGroup};
    if !crate::attended() || crate::arg("--fixture").is_some() || crate::demo::requested() {
        return None;
    }
    let data = crate::dirs::data()?;
    let mut sidecar = vec![opencode_kit::client::DEFAULT_SERVER.to_owned()];
    sidecar.extend(crate::arg("--server"));
    let context = opencode_kit::servers::migration::Context { sidecar, ..Default::default() };
    let mut import = ImportConfig::new(&data).context(context).groups(&[ImportGroup::App]);
    if let Some(updates) = crate::dirs::updates() {
        import = import.updates_dir(updates);
    }
    Some(opencode_storage::StorageConfig::native(data.join("opencode.sqlite"), crate::wall_ms).import(import).official(crate::dirs::official()))
}

/// The first window presented its first frame: stage B is joined off the UI thread.
pub fn presented() {
    send(Event::TakeBoot);
    send(Event::Presented);
}

/// Window `waker` is woken once the storage is ready (at once when it is).
pub fn on_ready(waker: Waker) {
    let ready = is_ready();
    WAKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(waker.clone());
    if ready {
        waker();
    }
}

/// Whether every stored value is in memory (stage B joined, or a run in memory).
pub fn is_ready() -> bool {
    matches!(&*lock(), Some(Phase::Ready(_) | Phase::Memory(_)) | None)
}

/// Runs `f` with the storage when it's ready; otherwise with the boot packet and the queue.
fn with<R>(ready: impl FnOnce(&Storage) -> R, booting: impl FnOnce(&BootPacket, &mut Queued) -> R) -> R {
    let mut guard = lock();
    if matches!(&*guard, Some(Phase::Opening { .. })) {
        drop(guard);
        send(Event::TakeBoot);
        guard = lock();
    }
    match &mut *guard {
        Some(Phase::Ready(s) | Phase::Memory(s)) => {
            let s = s.clone();
            drop(guard);
            ready(&s)
        }
        Some(Phase::Booted { boot, queued, .. } | Phase::Joining { boot, queued }) => {
            let boot = boot.clone();
            booting(&boot, queued)
        }
        Some(Phase::Opening { .. }) => unreachable!("the boot packet was just taken"),
        None => {
            let s = Storage::memory();
            *guard = Some(Phase::Memory(s.clone()));
            drop(guard);
            ready(&s)
        }
    }
}

/// A `state` value (JSON text): `name` is Desktop's storage name (`opencode.global.dat`, a
/// window's `opencode.window.<id>.dat`, `plugin.file`), `key` its key.
pub fn text(name: &str, key: &str) -> Option<Arc<str>> {
    with(|s| s.state().text(name, key), |boot, queued| queued.state(name, key).unwrap_or_else(|| boot.state(name, key).map(Arc::from)))
}

pub fn json(name: &str, key: &str) -> Option<Value> {
    serde_json::from_str(&text(name, key)?).ok()
}

/// Sets `name`/`key` to `value` (JSON text). An unchanged value writes nothing.
pub fn set(name: &str, key: &str, value: String) {
    put(name, key, Some(Arc::from(value)));
}

pub fn set_json(name: &str, key: &str, value: &Value) {
    set(name, key, value.to_string());
}

pub fn delete(name: &str, key: &str) {
    put(name, key, None);
}

fn put(name: &str, key: &str, value: Option<Arc<str>>) {
    with(
        |s| {
            match &value {
                Some(v) => s.state().set(name, key, v.clone()),
                None => s.state().delete(name, key),
            };
        },
        |boot, queued| {
            let current = queued.state(name, key).unwrap_or_else(|| boot.state(name, key).map(Arc::from));
            if current != value {
                queued.state.push((name.to_owned(), key.to_owned(), value.clone()));
            }
        },
    );
}

/// Changes part of a JSON value shared by several owners (`layout`, `server`): `f` edits the
/// stored value (an object when absent), and it's written only when it changed.
pub fn update(name: &str, key: &str, f: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let before = json(name, key).filter(Value::is_object).unwrap_or_else(|| Value::Object(Default::default()));
    let mut after = before.clone();
    if let Some(map) = after.as_object_mut() {
        f(map);
    }
    if after != before {
        set_json(name, key, &after);
    }
}

/// A document's JSON text (drafts, prompt history); `None` when absent, `Pending` while it waits
/// on disk ([`load_docs`]).
pub fn doc(key: &str) -> Lookup {
    with(
        |s| s.documents().get(key),
        |boot, queued| match queued.doc(key) {
            Some(v) => Lookup::Ready(v.map(Arc::from)),
            // The packet holds each window's shown draft; any other waits for stage B.
            None => match boot.document(key) {
                Some(v) => Lookup::Ready(Some(Arc::from(v))),
                None => Lookup::Pending,
            },
        },
    )
}

/// Sets or removes a document. An unchanged one writes nothing.
pub fn set_doc(key: &str, value: Option<String>) {
    with(
        |s| s.documents().set(key, value.clone()),
        |boot, queued| {
            let current = queued.doc(key).unwrap_or_else(|| boot.document(key).map(str::to_owned));
            if current != value {
                queued.docs.push((key.to_owned(), value.clone()));
            }
        },
    );
}

/// The documents stored under `prefix`; empty until the storage is ready.
pub fn doc_keys(prefix: &str) -> Vec<String> {
    with(|s| s.documents().keys(prefix), |_, _| Vec::new())
}

/// Reads documents into memory on the storage thread; `done` runs there once they're in (at
/// once when they are, or when nothing is stored).
pub fn load_docs(keys: &[&str], done: Box<dyn FnOnce() + Send>) {
    with(|s| s.documents().load(keys, Some(done)), |_, _| {});
}

/// The window ids saved at the last quit, in order; none when no list was ever saved.
pub fn window_ids() -> Option<Vec<String>> {
    let ids: Vec<String> = serde_json::from_str(&text("opencode.settings", "windowIds")?).ok()?;
    Some(ids.into_iter().filter(|id| !id.is_empty()).collect())
}

pub fn save_window_ids(ids: &[String]) {
    set_json("opencode.settings", "windowIds", &serde_json::json!(ids));
}

/// Window `id`'s frame as last saved.
pub fn frame(id: &str) -> Option<WindowFrame> {
    with(|s| s.windows().frame(id), |boot, queued| queued.frames.iter().rev().find(|(i, _)| i == id).map(|(_, f)| *f).or_else(|| boot.frame(id)))
}

/// Records window `id`'s frame (coalesced; call when a move or resize settles and on close).
pub fn set_frame(id: &str, frame: WindowFrame) {
    with(
        |s| s.windows().set_frame(id, frame),
        |boot, queued| {
            if queued.frames.iter().rev().find(|(i, _)| i == id).map(|(_, f)| *f).or_else(|| boot.frame(id)) != Some(frame) {
                queued.frames.push((id.to_owned(), frame));
            }
        },
    );
}

/// Window `id` closed for good: its frame, tabs, panes and drafts go (Desktop's `registry.closed`).
pub fn forget_window(id: &str) {
    with(|s| s.windows().forget(id), |_, queued| queued.forgotten.push(id.to_owned()));
}

/// Window `id`'s own namespace (`opencode.window.<id>.dat`), every row of it: what the packet
/// holds before stage B, everything after.
pub fn window_rows(id: &str) -> BTreeMap<String, Arc<str>> {
    let name = opencode_storage::window_namespace(id);
    with(
        |s| s.state().items(&name).unwrap_or_default().into_iter().collect(),
        |boot, queued| {
            let mut rows: BTreeMap<String, Arc<str>> = boot.namespace(&name).map(|(k, v)| (k.to_owned(), Arc::from(v))).collect();
            for (n, k, v) in &queued.state {
                if *n == name {
                    match v {
                        Some(v) => rows.insert(k.clone(), v.clone()),
                        None => rows.remove(k),
                    };
                }
            }
            rows
        },
    )
}

/// Lands every queued write. `wait`: block until it's durable (close, quit, session end), else
/// ask the writer and go on (a window hidden).
pub fn flush(reason: FlushReason, wait: bool) {
    if matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart) {
        send(Event::Quit);
        let mut guard = lock();
        while matches!(&*guard, Some(Phase::Joining { .. })) {
            guard = READY.wait_timeout(guard, Duration::from_secs(5)).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }
    let storage = match &*lock() {
        Some(Phase::Ready(s)) => s.clone(),
        _ => return,
    };
    let deadline = if wait { Duration::from_secs(2) } else { Duration::ZERO };
    storage.flush(reason, deadline);
}
