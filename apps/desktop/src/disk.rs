//! The small state files (drafts, tabs, zoom, settings) are written on one background thread,
//! so a keystroke or a tab click never waits on the disk. Each slot keeps only its newest
//! pending write, and slots run in the order they were first asked for. `flush` waits for the
//! queue to empty, when the window closes and after `run` returns.

use std::borrow::Cow;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Queue {
    pending: Vec<(Cow<'static, str>, Job)>,
    busy: bool,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;

static QUEUE: OnceLock<Shared> = OnceLock::new();

fn queue() -> &'static Shared {
    QUEUE.get_or_init(|| {
        let q: Shared = Arc::default();
        let worker = q.clone();
        std::thread::Builder::new().name("opencode-saves".into()).spawn(move || run(&worker)).expect("spawn saves thread");
        q
    })
}

fn run(q: &Shared) {
    let (lock, cv) = &**q;
    let mut queue = lock.lock().unwrap();
    loop {
        while queue.pending.is_empty() {
            queue = cv.wait(queue).unwrap();
        }
        let (_, job) = queue.pending.remove(0);
        queue.busy = true;
        drop(queue);
        job();
        queue = lock.lock().unwrap();
        queue.busy = false;
        cv.notify_all();
    }
}

/// Runs `job` off the UI thread, replacing `slot`'s write if one is still waiting.
pub fn later(slot: impl Into<Cow<'static, str>>, job: impl FnOnce() + Send + 'static) {
    let slot = slot.into();
    let (lock, cv) = &**queue();
    let mut queue = lock.lock().unwrap();
    match queue.pending.iter_mut().find(|(s, _)| *s == slot) {
        Some(waiting) => waiting.1 = Box::new(job),
        None => queue.pending.push((slot, Box::new(job))),
    }
    cv.notify_all();
}

/// Waits, up to two seconds, for every write asked for so far.
pub fn flush() {
    // Nothing to wait for if nothing ever asked for a write.
    let Some(q) = QUEUE.get() else { return };
    let (lock, cv) = &**q;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut queue = lock.lock().unwrap();
    while !queue.pending.is_empty() || queue.busy {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else { return };
        queue = cv.wait_timeout(queue, left).unwrap().0;
    }
}

/// Writes `bytes` to `path` through a temp file renamed into place, making its folder first.
pub fn replace(path: &std::path::Path, bytes: &[u8]) -> bool {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(tmp, path).is_ok()
}

