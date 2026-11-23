//! The GPU hang watch (`widgets-gpu.md`, GPU hang): live runs' GPU work is looked at about once a
//! display frame (`widget_gpu::Gpu::poll`) on a thread of its own, which never submits, so a run
//! stuck submitting or waiting while the GPU hangs can't keep it from looking. It looks only while
//! some run's work may be unfinished, and sleeps otherwise until a run submits.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

/// A run the watch looks at.
pub(crate) trait Watched: Send + Sync {
    /// GPU work it submitted may be unfinished.
    fn busy(&self) -> bool;
    /// Looks for a hang, never waiting (nor for a run busy replaying).
    fn look(&self);
}

/// About a display frame.
const EVERY: Duration = Duration::from_millis(16);

/// The runs watched, and how many times one submitted.
type Runs = (Vec<Weak<dyn Watched>>, u64);

struct Watch {
    state: Mutex<Runs>,
    turn: Condvar,
}

fn watch() -> &'static Watch {
    static WATCH: OnceLock<&'static Watch> = OnceLock::new();
    WATCH.get_or_init(|| {
        let w: &'static Watch = Box::leak(Box::new(Watch { state: Mutex::default(), turn: Condvar::new() }));
        if let Err(e) = std::thread::Builder::new().name("widget-gpu-watch".into()).spawn(move || w.run()) {
            eprintln!("widget gpu: no thread to watch for GPU hangs: {e}");
        }
        w
    })
}

/// Watches `run` while it lives.
#[cfg_attr(not(feature = "runner"), allow(dead_code))]
pub(crate) fn add(run: Weak<dyn Watched>) {
    watch().lock().0.push(run);
}

/// A run submitted GPU work: the watch looks.
#[cfg_attr(not(feature = "runner"), allow(dead_code))]
pub(crate) fn poke() {
    let w = watch();
    w.lock().1 += 1;
    w.turn.notify_all();
}

impl Watch {
    fn lock(&self) -> MutexGuard<'_, Runs> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(&self) {
        loop {
            let (runs, seen) = {
                let mut st = self.lock();
                st.0.retain(|r| r.strong_count() > 0);
                (st.0.iter().filter_map(Weak::upgrade).collect::<Vec<Arc<dyn Watched>>>(), st.1)
            };
            let mut busy = false;
            for run in &runs {
                if run.busy() {
                    busy = true;
                    run.look();
                }
            }
            drop(runs);
            let st = self.lock();
            if busy {
                drop(self.turn.wait_timeout(st, EVERY).unwrap_or_else(PoisonError::into_inner));
            } else {
                drop(self.turn.wait_while(st, |s| s.1 == seen).unwrap_or_else(PoisonError::into_inner));
            }
        }
    }
}
