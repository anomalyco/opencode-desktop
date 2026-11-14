//! Queries share the exclusive connection. A surface keeps a single ticket across
//! keystrokes; its generation (not an interrupt on the connection) cancels old reads.

use std::collections::VecDeque;
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

use rusqlite::{Connection, params};
use crate::{Storage, StorageError};

#[derive(Default, Clone)]
pub struct SearchSurface(Arc<AtomicU64>);

#[derive(Clone, Default)]
pub struct Scope {
    pub server: Option<String>,
    pub session: Option<String>,
    pub include_archived: bool,
}

/// A row for `opencode_search::fulltext::rank`: newest first, at most [`LIMIT`].
pub struct Hit {
    pub server: String,
    pub session: String,
    /// The draft, outbox row or message it is.
    pub entity: String,
    /// `draft`, `outbox` (a queued prompt), `user`, `assistant`.
    pub kind: String,
    pub project_dir: String,
    pub title: String,
    /// The stored snippet (the text's first 400 bytes), decompressed.
    pub body: String,
    pub updated_ms: i64,
}

/// The newest rows a query answers with (SEARCH ranks them).
pub const LIMIT: i64 = 250;

pub(crate) struct Query {
    surface: SearchSurface,
    generation: u64,
    words: String,
    scope: Scope,
    reply: Box<dyn FnOnce(u64, Result<Vec<Hit>, StorageError>) + Send>,
}

impl SearchSurface {
    /// Cancels the pending select on close. Result receivers also compare their generation
    /// when applying an answer: the user may have typed again since it was sent.
    pub fn cancel(&self) { self.0.fetch_add(1, Ordering::AcqRel); }

    pub fn query(&self, storage: &Storage, words: String, scope: Scope, reply: impl FnOnce(u64, Result<Vec<Hit>, StorageError>) + Send + 'static) -> u64 {
        let generation = self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        storage.inner.backend.query(Query { surface: self.clone(), generation, words, scope, reply: Box::new(reply) });
        generation
    }

    pub fn current(&self, generation: u64) -> bool { self.0.load(Ordering::Acquire) == generation }
}

#[derive(Default)]
pub(crate) struct Lane(VecDeque<Query>);

impl Lane {
    pub(crate) fn replace(&mut self, q: Query) {
        if !q.surface.current(q.generation) { return }
        self.0.retain(|old| !Arc::ptr_eq(&old.surface.0, &q.surface.0));
        self.0.push_back(q);
    }
    pub(crate) fn pop(&mut self) -> Option<Query> { self.0.pop_front() }
}

pub(crate) enum Completion { Done, Yield(Query) }

impl Query {
    pub(crate) fn unavailable(self, why: String) {
        if self.surface.current(self.generation) { (self.reply)(self.generation, Err(StorageError(why))) }
    }

    pub(crate) fn run(self, c: &mut Connection, writes: &Arc<AtomicU64>, admitted: u64) -> Completion {
        let epoch = writes.clone();
        let surface = self.surface.clone();
        let generation = self.generation;
        if !surface.current(generation) { return Completion::Done }
        let result = c.progress_handler(200, Some(move || epoch.load(Ordering::Acquire) != admitted || !surface.current(generation)))
            .and_then(|()| self.select(c));
        // The SELECT's rows and statement have both dropped. No connection-wide
        // sqlite3_interrupt call can race a subsequent write or commit.
        let removed = c.progress_handler(0, None::<fn() -> bool>);
        if self.surface.current(generation) {
            if removed.is_ok() && writes.load(Ordering::Acquire) != admitted { return Completion::Yield(self) }
            (self.reply)(generation, removed.and(result).map_err(|e| StorageError(e.to_string())));
        }
        Completion::Done
    }

    fn select(&self, c: &Connection) -> rusqlite::Result<Vec<Hit>> {
        let Some(query) = opencode_search::tokenizer::fts_query(&self.words) else { return Ok(Vec::new()) };
        let mut stmt = c.prepare_cached(&format!("SELECT m.server,m.session_id,m.message_id,m.kind,m.project_dir,m.title,m.snippet_zstd,m.updated_ms
            FROM search_fts JOIN search_meta m ON m.id=search_fts.rowid WHERE search_fts MATCH ?1
            AND (?2 IS NULL OR m.server=?2) AND (?3 IS NULL OR m.session_id=?3)
            AND (?4 OR NOT m.archived) ORDER BY search_fts.rowid LIMIT {LIMIT}"))?;
        stmt.query_map(params![query,self.scope.server,self.scope.session,self.scope.include_archived], |r| {
            let packed: Vec<u8> = r.get(6)?;
            let body = zstd::bulk::decompress(&packed, 4096).ok().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
            Ok(Hit { server: r.get(0)?, session: r.get(1)?, entity: r.get(2)?, kind: r.get(3)?, project_dir: r.get(4)?, title: r.get(5)?, body, updated_ms: r.get(7)? })
        })?.collect()
    }
}
