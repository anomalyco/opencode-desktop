//! The durable prompt outbox, per server (Tier 0): every row committed with a full sync before
//! its callback runs, so the caller hands a prompt to the network only once it would survive a
//! crash or a power cut. The caller's thread does no work beyond handing the request over: a
//! prompt's first row (its JSON and the SHA-256 of its files) is built on the storage thread by
//! the caller's closure, and every later step writes only the columns it changes.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod native;
#[cfg(target_arch = "wasm32")]
pub(crate) mod web;

use std::sync::Arc;

use crate::cache::OutboxCache;
use crate::{Cmd, Done, Loaded, OutboxRow, Storage, StorageError};

/// A prompt's first row and the blobs it names (id, bytes).
pub struct OutboxPut {
    pub row: OutboxRow,
    pub blobs: Vec<(String, Arc<[u8]>)>,
}

/// Builds a prompt's first row, on the storage thread.
pub type OutboxBuild = Box<dyn FnOnce() -> OutboxPut + Send>;

/// The columns a prompt's later steps change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutboxUpdate {
    pub state: String,
    pub why: String,
    pub after_check: Option<String>,
    pub restored: bool,
    pub operation_json: String,
}

/// A stored prompt and the bytes of the blobs it names.
pub struct OutboxEntry {
    pub row: OutboxRow,
    pub blobs: Vec<(String, Arc<[u8]>)>,
}

/// A server's stored prompts, oldest first, on the storage thread; an error when nothing durable
/// holds them (storage in memory, or a failed read).
pub type OutboxRead = Box<dyn FnOnce(Result<Vec<OutboxEntry>, StorageError>) + Send>;

pub(crate) enum OutboxOp {
    Put(OutboxBuild),
    Update { server: String, id: String, update: OutboxUpdate },
    Delete { server: String, id: String },
    Read { server: String, adopt: Vec<String>, done: OutboxRead },
}

/// What memory-only storage answers: nothing it holds is durable.
pub(crate) fn offline(op: OutboxOp, done: Option<Done>) {
    let error = || StorageError("storage is in memory: nothing is kept".into());
    match op {
        OutboxOp::Read { done, .. } => done(Err(error())),
        _ => {
            if let Some(done) = done {
                done(Err(error()));
            }
        }
    }
}

/// Applies a committed write to the rows in memory (when they're loaded).
pub(crate) fn cached(outbox: &mut OutboxCache, write: &Written) {
    let OutboxCache::Loaded(all) = outbox else { return };
    match write {
        Written::Put(row) => {
            let list = all.entry(row.server.clone()).or_default();
            match list.iter_mut().find(|r| r.id == row.id) {
                Some(r) => *r = row.clone(),
                None => list.push(row.clone()),
            }
            list.sort_by_key(|r| r.seq);
        }
        Written::Update { server, id, update } => {
            if let Some(r) = all.get_mut(server).and_then(|l| l.iter_mut().find(|r| r.id == *id)) {
                r.state.clone_from(&update.state);
                r.why.clone_from(&update.why);
                r.after_check.clone_from(&update.after_check);
                r.restored = update.restored;
                r.operation_json.clone_from(&update.operation_json);
            }
        }
        Written::Delete { server, id } => {
            if let Some(list) = all.get_mut(server) {
                list.retain(|r| r.id != *id);
            }
        }
    }
}

/// A write as the storage thread commits it: a put already built.
pub(crate) enum Written {
    Put(OutboxRow),
    Update { server: String, id: String, update: OutboxUpdate },
    Delete { server: String, id: String },
}

/// The durable prompt outbox, per server (Tier 0).
pub struct OutboxStore<'a> {
    pub(crate) storage: &'a Storage,
}

impl OutboxStore<'_> {
    /// Server `server`'s rows in order; none until loaded ([`OutboxStore::load`]).
    pub fn rows(&self, server: &str) -> Option<Vec<OutboxRow>> {
        match &self.storage.cache().outbox {
            OutboxCache::Loaded(all) => Some(all.get(server).cloned().unwrap_or_default()),
            OutboxCache::Unloaded => None,
        }
    }

    /// Every server with rows; none until loaded.
    pub fn servers(&self) -> Option<Vec<String>> {
        match &self.storage.cache().outbox {
            OutboxCache::Loaded(all) => Some(all.keys().cloned().collect()),
            OutboxCache::Unloaded => None,
        }
    }

    pub fn load(&self, done: Option<Loaded>) {
        if matches!(self.storage.cache().outbox, OutboxCache::Loaded(_)) {
            if let Some(done) = done {
                done();
            }
            return;
        }
        self.storage.send(Cmd::LoadOutbox { done });
    }

    /// Server `server`'s prompts with their blobs' bytes, oldest first, handed to `done` on the
    /// storage thread. Rows kept under one of the `adopt` names (the importer's `sha256:<file
    /// hash>` for a journal whose server it couldn't name) become `server`'s first, durably.
    pub fn read(&self, server: &str, adopt: Vec<String>, done: OutboxRead) {
        self.storage.send(Cmd::Outbox { op: OutboxOp::Read { server: server.to_owned(), adopt, done }, done: None });
    }

    /// Admits a prompt: `build` makes its row and blobs on the storage thread, and `done` runs
    /// there once they're durable (`synchronous = FULL`): forward the network write only then.
    pub fn put(&self, build: OutboxBuild, done: Option<Done>) {
        self.storage.send(Cmd::Outbox { op: OutboxOp::Put(build), done });
    }

    /// A later step of prompt `id`: only `update`'s columns are written. `done` gets an error
    /// when no such row is stored.
    pub fn update(&self, server: &str, id: &str, update: OutboxUpdate, done: Option<Done>) {
        self.storage.send(Cmd::Outbox { op: OutboxOp::Update { server: server.to_owned(), id: id.to_owned(), update }, done });
    }

    pub fn delete(&self, server: &str, id: &str, done: Option<Done>) {
        self.storage.send(Cmd::Outbox { op: OutboxOp::Delete { server: server.to_owned(), id: id.to_owned() }, done });
    }
}
