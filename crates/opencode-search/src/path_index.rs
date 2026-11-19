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

    /// Whether a background index build is in flight.
    pub fn is_building(&self) -> bool {
        matches!(self.state, PathsState::Building { .. })
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

    /// Active worker phase. At most one worker thread exists at any time: none in `Idle` or
    /// `Ready`, and one in `Queued`, `Building`, or `Draining`. A queued job always supersedes
    /// any previously landed index, so `Queued` and a landed index can never coexist.
    #[derive(Default)]
    enum Phase {
        /// No worker thread is running and no built index waits for `take`.
        #[default]
        Idle,
        /// No worker thread is running; the latest built index waits for `take`.
        Ready(u64, PathIndex),
        /// A worker thread exists and a newer job is queued (either freshly spawned before its
        /// first dequeue, or currently building a stale generation whose result will be discarded).
        Queued(u64, Vec<String>),
        /// The worker thread is actively building `generation` with no newer request queued.
        Building(u64),
        /// The worker finished its current build (storing `landed` if it succeeded and has not
        /// yet been taken) and is looping back to exit unless another request arrives first.
        Draining(Option<(u64, PathIndex)>),
    }

    /// Shared worker slot protected by `Inner::slot`: either active or cancelled on drop (which
    /// holds no waker, pending job, or landed index).
    enum Slot {
        Active { waker: Option<Waker>, phase: Phase },
        Cancelled,
    }

    impl Default for Slot {
        fn default() -> Self {
            Self::Active {
                waker: None,
                phase: Phase::Idle,
            }
        }
    }

    enum SlotEvent {
        SetWaker(Waker),
        Request { generation: u64, paths: Vec<String> },
        Take,
        Cancel,
        WorkerNext,
        WorkerFinished { generation: u64, built: Option<PathIndex> },
    }

    enum SlotEffect {
        DropWaker(Option<Waker>),
        Requested {
            spawn: bool,
            drop_landed: Option<(u64, PathIndex)>,
            drop_pending: Option<(u64, Vec<String>)>,
        },
        Taken(Option<(u64, PathIndex)>),
        DropSlot(Slot),
        Run(Option<(u64, Vec<String>)>),
        Finished {
            wake: Option<Waker>,
            drop_index: Option<PathIndex>,
        },
    }

    impl Slot {
        /// Pure state transition run under `Inner::slot`. Every replaced or discarded value is
        /// returned in the effect so the caller drops it after releasing the mutex.
        fn step(&mut self, event: SlotEvent) -> SlotEffect {
            match (self, event) {
                (Slot::Active { waker, .. }, SlotEvent::SetWaker(next)) => {
                    SlotEffect::DropWaker(waker.replace(next))
                }
                (Slot::Cancelled, SlotEvent::SetWaker(next)) => SlotEffect::DropWaker(Some(next)),
                (Slot::Active { phase, .. }, SlotEvent::Request { generation, paths }) => {
                    let (spawn, drop_landed, drop_pending) =
                        match std::mem::replace(phase, Phase::Queued(generation, paths)) {
                            Phase::Idle => (true, None, None),
                            Phase::Ready(g, idx) => (true, Some((g, idx)), None),
                            Phase::Queued(g, p) => (false, None, Some((g, p))),
                            Phase::Building(_) => (false, None, None),
                            Phase::Draining(landed) => (false, landed, None),
                        };
                    SlotEffect::Requested {
                        spawn,
                        drop_landed,
                        drop_pending,
                    }
                }
                (Slot::Cancelled, SlotEvent::Request { generation, paths }) => {
                    SlotEffect::Requested {
                        spawn: false,
                        drop_landed: None,
                        drop_pending: Some((generation, paths)),
                    }
                }
                (Slot::Active { phase, .. }, SlotEvent::Take) => {
                    let taken = match phase {
                        Phase::Ready(_, _) => match std::mem::replace(phase, Phase::Idle) {
                            Phase::Ready(g, idx) => Some((g, idx)),
                            _ => unreachable!(),
                        },
                        Phase::Draining(landed) => landed.take(),
                        Phase::Idle | Phase::Queued(_, _) | Phase::Building(_) => None,
                    };
                    SlotEffect::Taken(taken)
                }
                (Slot::Cancelled, SlotEvent::Take) => SlotEffect::Taken(None),
                (slot, SlotEvent::Cancel) => {
                    SlotEffect::DropSlot(std::mem::replace(slot, Slot::Cancelled))
                }
                (Slot::Active { phase, .. }, SlotEvent::WorkerNext) => {
                    match std::mem::take(phase) {
                        Phase::Queued(generation, paths) => {
                            *phase = Phase::Building(generation);
                            SlotEffect::Run(Some((generation, paths)))
                        }
                        Phase::Draining(Some((g, idx))) => {
                            *phase = Phase::Ready(g, idx);
                            SlotEffect::Run(None)
                        }
                        Phase::Draining(None) | Phase::Idle => {
                            *phase = Phase::Idle;
                            SlotEffect::Run(None)
                        }
                        prev @ (Phase::Ready(_, _) | Phase::Building(_)) => {
                            *phase = prev;
                            SlotEffect::Run(None)
                        }
                    }
                }
                (Slot::Cancelled, SlotEvent::WorkerNext) => SlotEffect::Run(None),
                (
                    Slot::Active { waker, phase },
                    SlotEvent::WorkerFinished { generation, built },
                ) => match phase {
                    Phase::Building(g) if *g == generation => {
                        let wake = built.as_ref().and_then(|_| waker.clone());
                        *phase = Phase::Draining(built.map(|idx| (generation, idx)));
                        SlotEffect::Finished {
                            wake,
                            drop_index: None,
                        }
                    }
                    _ => SlotEffect::Finished {
                        wake: None,
                        drop_index: built,
                    },
                },
                (Slot::Cancelled, SlotEvent::WorkerFinished { built, .. }) => {
                    SlotEffect::Finished {
                        wake: None,
                        drop_index: built,
                    }
                }
            }
        }
    }

    #[derive(Default)]
    struct Inner {
        slot: Mutex<Slot>,
    }

    impl Inner {
        fn step(&self, event: SlotEvent) -> SlotEffect {
            self.slot.lock().unwrap_or_else(|e| e.into_inner()).step(event)
        }
    }

    /// Latest-wins background builder: at most one thread per `LivePaths`, coalescing rapid
    /// updates and dropping stale builds before they can land or wake.
    #[derive(Default)]
    pub(super) struct PathWorker {
        inner: Arc<Inner>,
    }

    impl PathWorker {
        pub(super) fn set_waker(&mut self, waker: Waker) {
            if let SlotEffect::DropWaker(old) = self.inner.step(SlotEvent::SetWaker(waker)) {
                drop(old);
            }
        }

        pub(super) fn request(&mut self, generation: u64, paths: Vec<String>) {
            let SlotEffect::Requested {
                spawn,
                drop_landed,
                drop_pending,
            } = self.inner.step(SlotEvent::Request { generation, paths })
            else {
                return;
            };
            drop(drop_landed);
            drop(drop_pending);
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
            match self.inner.step(SlotEvent::Take) {
                SlotEffect::Taken(landed) => landed,
                _ => None,
            }
        }
    }

    impl Drop for PathWorker {
        fn drop(&mut self) {
            if let SlotEffect::DropSlot(old) = self.inner.step(SlotEvent::Cancel) {
                drop(old);
            }
        }
    }

    fn drive(inner: Arc<Inner>) {
        loop {
            let SlotEffect::Run(Some((generation, paths))) = inner.step(SlotEvent::WorkerNext)
            else {
                return;
            };
            let built = catch_unwind(AssertUnwindSafe(|| PathIndex::new(paths.into_iter()))).ok();
            let SlotEffect::Finished { wake, drop_index } =
                inner.step(SlotEvent::WorkerFinished { generation, built })
            else {
                return;
            };
            drop(drop_index);
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
