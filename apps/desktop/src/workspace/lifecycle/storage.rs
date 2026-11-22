//! The app's own state in one SQLite database, as Desktop keeps its (`packages/desktop/AGENTS.md:22`,
//! `src/main/storage/index.ts`): `opencode_storage` opens `opencode.sqlite` on its own thread as
//! the first thing `main` does, hands the first frame its state in one read (the boot packet),
//! and from then on every read comes from memory and every write goes behind, coalesced on the
//! storage thread, at its next turn: every state, document and window row set here asks for a
//! kill-safe commit (`Storage::commit_soon`), so a kill loses at most the change in flight, and
//! the writes of one turn share one commit. Nothing here names a file: the web build backs the
//! same calls with browser storage.
//!
//! The open's two stages meet the windows in [`Phase`]: the first frame is built from the boot
//! packet; once the first window has presented, stage B (SQLite itself, the cache of everything
//! else) is joined off the UI thread, writes made meanwhile are handed to it, and every window is
//! woken to take what the packet didn't hold (closed tabs, the other drafts, prompt history).

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use opencode_storage::{AppStorage, BootPacket, FlushReason, ImportGroup, Lookup, Opening, PluginRecord, Storage, StorageConfig, WindowFrame};
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
    /// Stage B was done when a read the boot packet couldn't answer needed it, before the first
    /// frame: everything reads from it, and what waits for the first frame (the import's file
    /// moves, the seed) still waits. The boot packet stays until then for what the first frame
    /// draws from it alone (the shown sessions' previews).
    Early { storage: Storage, boot: Arc<BootPacket> },
    Ready(Storage),
}

/// What the phase machine asks for.
enum Effect {
    /// Wait for stage B on a thread of its own, then [`Event::Joined`].
    Join(Opening),
    /// Wait for stage B here (the app quits before it was joined).
    JoinNow(Opening),
    /// Hand the writes made while booting to the storage; with why the database didn't open, when
    /// it didn't. Once the first frame is up ([`Frame::Presented`]), tell the storage and wake every
    /// window.
    Adopt(Storage, Queued, Opened, Frame),
    /// The first frame is up after an early adoption: tell the storage, wake every window.
    Presented(Storage),
}

/// How the database opened, for what the windows are told.
enum Opened {
    Saving { drafts_replayed: bool, dropped_drafts: Option<(usize, String)> },
    /// It couldn't (why): this run keeps its changes in memory.
    Memory(String),
    /// Another copy of the app has the profile: this run keeps its changes in memory.
    InUse,
    /// It was damaged: moved aside (here) and a new one made.
    Fresh(std::path::PathBuf),
}

impl Opened {
    fn of(report: opencode_storage::BootReport) -> Opened {
        match (report.error, report.damaged) {
            (Some(why), _) if why == opencode_storage::IN_USE => Opened::InUse,
            (Some(why), _) => Opened::Memory(why),
            (None, Some(kept)) => Opened::Fresh(kept),
            (None, None) => {
                #[cfg(not(target_arch = "wasm32"))]
                let dropped_drafts = (report.import.seeded_dropped_drafts > 0).then(|| (report.import.seeded_dropped_drafts, report.import.seeded_source_dir.as_ref().map_or_else(String::new, |p| p.display().to_string())));
                #[cfg(target_arch = "wasm32")]
                let dropped_drafts = None;
                Opened::Saving { drafts_replayed: !report.created && report.journal_replayed, dropped_drafts }
            }
        }
    }
}

/// Whether the first frame was presented when the storage was adopted.
enum Frame { Presented, Pending }

enum Event {
    /// A read or write before the first frame wants the packet.
    TakeBoot,
    /// The first window presented its first frame.
    Presented,
    Joined(Storage, Opened),
    /// The app quits: stage B is needed now.
    Quit,
    /// A read the boot packet can't answer: adopt stage B if it's done (never wait for it here).
    Need,
}

/// The developer tools' own namespace (the storybook's folds and widths), in the boot packet.
pub const DEV: &str = "rune.dev";

