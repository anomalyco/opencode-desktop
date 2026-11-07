//! The hot-reload watcher: a background thread that notices a plugin library rebuilt on disk,
//! and does the slow part of a reload there: the shadow copy, `LoadLibrary`/`dlopen` (with the
//! library's own initialisers) and the ABI handshake. It hands the loaded library to the app's
//! thread and wakes it; `PluginHost::poll` then only swaps instances. It waits on the OS's file
//! change notifications, so at rest it runs nothing, and wakes the window only on a change.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::loader::{self, Loaded};
use super::vtable::{WakeShared, slot_bit};

const TICK: Duration = Duration::from_millis(250);

type Stamp = Option<(u64, SystemTime)>;

fn stamp(path: &PathBuf) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// A library the watcher loaded, waiting for the app's thread.
pub struct Prepared {
    pub loaded: Result<Loaded, String>,
    /// How long the copy, load and handshake took on the watcher's thread.
    pub took: Duration,
}

/// One library to watch: its slot, path and plugin id.
pub struct Watch {
    pub slot: usize,
    pub path: PathBuf,
    pub id: String,
}

/// Libraries loaded off the app's thread, waiting for it to swap them in.
pub type Ready = Arc<Mutex<Vec<(usize, Prepared)>>>;

fn prepare(path: &std::path::Path, shadow_dir: Option<&std::path::Path>, id: &str) -> Prepared {
    let started = Instant::now();
    Prepared { loaded: loader::load(path, shadow_dir, id), took: started.elapsed() }
}

fn deliver(ready: &Ready, wake: &WakeShared, slot: usize, prepared: Prepared) {
    if let Ok(mut ready) = ready.lock() {
        ready.push((slot, prepared));
    }
    wake.wake(slot_bit(slot), true);
}

/// Loads slot `slot`'s library on a plugin worker and hands it to the app's thread, as the
/// watcher does (a library the user just trusted).
pub fn load_in_background(wake: Arc<WakeShared>, ready: Ready, slot: usize, path: PathBuf, shadow_dir: Option<PathBuf>, id: String) {
    super::tasks::run(move || deliver(&ready, &wake, slot, prepare(&path, shadow_dir.as_deref(), &id)));
}

/// The newest library loaded for slot `slot`, if any; older ones are dropped.
pub fn take(ready: &Ready, slot: usize) -> Option<Prepared> {
    let mut list = ready.lock().ok()?;
    let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *list).into_iter().partition(|(s, _)| *s == slot);
    *list = rest;
    drop(list);
    mine.into_iter().map(|(_, p)| p).next_back()
}
/// Keeps the watch on; dropping it ends the watcher's thread.
pub struct Watcher {
    _events: notify::RecommendedWatcher,
}

impl Watcher {
    /// Watches each library's folder with the OS's change notifications, so at rest the thread
    /// sleeps in a blocking wait and costs nothing. After a change it checks every tick until
    /// the file holds still, so a half-written library is never loaded.
    pub fn spawn(wake: Arc<WakeShared>, ready: Ready, libraries: Vec<Watch>, shadow_dir: Option<PathBuf>) -> Option<Watcher> {
        use notify::Watcher as _;
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let paths: Vec<PathBuf> = libraries.iter().map(|w| w.path.clone()).collect();
        let handler = move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|e| e.paths.iter().any(|p| paths.iter().any(|w| w == p))) {
                tx.send(()).ok();
            }
        };
        let mut events = notify::recommended_watcher(handler).map_err(|e| eprintln!("plugin hot reload is off: {e}")).ok()?;
        let folders: std::collections::BTreeSet<PathBuf> = libraries.iter().filter_map(|w| w.path.parent().map(PathBuf::from)).collect();
        for folder in &folders {
            if let Err(e) = events.watch(folder, notify::RecursiveMode::NonRecursive) {
                eprintln!("plugin hot reload can't watch {}: {e}", folder.display());
            }
        }
        let mut watched: Vec<(Watch, Stamp)> = libraries.into_iter().map(|w| {
            let s = stamp(&w.path);
            (w, s)
        }).collect();
        let spawned = std::thread::Builder::new().name("plugin-hot".into()).spawn(move || {
            // Ends when the watch drops its sender.
            while rx.recv().is_ok() {
                let mut last: Vec<Stamp> = watched.iter().map(|(w, _)| stamp(&w.path)).collect();
                loop {
                    std::thread::sleep(TICK);
                    while rx.try_recv().is_ok() {}
                    let mut settling = false;
                    for ((w, loaded), last) in watched.iter_mut().zip(&mut last) {
                        let now = stamp(&w.path);
                        if now != *last {
                            settling = true;
                        } else if now != *loaded && now.is_some() {
                            *loaded = now;
                            deliver(&ready, &wake, w.slot, prepare(&w.path, shadow_dir.as_deref(), &w.id));
                        }
                        *last = now;
                    }
                    if !settling {
                        break;
                    }
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("plugin hot reload is off: {e}");
            return None;
        }
        Some(Watcher { _events: events })
    }
}