//! The typed views over [`Storage`]: every read from memory, every write queued.

use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::cache::{Doc, Lookup, OutboxCache};
use crate::{Cmd, Done, Loaded, OutboxOp, OutboxRow, Storage, WindowFrame};

/// Namespaced key-value state (`state(name, key, value)`), as Desktop's `platform.storage(name)`.
/// Values are JSON text.
pub struct StateStore<'a> {
    pub(crate) storage: &'a Storage,
}

impl StateStore<'_> {
    pub fn get(&self, name: &str, key: &str) -> Lookup {
        self.storage.cache().get(name, key)
    }

    /// The value as text; none when absent or not loaded yet.
    pub fn text(&self, name: &str, key: &str) -> Option<Arc<str>> {
        match self.get(name, key) {
            Lookup::Ready(v) => v,
            Lookup::Pending => None,
        }
    }

    /// The value parsed; none when absent, not loaded yet, or another shape.
    pub fn json<T: DeserializeOwned>(&self, name: &str, key: &str) -> Option<T> {
        serde_json::from_str(&self.text(name, key)?).ok()
    }

    /// Sets a value (JSON text) and returns the namespace's new revision.
    pub fn set(&self, name: &str, key: &str, value: impl Into<Arc<str>>) -> u64 {
        self.put(name, key, Some(value.into()))
    }

    pub fn set_json<T: Serialize + ?Sized>(&self, name: &str, key: &str, value: &T) -> u64 {
        self.set(name, key, serde_json::to_string(value).unwrap_or_else(|_| "null".into()))
    }

    pub fn delete(&self, name: &str, key: &str) -> u64 {
        self.put(name, key, None)
    }

    fn put(&self, name: &str, key: &str, value: Option<Arc<str>>) -> u64 {
        let revision = {
            let mut cache = self.storage.cache();
            if matches!(cache.get(name, key), Lookup::Ready(ref v) if *v == value) {
                return cache.state.get(name).map_or(0, crate::cache::Namespace::revision);
            }
            cache.set(name, key.to_owned(), value.clone())
        };
        self.storage.send(Cmd::State { name: name.to_owned(), key: key.to_owned(), value });
        revision
    }

    /// Every row of `name`; none until a lazy namespace is loaded ([`StateStore::load`]).
    pub fn items(&self, name: &str) -> Option<Vec<(String, Arc<str>)>> {
        self.storage.cache().items(name)
    }

    /// Removes every row of `name` (a window closed for good).
    pub fn clear(&self, name: &str) -> u64 {
        let revision = self.storage.cache().clear(name);
        self.storage.send(Cmd::ClearState { name: name.to_owned() });
        revision
    }

    /// The namespace's revision: it grows with every write, so a window compares it to the one
    /// it last read.
    pub fn revision(&self, name: &str) -> u64 {
        self.storage.cache().state.get(name).map_or(0, crate::cache::Namespace::revision)
    }

    /// Whether `name` is in memory in full.
    pub fn is_loaded(&self, name: &str) -> bool {
        self.storage.cache().items(name).is_some()
    }

    /// Loads lazy namespaces on the storage thread; `done` runs there once they're in memory.
    pub fn load(&self, names: &[&str], done: Option<Loaded>) {
        let names: Vec<String> = { names.iter().filter(|n| !self.is_loaded(n)).map(|n| (*n).to_owned()).collect() };
        if names.is_empty() {
            if let Some(done) = done {
                done();
            }
            return;
        }
        self.storage.send(Cmd::LoadState { names, done });
    }
}

/// Drafts and prompt history (`document(key, value)`), with long strings kept as text chunks in
/// `blob`, and binary attachments by content hash.
pub struct DocumentStore<'a> {
    pub(crate) storage: &'a Storage,
}