static PHASE: Mutex<Option<Phase>> = Mutex::new(None);
static READY: Condvar = Condvar::new();
/// What the windows are told about where their state is, once, by the first window to take it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// The database couldn't open (why): this run keeps its changes in memory.
    Memory(String),
    /// Another copy of OpenCode has this profile open: this run keeps its changes in memory.
    InUse,
    /// Another copy of OpenCode is finishing a save: the windows show the last save until it has.
    Waiting,
    /// The saved state was damaged: it's kept at this path, and the app started a new one.
    Damaged(String),
    /// Another process holds the file: the windows show its last checkpoint, and nothing is saved.
    Checkpoint,
    /// Another process holds the file and won't let it be read: a first-run screen; the saved
    /// state is untouched, and nothing is saved.
    FirstRun,
    /// The file came free and still holds what the windows show, or writes that failed succeed
    /// again: changes are saved again.
    Saving,
    /// Writes to the database fail (`opencode_storage::write_trouble`): what changes stays in memory
    /// and is written once a write succeeds. `full` when the disk is full, else the failure's words.
    NotSaving { full: bool, why: String },
    /// Some Official Desktop drafts could not be migrated (missing chunk/attachment blob).
    SeedDroppedDrafts { count: usize, official_dir: String },
}

/// Hand-over of a boot-time storage notice (and whether drafts replayed) to the first window.
enum BootNotice {
    /// Before stage B adopts, and the boot packet was not held.
    Booting,
    /// The first frame showed a held file's stand-in; its notice has not been taken yet.
    HeldOwed(Notice),
    /// The first frame showed a held file's stand-in; its notice was already taken.
    HeldTold,
    /// Stage B adopted and owes the first window one notice (never coincides with replayed drafts).
    Owed(Notice),
    /// Stage B adopted with no boot notice left; no existing draft journal replayed.
    Settled,
    /// Stage B adopted cleanly after replaying an existing draft journal; no boot notice owed.
    DraftsReplayed,
}

enum BootNoticeEvent {
    Held(Notice),
    Adopted { opened: Opened, held_by_another_copy: bool },
    Take,
}

impl BootNotice {
    fn drafts_replayed(&self) -> bool { matches!(self, Self::DraftsReplayed) }

    fn step(&mut self, event: BootNoticeEvent) -> Option<Notice> {
        match (std::mem::replace(self, Self::Settled), event) {
            (Self::Booting, BootNoticeEvent::Held(notice)) => { *self = Self::HeldOwed(notice); None }
            (Self::Booting, BootNoticeEvent::Adopted { opened, .. }) => {
                *self = match opened {
                    Opened::Memory(why) => Self::Owed(Notice::Memory(why)),
                    Opened::InUse => Self::Owed(Notice::InUse),
                    Opened::Fresh(kept) => Self::Owed(Notice::Damaged(kept.display().to_string())),
                    Opened::Saving { dropped_drafts: Some((count, official_dir)), .. } => Self::Owed(Notice::SeedDroppedDrafts { count, official_dir }),
                    Opened::Saving { drafts_replayed: true, dropped_drafts: None } => Self::DraftsReplayed,
                    Opened::Saving { drafts_replayed: false, dropped_drafts: None } => Self::Settled,
                };
                None
            }
            (held @ (Self::HeldOwed(_) | Self::HeldTold), BootNoticeEvent::Adopted { opened, held_by_another_copy }) => {
                let kept_notice = match held { Self::HeldOwed(n) => Some(n), _ => None };
                *self = match opened {
                    // Held for another copy finishing a save ("OpenCode is finishing a save"), and the file
                    // changed under it: now it says this window won't save.
                    Opened::Memory(_) if held_by_another_copy => Self::Owed(Notice::InUse),
                    // The first frame's notice already says nothing is saved.
                    Opened::Memory(_) => kept_notice.map_or(Self::Settled, Self::Owed),
                    // In its own words, over a held frame's notice that blames another program.
                    Opened::InUse => Self::Owed(Notice::InUse),
                    Opened::Fresh(kept) => Self::Owed(Notice::Damaged(kept.display().to_string())),
                    Opened::Saving { .. } => Self::Owed(Notice::Saving),
                };
                None
            }
            (Self::HeldOwed(n), BootNoticeEvent::Take) => { *self = Self::HeldTold; Some(n) }
            (Self::Owed(n), BootNoticeEvent::Take) => { *self = Self::Settled; Some(n) }
            (state, _) => { *self = state; None }
        }
    }
}

