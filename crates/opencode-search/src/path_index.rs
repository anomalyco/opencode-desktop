//! Immutable projection of workspace file paths and a headless latest-wins live builder.
//! Query results refer to the same snapshot as their labels, even if a directory reply arrives.

use std::sync::Arc;

use crate::SearchArena;

/// Wake callback invoked when a background path-index build lands.
/// Type-identical to `rune::Waker` without depending on Rune.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The most rows any surface asks of a path query (Ctrl+P's file search).
const MOST: usize = 100;

pub struct PathIndex {
    paths: Vec<String>,
    arena: SearchArena,
}

impl PathIndex {
    pub fn new(paths: impl Iterator<Item = String>) -> Self {
        let mut arena = SearchArena::default();
        let paths: Vec<_> = paths
            .inspect(|p| {
                arena.push(p);
            })
            .collect();
        // A first letter answers from a list made here, not by ranking every path on the key.
        arena.prepare_first_letters(MOST);
        arena.compact();
        Self { paths, arena }
    }

    pub fn query(&self, query: &str, limit: usize) -> Vec<String> {
        if query.is_empty() {
            return self.paths.iter().take(limit).cloned().collect();
        }
        self.arena
            .query(query, limit)
            .into_iter()
            .map(|i| self.paths[i].clone())
            .collect()
    }
}

/// A path list's index, built on a worker (or inline on `wasm32`) when the list changes so no
/// keystroke waits for it.
#[derive(Default)]
pub struct LivePaths {
    state: PathsState,
    worker: PathWorker,
    /// The last generation asked for.
    asked: u64,
}

/// Empty until a list arrives; building a generation (still answering from the index before
/// it, if any); or ready on one.
#[derive(Default)]
enum PathsState {
    #[default]
    Empty,
    Building {
        generation: u64,
        shown: Option<(u64, PathIndex)>,
    },
    Ready {
        generation: u64,
        index: PathIndex,
    },
}

enum PathsEvent {
    /// The list changed: generation `0` is the one to build.
    Changed(u64),
    Landed(u64, PathIndex),
}

enum PathsEffect {
    Build(u64),
}

impl PathsState {
    fn step(&mut self, event: PathsEvent) -> Option<PathsEffect> {
        match event {
            PathsEvent::Changed(generation) => {
                let shown = match std::mem::take(self) {
                    PathsState::Empty => None,
                    PathsState::Building { shown, .. } => shown,
                    PathsState::Ready { generation, index } => Some((generation, index)),
                };
                *self = PathsState::Building { generation, shown };
                Some(PathsEffect::Build(generation))
            }
            PathsEvent::Landed(generation, index) => {
                if matches!(self, PathsState::Building { generation: g, .. } if *g == generation) {
                    *self = PathsState::Ready { generation, index };
                }
                None
            }
        }
    }

    fn index(&self) -> Option<&PathIndex> {
        match self {
            PathsState::Empty => None,
            PathsState::Building { shown, .. } => shown.as_ref().map(|(_, i)| i),
            PathsState::Ready { index, .. } => Some(index),
        }
    }
}

impl LivePaths {
    pub fn set_waker(&mut self, waker: Waker) {
        self.worker.set_waker(waker);
    }

    /// The list is now `paths`: its index builds on a worker.
    pub fn changed(&mut self, paths: Vec<String>) {
        self.asked += 1;
        if let Some(PathsEffect::Build(generation)) =
            self.state.step(PathsEvent::Changed(self.asked))
        {
            self.worker.request(generation, paths);
        }
    }

    /// Lands a built index: true when the answers change.
    pub fn poll(&mut self) -> bool {
        let Some((generation, index)) = self.worker.take() else {
            return false;
        };
        let _ = self.state.step(PathsEvent::Landed(generation, index));
        matches!(self.state, PathsState::Ready { generation: g, .. } if g == generation)
    }

    /// Which index answers: changes when one lands.
    pub fn version(&self) -> u64 {
        match &self.state {
            PathsState::Empty | PathsState::Building { shown: None, .. } => 0,
            PathsState::Building {
                shown: Some((generation, _)),
                ..
            }
            | PathsState::Ready { generation, .. } => *generation,
        }
    }