impl DocumentStore<'_> {
    /// The document's JSON text, its chunks put back.
    pub fn get(&self, key: &str) -> Lookup {
        match self.storage.cache().docs.get(key) {
            Some(Doc::Present(v)) => Lookup::Ready(Some(v.clone())),
            Some(Doc::Stored) => Lookup::Pending,
            None => Lookup::Ready(None),
        }
    }

    pub fn json<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        match self.get(key) {
            Lookup::Ready(Some(v)) => serde_json::from_str(&v).ok(),
            _ => None,
        }
    }

    /// Sets (`Some`) or removes a document. Long strings are split into chunks on the storage
    /// thread, never on the caller's.
    pub fn set(&self, key: &str, value: Option<String>) {
        {
            let cache = self.storage.cache();
            let same = match (cache.docs.get(key), &value) {
                (Some(Doc::Present(old)), Some(new)) => **old == **new,
                (None, None) => true,
                _ => false,
            };
            if same {
                return;
            }
        }
        let value: Option<Arc<str>> = value.map(Into::into);
        {
            let mut cache = self.storage.cache();
            match &value {
                Some(v) => cache.docs.insert(key.to_owned(), Doc::Present(v.clone())),
                None => cache.docs.remove(key),
            };
        }
        self.storage.send(Cmd::Doc { key: key.to_owned(), value });
    }

    /// The keys stored under `prefix` (what's in memory and what waits on disk).
    pub fn keys(&self, prefix: &str) -> Vec<String> {
        let mut keys: Vec<String> = self.storage.cache().docs.keys().filter(|k| k.starts_with(prefix)).cloned().collect();
        keys.sort();
        keys
    }

    /// Loads documents on the storage thread; `done` runs there once they're in memory.
    pub fn load(&self, keys: &[&str], done: Option<Loaded>) {
        let keys: Vec<String> = keys.iter().filter(|k| matches!(self.get(k), Lookup::Pending)).map(|k| (*k).to_owned()).collect();
        if keys.is_empty() {
            if let Some(done) = done {
                done();
            }
            return;
        }
        self.storage.send(Cmd::LoadDocs { keys, done });
    }

    /// Stores binary bytes by their SHA-256 and returns the id. Hashing runs on the caller's
    /// thread: call it from a worker for a large file.
    pub fn put_blob(&self, data: Arc<[u8]>) -> String {
        let id = crate::chunk::blob_id(&data);
        self.storage.send(Cmd::Blob { id: id.clone(), data });
        id
    }

    /// Reads a blob on the storage thread.
    pub fn get_blob(&self, id: &str, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        self.storage.send(Cmd::GetBlob { id: id.to_owned(), done });
    }
}

/// Each window's frame (`window_state`), read before its window opens.
pub struct WindowStore<'a> {
    pub(crate) storage: &'a Storage,
}

impl WindowStore<'_> {
    pub fn frame(&self, id: &str) -> Option<WindowFrame> {
        self.storage.cache().windows.get(id).copied()
    }

    /// Records the frame (coalesced for 250 ms). Call it when a move or resize settles and on close.
    pub fn set_frame(&self, id: &str, frame: WindowFrame) {
        if self.storage.cache().windows.insert(id.to_owned(), frame) == Some(frame) {
            return;
        }
        self.storage.send(Cmd::Window { id: id.to_owned(), frame: Some(frame) });
    }

    /// The window ids saved at the last quit (`opencode.settings` `windowIds`), in order.
    pub fn ids(&self) -> Option<Vec<String>> {
        self.storage.state().json::<Vec<String>>("opencode.settings", "windowIds").map(|ids| ids.into_iter().filter(|id| !id.is_empty()).collect())
    }

    pub fn save_ids(&self, ids: &[String]) {
        self.storage.state().set_json("opencode.settings", "windowIds", ids);
    }

    /// Window `id` closed for good while others stay: its frame, state and drafts go
    /// (Desktop's `registry.closed`).
    pub fn forget(&self, id: &str) {
        let name = crate::window_namespace(id);
        let drafts = self.storage.documents().keys(&format!("{name}:"));
        self.storage.cache().windows.remove(id);
        self.storage.send(Cmd::Window { id: id.to_owned(), frame: None });
        self.storage.state().clear(&name);
        for key in drafts {
            self.storage.documents().set(&key, None);
        }
    }
}

/// GUI plugin enable overrides (`plugin`) and third-party library trust (`plugin_trust`), both
/// Tier 0; each plugin's own values live in `state` under [`crate::plugin_namespace`].
pub struct PluginStore<'a> {
    pub(crate) storage: &'a Storage,
}

impl PluginStore<'_> {
    /// The override for plugin `id`; none when it never changed (built-ins are on).
    pub fn enabled(&self, id: &str) -> Option<bool> {
        self.storage.cache().plugins.get(id).copied()
    }

    pub fn disabled(&self) -> Vec<String> {
        self.storage.cache().plugins.iter().filter(|(_, on)| !**on).map(|(id, _)| id.clone()).collect()
    }

    /// Sets (`Some`) or clears the override; `done` runs once it's durable.
    pub fn set_enabled(&self, id: &str, enabled: Option<bool>, done: Option<Done>) {
        {
            let mut cache = self.storage.cache();
            match enabled {
                Some(on) => cache.plugins.insert(id.to_owned(), on),
                None => cache.plugins.remove(id),
            };
        }
        self.storage.send(Cmd::Plugin { id: id.to_owned(), enabled, done });
    }

    /// The SHA-256 the user trusted for plugin `id`'s library.
    pub fn trusted(&self, id: &str) -> Option<String> {
        self.storage.cache().trust.get(id).cloned()
    }

    pub fn trusted_all(&self) -> Vec<(String, String)> {
        self.storage.cache().trust.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Trusts (`Some(sha256)`) or forgets plugin `id`'s library; `done` runs once it's durable.
    pub fn set_trusted(&self, id: &str, sha256: Option<String>, done: Option<Done>) {
        {
            let mut cache = self.storage.cache();
            match &sha256 {
                Some(sha) => cache.trust.insert(id.to_owned(), sha.clone()),
                None => cache.trust.remove(id),
            };
        }
        self.storage.send(Cmd::Trust { id: id.to_owned(), sha256, done });
    }
}

