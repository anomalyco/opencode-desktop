//! Immutable projection of the paths known to the local workspace. Query results
//! refer to the same snapshot as their labels, even if a directory reply arrives.
use opencode_search::SearchArena;
use rune::{Latest, Waker};

/// The most rows any surface asks of a path query (Ctrl+P's file search).
const MOST: usize = 100;

pub(crate) struct PathIndex {
    paths: Vec<String>,
    arena: SearchArena,
}

impl PathIndex {
    pub fn new(paths: impl Iterator<Item = String>) -> Self {
        let mut arena = SearchArena::default();
        let paths: Vec<_> = paths.inspect(|p| { arena.push(p); }).collect();
        // A first letter answers from a list made here, not by ranking every path on the key.
        arena.prepare_first_letters(MOST);
        arena.compact();
        Self { paths, arena }
    }

    pub fn query(&self, query: &str, limit: usize) -> Vec<String> {
        if query.is_empty() { return self.paths.iter().take(limit).cloned().collect(); }
        self.arena.query(query, limit).into_iter().map(|i| self.paths[i].clone()).collect()
    }
}

/// A path list's index, built on a worker when the list changes so no key waits for it.
#[derive(Default)]
pub(crate) struct LivePaths {
    state: PathsState,
    worker: Latest<(u64, PathIndex)>,
    /// The last generation asked for.
    asked: u64,
}

/// Empty until a list arrives; building a generation (still answering from the index before
/// it, if any); or ready on one.
#[derive(Default)]
enum PathsState {
    #[default]
    Empty,
    Building { generation: u64, shown: Option<(u64, PathIndex)> },
    Ready { generation: u64, index: PathIndex },
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
        if let Some(PathsEffect::Build(generation)) = self.state.step(PathsEvent::Changed(self.asked)) {
            self.worker.request(paths, move |paths| (generation, PathIndex::new(paths.into_iter())));
        }
    }

    /// Lands a built index: true when the answers change.
    pub fn poll(&mut self) -> bool {
        let Some((generation, index)) = self.worker.take() else { return false };
        let _ = self.state.step(PathsEvent::Landed(generation, index));
        matches!(self.state, PathsState::Ready { generation: g, .. } if g == generation)
    }

    /// Which index answers: changes when one lands.
    pub fn version(&self) -> u64 {
        match &self.state {
            PathsState::Empty | PathsState::Building { shown: None, .. } => 0,
            PathsState::Building { shown: Some((generation, _)), .. } | PathsState::Ready { generation, .. } => *generation,
        }
    }

    /// The paths matching `query` in the index built last; none before the first lands.
    pub fn query(&self, query: &str, limit: usize) -> Vec<String> {
        self.state.index().map(|i| i.query(query, limit)).unwrap_or_default()
    }
}