/// What the windows were last told about ongoing database writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteStatus { Saving, Failed, DiskFull }

impl WriteStatus {
    fn step(&mut self, trouble: Option<opencode_storage::WriteTrouble>) -> Option<Notice> {
        let next = match &trouble {
            None => Self::Saving,
            Some(opencode_storage::WriteTrouble::Failed(_)) => Self::Failed,
            Some(opencode_storage::WriteTrouble::DiskFull) => Self::DiskFull,
        };
        if next == *self { return None; }
        let was = std::mem::replace(self, next);
        match trouble {
            Some(opencode_storage::WriteTrouble::DiskFull) => Some(Notice::NotSaving { full: true, why: String::new() }),
            Some(opencode_storage::WriteTrouble::Failed(why)) => Some(Notice::NotSaving { full: false, why }),
            None if was != Self::Saving => Some(Notice::Saving),
            None => None,
        }
    }
}

/// The boot-time notice hand-over and ongoing write status owed to the windows.
struct Told {
    boot: BootNotice,
    writes: WriteStatus,
}

static TOLD: Mutex<Told> = Mutex::new(Told { boot: BootNotice::Booting, writes: WriteStatus::Saving });

fn told() -> MutexGuard<'static, Told> {
    TOLD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
/// Each window's waker, woken once the storage is ready.
static WAKERS: Mutex<Vec<Waker>> = Mutex::new(Vec::new());
/// The boot packet's size and, of it, the plugin values', for the launch trace.
#[cfg(not(target_arch = "wasm32"))]
static BOOT_BYTES: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();

fn step(phase: Phase, event: Event) -> (Phase, Option<Effect>) {
    match (phase, event) {
        (Phase::Opening { opening }, Event::TakeBoot | Event::Presented) => {
            let boot = Arc::new(opening.boot());
            // The UI thread holds the boot packet (it waited for it here, if it had to).
            rune::marks::mark("storage.boot");
            if boot.source.held() {
                let notice = match boot.source {
                    opencode_storage::BootSource::Checkpoint | opencode_storage::BootSource::Denied if opencode_storage::held_by_another_copy() => Notice::Waiting,
                    opencode_storage::BootSource::Checkpoint => Notice::Checkpoint,
                    _ => Notice::FirstRun,
                };
                told().boot.step(BootNoticeEvent::Held(notice));
            }
            (Phase::Booted { boot, opening, queued: Queued::default() }, None)
        }
        (Phase::Opening { opening }, Event::Quit) => (Phase::Joining { boot: Arc::default(), queued: Queued::default() }, Some(Effect::JoinNow(opening))),
        (Phase::Booted { boot, mut opening, queued }, Event::Presented) => match opening.try_storage() {
            // Stage B is usually done long before the first frame: no thread to wait on it.
            Some((storage, report)) => (Phase::Ready(storage.clone()), Some(Effect::Adopt(storage, queued, Opened::of(report), Frame::Presented))),
            None => (Phase::Joining { boot, queued }, Some(Effect::Join(opening))),
        },
        // Another process holds the file and the windows show its stand-in: nothing this run
        // changed is saved, so the quit doesn't wait for the open (it may wait out the lock).
        (Phase::Booted { boot, .. } | Phase::Joining { boot, .. }, Event::Quit) if boot.source.held() => (Phase::Memory(Storage::memory()), None),
        (Phase::Booted { boot, opening, queued }, Event::Quit) => (Phase::Joining { boot, queued }, Some(Effect::JoinNow(opening))),
        (Phase::Booted { boot, mut opening, queued }, Event::Need) => match opening.try_storage() {
            Some((storage, report)) => (Phase::Early { storage: storage.clone(), boot }, Some(Effect::Adopt(storage, queued, Opened::of(report), Frame::Pending))),
            None => (Phase::Booted { boot, opening, queued }, None),
        },
        (Phase::Joining { queued, .. }, Event::Joined(storage, opened)) => (Phase::Ready(storage.clone()), Some(Effect::Adopt(storage, queued, opened, Frame::Presented))),
        (Phase::Early { storage, .. }, Event::Presented) => (Phase::Ready(storage.clone()), Some(Effect::Presented(storage))),
        (phase, _) => (phase, None),
    }
}