    /// The paths matching `query` in the index built last; none before the first lands.
    pub fn query(&self, query: &str, limit: usize) -> Vec<String> {
        self.state
            .index()
            .map(|i| i.query(query, limit))
            .unwrap_or_default()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod worker {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Mutex};

    use super::{PathIndex, Waker};

    #[derive(Default)]
    struct Slot {
        latest: u64,
        pending: Option<(u64, Vec<String>)>,
        running: bool,
        landed: Option<(u64, PathIndex)>,
        waker: Option<Waker>,
        cancelled: bool,
    }

    #[derive(Default)]
    struct Inner {
        slot: Mutex<Slot>,
    }

    /// Latest-wins background builder: at most one thread per `LivePaths`, coalescing rapid
    /// updates and dropping stale builds before they can land or wake.
    #[derive(Default)]
    pub(super) struct PathWorker {
        inner: Arc<Inner>,
    }

    impl PathWorker {
        pub(super) fn set_waker(&mut self, waker: Waker) {
            let old = self
                .inner
                .slot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .waker
                .replace(waker);
            drop(old);
        }

        pub(super) fn request(&mut self, generation: u64, paths: Vec<String>) {
            let (spawn, old_landed, old_pending) = {
                let mut slot = self.inner.slot.lock().unwrap_or_else(|e| e.into_inner());
                slot.latest = generation;
                let old_landed = slot.landed.take();
                let old_pending = slot.pending.replace((generation, paths));
                let spawn = !slot.running;
                if spawn {
                    slot.running = true;
                }
                (spawn, old_landed, old_pending)
            };
            drop(old_landed);
            drop(old_pending);
            if spawn {
                let inner = self.inner.clone();
                if std::thread::Builder::new()
                    .name("opencode-paths".into())
                    .spawn(move || drive(inner))
                    .is_err()
                {
                    drive(self.inner.clone());
                }
            }
        }

        pub(super) fn take(&mut self) -> Option<(u64, PathIndex)> {
            self.inner
                .slot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .landed
                .take()
        }
    }

    impl Drop for PathWorker {
        fn drop(&mut self) {
            let (old_landed, old_pending, old_waker) = {
                let mut slot = self.inner.slot.lock().unwrap_or_else(|e| e.into_inner());
                slot.cancelled = true;
                (slot.landed.take(), slot.pending.take(), slot.waker.take())
            };
            drop(old_landed);
            drop(old_pending);
            drop(old_waker);
        }
    }

    fn drive(inner: Arc<Inner>) {
        loop {
            let next = {
                let mut slot = inner.slot.lock().unwrap_or_else(|e| e.into_inner());
                if slot.cancelled {
                    slot.running = false;
                    return;
                }
                match slot.pending.take() {
                    Some(job) => job,
                    None => {
                        slot.running = false;
                        return;
                    }
                }
            };
            let (generation, paths) = next;
            let built = catch_unwind(AssertUnwindSafe(|| PathIndex::new(paths.into_iter())));
            let (wake, discarded) = {
                let mut slot = inner.slot.lock().unwrap_or_else(|e| e.into_inner());
                if !slot.cancelled && slot.latest == generation {
                    match built {
                        Ok(index) => {
                            let old = slot.landed.replace((generation, index));
                            (slot.waker.clone(), old)
                        }
                        Err(_) => (None, None),
                    }
                } else {
                    (None, built.ok().map(|i| (generation, i)))
                }
            };
            drop(discarded);
            if let Some(waker) = wake {
                waker();
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod worker {
    use super::{PathIndex, Waker};

    #[derive(Default)]
    pub(super) struct PathWorker {
        landed: Option<(u64, PathIndex)>,
        waker: Option<Waker>,
    }

    impl PathWorker {
        pub(super) fn set_waker(&mut self, waker: Waker) {
            self.waker = Some(waker);
        }

        pub(super) fn request(&mut self, generation: u64, paths: Vec<String>) {
            self.landed = Some((generation, PathIndex::new(paths.into_iter())));
            if let Some(waker) = self.waker.clone() {
                waker();
            }
        }

        pub(super) fn take(&mut self) -> Option<(u64, PathIndex)> {
            self.landed.take()
        }
    }
}

use worker::PathWorker;
