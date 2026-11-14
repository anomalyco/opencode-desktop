//! Immutable projection of the paths known to the local workspace. Query results
//! refer to the same snapshot as their labels, even if a directory reply arrives.
use opencode_search::SearchArena;

pub(crate) struct PathIndex {
    paths: Vec<String>,
    arena: SearchArena,
}

impl PathIndex {
    pub fn new(paths: impl Iterator<Item = String>) -> Self {
        let mut arena = SearchArena::default();
        let paths: Vec<_> = paths.inspect(|p| { arena.push(p); }).collect();
        arena.compact();
        Self { paths, arena }
    }

    pub fn query(&self, query: &str, limit: usize) -> Vec<String> {
        if query.is_empty() { return self.paths.iter().take(limit).cloned().collect(); }
        self.arena.query(query, limit).into_iter().map(|i| self.paths[i].clone()).collect()
    }
}