/// The durable prompt outbox, per server (Tier 0). Loaded after the first frame.
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

    /// Admits or updates a prompt with its attachment bytes (`blobs`: id and bytes, ids as
    /// [`OutboxRow::attachments`] name them). `done` runs once the row and its blobs are durable
    /// (`synchronous = FULL`): forward the network write only then.
    pub fn put(&self, row: OutboxRow, blobs: Vec<(String, Arc<[u8]>)>, done: Option<Done>) {
        if let OutboxCache::Loaded(all) = &mut self.storage.cache().outbox {
            let list = all.entry(row.server.clone()).or_default();
            match list.iter_mut().find(|r| r.id == row.id) {
                Some(r) => *r = row.clone(),
                None => list.push(row.clone()),
            }
            list.sort_by_key(|r| r.seq);
        }
        self.storage.send(Cmd::Outbox { op: OutboxOp::Put { row: Box::new(row), blobs }, done });
    }

    pub fn delete(&self, server: &str, id: &str, done: Option<Done>) {
        if let OutboxCache::Loaded(all) = &mut self.storage.cache().outbox
            && let Some(list) = all.get_mut(server)
        {
            list.retain(|r| r.id != id);
        }
        self.storage.send(Cmd::Outbox { op: OutboxOp::Delete { server: server.to_owned(), id: id.to_owned() }, done });
    }
}

/// What the first screen of a session draws (its latest visible messages, in the timeline's own
/// encoding), per `(server, session)`. The app keeps it current; the active tab of each window
/// goes into the boot record, so the first frame draws real messages before anything else loads.
pub struct PreviewStore<'a> {
    pub(crate) storage: &'a Storage,
}

impl PreviewStore<'_> {
    /// Sets (`Some((updated_at, bytes))`) or removes a session's preview (500 ms; an unchanged
    /// one writes nothing).
    pub fn set(&self, server: &str, session: &str, value: Option<(i64, Arc<[u8]>)>) {
        let key = (server.to_owned(), session.to_owned());
        let sum = value.as_ref().map(|(at, data)| (*at, twox_hash::XxHash3_64::oneshot(data)));
        {
            let mut cache = self.storage.cache();
            if cache.preview_sums.get(&key).copied() == sum {
                return;
            }
            match sum {
                Some(s) => cache.preview_sums.insert(key, s),
                None => cache.preview_sums.remove(&key),
            };
        }
        self.storage.send(Cmd::Preview { server: server.to_owned(), session: session.to_owned(), value });
    }

    /// Reads a session's preview on the storage thread.
    pub fn get(&self, server: &str, session: &str, done: crate::PreviewRead) {
        self.storage.send(Cmd::GetPreview { server: server.to_owned(), session: session.to_owned(), done });
    }
}

/// Discardable caches of computed inputs (compiled bytecode, shaped runs), by key and version.
/// Never read at boot; a version other than the stored one reads as absent.
pub struct CacheStore<'a> {
    pub(crate) storage: &'a Storage,
}

impl CacheStore<'_> {
    pub fn put(&self, key: &str, version: i64, data: Arc<[u8]>) {
        let sum = (version, twox_hash::XxHash3_64::oneshot(&data));
        {
            let mut cache = self.storage.cache();
            if cache.cache_sums.get(key) == Some(&sum) {
                return;
            }
            cache.cache_sums.insert(key.to_owned(), sum);
        }
        self.storage.send(Cmd::Cache { key: key.to_owned(), value: Some((version, data)) });
    }

    pub fn discard(&self, key: &str) {
        self.storage.cache().cache_sums.remove(key);
        self.storage.send(Cmd::Cache { key: key.to_owned(), value: None });
    }

    /// Reads `key` at `version` on the storage thread.
    pub fn get(&self, key: &str, version: i64, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        self.storage.send(Cmd::GetCache { key: key.to_owned(), version, done });
    }
}
