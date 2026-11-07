//! The hot-reload watcher: a background thread that notices a plugin library rebuilt on disk
//! and marks its slot for `PluginHost::poll`. It reads file metadata only, and wakes the
//! window only on a change, so an idle window still draws nothing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use super::vtable::{WakeShared, slot_bit};

const TICK: Duration = Duration::from_millis(250);

type Stamp = Option<(u64, SystemTime)>;

fn stamp(path: &PathBuf) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

pub struct Watcher {
    stop: Arc<AtomicBool>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl Watcher {
    /// Watches each `(slot, library)`; a change counts once the file holds still for a tick,
    /// so a half-written library is never loaded.
    pub fn spawn(wake: Arc<WakeShared>, libraries: Vec<(usize, PathBuf)>) -> Watcher {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let mut watched: Vec<(usize, PathBuf, Stamp, Stamp)> = libraries.into_iter().map(|(i, p)| {
            let s = stamp(&p);
            (i, p, s, s)
        }).collect();
        let spawned = std::thread::Builder::new().name("plugin-hot".into()).spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                std::thread::sleep(TICK);
                for (slot, path, loaded, last) in &mut watched {
                    let now = stamp(path);
                    if now != *loaded && now.is_some() && now == *last {
                        *loaded = now;
                        wake.wake(slot_bit(*slot), true);
                    }
                    *last = now;
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("plugin hot reload is off: {e}");
        }
        Watcher { stop }
    }
}