type QueuedPreview = (String, String, Option<(i64, Arc<[u8]>)>);
type StorageTask = Box<dyn FnOnce(&Storage) + Send>;

/// Writes made before stage B was in hand, newest last; reads see them.
#[derive(Default)]
struct Queued {
    state: Vec<(String, String, Option<Arc<str>>)>,
    docs: Vec<(String, Option<String>, u64)>,
    frames: Vec<(String, WindowFrame)>,
    forgotten: Vec<String>,
    previews: Vec<QueuedPreview>,
    /// Work that needs the storage itself (the outbox's reads and Tier 0 rows), in order.
    later: Vec<StorageTask>,
}

impl Queued {
    fn state(&self, name: &str, key: &str) -> Option<Option<Arc<str>>> {
        self.state.iter().rev().find(|(n, k, _)| n == name && k == key).map(|(_, _, v)| v.clone())
    }

    fn doc(&self, key: &str) -> Option<Option<String>> {
        self.docs.iter().rev().find(|(k, ..)| k == key).map(|(_, v, _)| v.clone())
    }

    fn apply(self, s: &Storage) {
        for (name, key, value) in self.state {
            match value {
                Some(v) => s.state().set(&name, &key, v),
                None => s.state().delete(&name, &key),
            };
        }
        for (key, value, rev) in self.docs {
            s.documents().set_rev(&key, value, rev);
        }
        for (id, frame) in self.frames {
            s.windows().set_frame(&id, frame);
        }
        for id in self.forgotten {
            s.windows().forget(&id);
        }
        for (server, session, value) in self.previews {
            s.previews().set(&server, &session, value);
        }
        for f in self.later {
            f(s);
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
        let (next, mut effect) = step(phase, event);
        // Before the lock goes: a window that sees the storage ready sees why it's in memory too.
        if let Some(Effect::Adopt(_, _, opened, _)) = &mut effect {
            let was_opened = std::mem::replace(opened, Opened::Saving { drafts_replayed: false, dropped_drafts: None });
            let held_by_another_copy = matches!(was_opened, Opened::Memory(_)) && opencode_storage::held_by_another_copy();
            told().boot.step(BootNoticeEvent::Adopted { opened: was_opened, held_by_another_copy });
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Phase::Booted { boot, .. } = &next {
            BOOT_BYTES.get_or_init(|| (boot.len(), boot.plugin_bytes()));
        }
        *guard = Some(next);
        effect
    };
    match effect {
        Some(Effect::Join(opening)) => {
            let spawned = std::thread::Builder::new().name("opencode-storage-join".into()).spawn(move || joined(opening));
            if let Err(e) = spawned {
                eprintln!("opencode-storage: couldn't start the join thread: {e}");
            }
        }
        Some(Effect::JoinNow(opening)) => joined(opening),
        Some(Effect::Adopt(storage, queued, _, frame)) => {
            queued.apply(&storage);
            READY.notify_all();
            if matches!(frame, Frame::Presented) {
                storage.first_frame_presented(Some(Box::new(wake_all)));
                wake_all();
            }
        }
        Some(Effect::Presented(storage)) => {
            storage.first_frame_presented(Some(Box::new(wake_all)));
            wake_all();
        }
        None => {}
    }
}

fn joined(opening: Opening) {
    let (storage, report) = opening.storage();
    send(Event::Joined(storage, Opened::of(report)));
}

/// What the windows must be told about their state, once: the first window to take it says so.
/// A run in memory by design (headless, fixtures) has nothing to tell.
pub fn take_notice() -> Option<Notice> {
    let mut t = told();
    if let Some(n) = t.boot.step(BootNoticeEvent::Take) {
        return Some(n);
    }
    // Writes that start or stop failing, told once each way (by the first window to poll).
    t.writes.step(opencode_storage::write_trouble())
}

/// This process is the app's one instance: the open may take the database's lock now (the
/// record's raw read ran beside the claim).
#[cfg(not(target_arch = "wasm32"))]
pub fn claimed() {
    rune::marks::mark("instance.claimed");
    if let Some(Phase::Opening { opening } | Phase::Booted { opening, .. }) = &*lock() {
        opening.proceed();
    }
}

/// True when an existing draft journal from the previous run attached and replayed cleanly.
pub fn drafts_replayed() -> bool {
    told().boot.drafts_replayed()
}

pub(crate) fn wake_all() {
    for w in WAKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter() {
        w();
    }
}

/// `main`, first thing: opens the database on its own thread, or keeps this run in memory. The
/// boot record's raw read starts now, beside the instance claim that follows; SQLite's open, which
/// locks the file, waits for the claim ([`claimed`]), so a launch that hands off never takes it.
pub fn start() {
    let phase = match config() {
        Some(config) => Phase::Opening { opening: Storage::open_async(config) },
        None => Phase::Memory(Storage::memory()),
    };
    *lock() = Some(phase);
    rune::marks::mark("storage.started");
    rune::marks::mark("instance.claim");
}

/// The database for a run with a person at it (or the harness standing in for one, under a
/// scratch root); none keeps the run in memory.
fn config() -> Option<StorageConfig> {
    if !crate::attended() || crate::arg("--fixture").is_some() || crate::demo::requested() {
        return None;
    }
    let mut sidecar = vec![opencode_kit::client::DEFAULT_SERVER.to_owned()];
    sidecar.extend(crate::arg("--server"));
    // Every group of legacy files is cut over: the app reads and writes none of them.
    let groups = ImportGroup::ALL.to_vec();
    let config = StorageConfig::app(AppStorage { data: crate::dirs::data(), updates: crate::dirs::updates(), official: crate::dirs::official(), sidecar, groups, clock: crate::wall_ms })?;
    // SQLite's open waits for the instance claim (`claimed`); the record's raw read doesn't.
    Some(crate::glyph_masks::early(config.after_claim().on_packet(|| rune::marks::mark("storage.packet"))))
}

/// The first window presented its first frame: stage B is joined off the UI thread.
pub fn presented() {
    send(Event::TakeBoot);
    send(Event::Presented);
}

/// Joins stage B synchronously for non-realtime headless runs (which have no `on_ready` hook to
/// call [`presented`]), so `with_storage` tasks (`start_later`) never strand until process exit.
pub fn join_for_headless() {
    presented();
    let guard = lock();
    READY
        .wait_timeout_while(guard, Duration::from_secs(5), |g| matches!(g, Some(Phase::Joining { .. })))
        .ok();
}

/// Window `waker` is woken once the storage is ready (at once when it is).
pub fn on_ready(waker: Waker) {
    let ready = is_ready();
    WAKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(waker.clone());
    if ready {
        waker();
    }
}

/// Whether this run keeps its state in the database (not a run in memory).
#[cfg(not(target_arch = "wasm32"))]
pub fn on_disk() -> bool {
    !matches!(&*lock(), Some(Phase::Memory(_)) | None)
}

/// Whether every stored value is in memory (stage B joined, or a run in memory).
pub fn is_ready() -> bool {
    matches!(&*lock(), Some(Phase::Ready(_) | Phase::Early { .. } | Phase::Memory(_)) | None)
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
        Some(Phase::Ready(s) | Phase::Early { storage: s, .. } | Phase::Memory(s)) => {
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
            s.commit_soon();
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

/// Sets or removes a document. An unchanged one writes nothing. A draft goes into the draft
/// journal first (a memory copy), so a kill keeps it even while the database's commit waits.
pub fn set_doc(key: &str, value: Option<String>) {
    let rev = opencode_storage::journal::record(key, value.as_deref());
    put_doc(key, value, rev);
}

/// A draft whose words a prompt takes (`send_failed`): removed without a journal record, since
/// until the prompt's row is committed the words are only there. Returns the revision the row
/// settles ([`opencode_storage::OutboxPut::clear_documents`]).
pub fn hand_off_doc(key: &str) -> u64 {
    let rev = opencode_storage::journal::latest(key);
    put_doc(key, None, 0);
    rev
}

fn put_doc(key: &str, value: Option<String>, rev: u64) {
    with(
        |s| {
            s.documents().set_rev(key, value.clone(), rev);
            s.commit_soon();
        },
        |boot, queued| {
            let current = queued.doc(key).unwrap_or_else(|| boot.document(key).map(str::to_owned));
            if current != value {
                queued.docs.push((key.to_owned(), value.clone(), rev));
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
#[cfg(not(target_arch = "wasm32"))]
pub fn frame(id: &str) -> Option<WindowFrame> {
    with(|s| s.windows().frame(id), |boot, queued| queued.frames.iter().rev().find(|(i, _)| i == id).map(|(_, f)| *f).or_else(|| boot.frame(id)))
}

/// Records window `id`'s frame (coalesced; call when a move or resize settles and on close).
pub fn set_frame(id: &str, frame: WindowFrame) {
    with(
        |s| {
            s.windows().set_frame(id, frame);
            s.commit_soon();
        },
        |boot, queued| {
            if queued.frames.iter().rev().find(|(i, _)| i == id).map(|(_, f)| *f).or_else(|| boot.frame(id)) != Some(frame) {
                queued.frames.push((id.to_owned(), frame));
            }
        },
    );
}

/// Every row of a namespace the first frame doesn't draw (`plugin.terminal`); none until it's loaded
/// (`StateStore::load`, through [`with_storage`]).
pub fn items(name: &str) -> Option<Vec<(String, Arc<str>)>> {
    with(|s| s.state().items(name), |_, _| None)
}

/// Whether this run keeps its state in memory by design (headless, fixtures): everything is there.
pub fn in_memory() -> bool {
    matches!(&*lock(), Some(Phase::Memory(_)) | None)
}

/// Whether every row of namespace `name` is in memory (never before stage B).
pub fn is_loaded(name: &str) -> bool {
    with(|s| s.state().is_loaded(name), |_, _| false)
}

/// A plugin's value (a `plugin.<id>` row): from memory, or before stage B from the boot packet;
/// `Pending` when neither has it yet (a namespace not loaded, a value too large for the record).
pub fn plugin_value(name: &str, key: &str) -> Lookup {
    with(
        |s| s.state().get(name, key),
        |boot, queued| match queued.state(name, key) {
            Some(v) => Lookup::Ready(v),
            None => boot.plugin_value(name, key),
        },
    )
}

/// A read the boot packet can't answer wants stage B: adopted now if the open has finished,
/// never waited for. Whether the storage is in memory now.
pub fn need() -> bool {
    if !is_ready() {
        send(Event::TakeBoot);
        send(Event::Need);
    }
    is_ready()
}

/// The boot packet, while the first frames are built from it.
pub fn boot_packet() -> Option<Arc<BootPacket>> {
    match &*lock() {
        Some(Phase::Booted { boot, .. } | Phase::Joining { boot, .. }) => Some(boot.clone()),
        _ => None,
    }
}

/// The boot packet's bytes and, of them, the plugin values'.
#[cfg(not(target_arch = "wasm32"))]
pub fn boot_bytes() -> Option<(usize, usize)> {
    BOOT_BYTES.get().copied()
}

/// Every plugin's enabled override and trusted files: from memory, or the boot packet when it
/// knows them; none until stage B when it doesn't (the plugin files weren't imported yet).
pub fn plugin_records() -> Option<(BTreeMap<String, bool>, BTreeMap<String, String>)> {
    with(
        |s| Some((s.plugins().overrides().into_iter().collect(), s.plugins().trusted_all().into_iter().collect())),
        |boot, _| boot.plugins_covered().then(|| (boot.plugins().map(|(id, on)| (id.to_owned(), on)).collect(), boot.trusted().map(|(id, sha)| (id.to_owned(), sha.to_owned())).collect())),
    )
}

/// Writes plugin `id`'s enabled override and trust in one durable commit; `done` runs on the
/// storage thread once it is (or with why not).
pub fn plugin_record(id: &str, record: PluginRecord, done: opencode_storage::Done) {
    let id = id.to_owned();
    with_storage(move |s| s.plugins().record(&id, record, Some(done)));
}

/// Loads `names` (lazy namespaces: plugin stores) into memory on the storage thread; `done` runs
/// there once they're in (at once when they are).
pub fn load_state(names: Vec<String>, done: Box<dyn FnOnce() + Send>) {
    #[cfg(feature = "storage-delay")]
    let done: Box<dyn FnOnce() + Send> = {
        // Scratch profiles only (RUNE_DIRS_ROOT): never a real one.
        let ms = std::env::var_os("RUNE_DIRS_ROOT").and_then(|_| std::env::var("RUNE_STORAGE_DELAY_MS").ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let what = names.join(",");
        Box::new(move || {
            if ms > 0 {
                eprintln!("storage-delay: holding the reply for {what} {ms} ms");
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            done()
        })
    };
    with_storage(move |s| {
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        s.state().load(&names, Some(done));
    });
}

/// Runs `f` with the storage: now when it's ready, else once it is, after the writes made
/// meanwhile and in the order asked. For the stores that live only in the database (the
/// outbox's rows), never read before the first frame.
pub fn with_storage(f: impl FnOnce(&Storage) + Send + 'static) {
    let f: std::cell::Cell<Option<StorageTask>> = std::cell::Cell::new(Some(Box::new(f)));
    with(
        |s| {
            if let Some(f) = f.take() {
                f(s);
            }
        },
        |_, queued| queued.later.extend(f.take()),
    );
}

/// Window `id` closed for good: its frame, tabs, panes and drafts go (Desktop's `registry.closed`).
pub fn forget_window(id: &str) {
    with(
        |s| {
            s.windows().forget(id);
            s.commit_soon();
        },
        |_, queued| queued.forgotten.push(id.to_owned()),
    );
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

/// The session preview the boot packet holds for `session` on `server` (each window's shown
/// session, before the storage is ready): its `updated_at` and bytes.
pub fn boot_preview(server: &str, session: &str) -> Option<(i64, Vec<u8>)> {
    // Stage B may be in hand before the first frame (`Early`): the packet still answers for it.
    if let Some(Phase::Early { boot, .. }) = &*lock() {
        return boot.preview(server, session).map(|(at, b)| (at, b.to_vec()));
    }
    with(|_| None, |boot, _| boot.preview(server, session).map(|(at, b)| (at, b.to_vec())))
}

/// Keeps session `session`'s first-screen preview (written behind; unchanged writes nothing).
pub fn set_preview(server: &str, session: &str, value: Option<(i64, Arc<[u8]>)>) {
    with(|s| s.previews().set(server, session, value.clone()), |_, queued| queued.previews.push((server.to_owned(), session.to_owned(), value.clone())));
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
    let storage = match &*lock() { Some(Phase::Ready(s) | Phase::Early { storage: s, .. }) => s.clone(), _ => return };
    let deadline = if wait { Duration::from_secs(2) } else { Duration::ZERO };
    storage.flush(reason, deadline);
}

/// The app is ending: every queued row lands, the WAL is checkpointed when it holds writes, and
/// the database closes, so its exclusive lock goes now rather than when the process is gone (an
/// update's helper, the next launch). Later reads and writes stay in memory.
#[cfg(not(target_arch = "wasm32"))]
pub fn close() {
    crate::launch::last_exit::mark_clean();
    flush(FlushReason::Quit, true);
    let storage = match &*lock() { Some(Phase::Ready(s) | Phase::Early { storage: s, .. }) => s.clone(), _ => return };
    storage.close();
}
