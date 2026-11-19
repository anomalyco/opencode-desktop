//! Process-wide persistence port shared by decoupled feature crates (`updater`, `settings`,
//! `projects`, `composer-input`, `inbox`, and future features).
//!
//! Host shells (`apps/desktop`, `apps/web`, or embedders) register one [`StoragePort`] once at
//! startup via [`configure`]. Feature crates call the free helpers in this module (or accept an
//! explicit [`StoragePort`] override) without defining duplicate `StorageOps` callback tables or
//! reaching into host globals.
//!
//! Calling any helper before [`configure`] runs returns a safe default (`false`, `None`, `0`,
//! [`DocLookup::Ready`]`(None)`, or a no-op) without panicking.

use std::sync::{Arc, OnceLock};

use serde_json::{Map, Value};

/// Result of reading a persisted document from memory (`Ready` with its text or `None`, or
/// `Pending` while loading from the backing store).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocLookup {
    Ready(Option<Arc<str>>),
    Pending,
}

/// Process-wide host persistence callbacks covering JSON key-value state, large documents, and
/// draft hand-offs.
#[derive(Clone, Copy)]
pub struct StoragePort {
    /// Whether the backing store has hydrated into memory.
    pub is_ready: fn() -> bool,
    /// Whether a user (or harness write flag) attends this process and global state may persist.
    pub attended: fn() -> bool,
    /// Reads a JSON value from `(namespace, key)`.
    pub read_json: fn(namespace: &str, key: &str) -> Option<Value>,
    /// Persists a JSON value under `(namespace, key)`.
    pub write_json: fn(namespace: &str, key: &str, value: &Value),
    /// Deletes `(namespace, key)` from persistent state.
    pub delete: fn(namespace: &str, key: &str),
    /// Reads document `key` from memory (or reports [`DocLookup::Pending`]).
    pub doc: fn(key: &str) -> DocLookup,
    /// Requests background loading of document `keys`, invoking `done` once in memory.
    pub load_docs: fn(keys: &[&str], done: Box<dyn FnOnce() + Send>),
    /// Writes (or clears when `None`) document `key`.
    pub set_doc: fn(key: &str, value: Option<String>),
    /// Hands off a draft document to an outbox row without writing a draft journal record,
    /// returning the journal revision settled by `OutboxPut::clear_documents`.
    pub hand_off_doc: fn(key: &str) -> u64,
}

fn noop_bool() -> bool {
    false
}

fn noop_read_json(_: &str, _: &str) -> Option<Value> {
    None
}

fn noop_write_json(_: &str, _: &str, _: &Value) {}

fn noop_delete(_: &str, _: &str) {}

fn noop_doc(_: &str) -> DocLookup {
    DocLookup::Ready(None)
}

fn noop_load_docs(_: &[&str], _: Box<dyn FnOnce() + Send>) {}

fn noop_set_doc(_: &str, _: Option<String>) {}

fn noop_hand_off_doc(_: &str) -> u64 {
    0
}

impl StoragePort {
    /// A port whose callbacks are safe no-ops returning empty/default values.
    /// Useful as a struct-update base (`StoragePort { read_json, ..StoragePort::noop() }`).
    pub const fn noop() -> Self {
        Self {
            is_ready: noop_bool,
            attended: noop_bool,
            read_json: noop_read_json,
            write_json: noop_write_json,
            delete: noop_delete,
            doc: noop_doc,
            load_docs: noop_load_docs,
            set_doc: noop_set_doc,
            hand_off_doc: noop_hand_off_doc,
        }
    }
}

impl Default for StoragePort {
    fn default() -> Self {
        Self::noop()
    }
}

static PORT: OnceLock<StoragePort> = OnceLock::new();

/// Registers the host's process-wide [`StoragePort`]. Call once during startup before feature
/// crates read or persist state; subsequent calls are ignored.
pub fn configure(port: StoragePort) {
    PORT.get_or_init(|| port);
}

/// Returns the registered process-wide [`StoragePort`], or `None` if [`configure`] was not called.
pub fn port() -> Option<&'static StoragePort> {
    PORT.get()
}

/// Whether a process-wide [`StoragePort`] has been registered via [`configure`].
pub fn is_configured() -> bool {
    PORT.get().is_some()
}

/// Whether the backing store has hydrated into memory (`false` if unconfigured).
pub fn is_ready() -> bool {
    PORT.get().is_some_and(|p| (p.is_ready)())
}

/// Whether this process is attended and global state may persist (`false` if unconfigured).
pub fn attended() -> bool {
    PORT.get().is_some_and(|p| (p.attended)())
}

/// Reads a JSON value from `(namespace, key)` (`None` if unconfigured or absent).
pub fn read_json(namespace: &str, key: &str) -> Option<Value> {
    PORT.get().and_then(|p| (p.read_json)(namespace, key))
}

/// Persists a JSON value under `(namespace, key)` (no-op if unconfigured).
pub fn write_json(namespace: &str, key: &str, value: &Value) {
    if let Some(p) = PORT.get() {
        (p.write_json)(namespace, key, value);
    }
}

/// Reads an object under `(namespace, key)`, mutates it via `f`, and writes back only if changed
/// (no-op if unconfigured).
pub fn update_json(namespace: &str, key: &str, f: impl FnOnce(&mut Map<String, Value>)) {
    let Some(p) = PORT.get() else { return };
    let before = (p.read_json)(namespace, key)
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mut after = before.clone();
    if let Some(map) = after.as_object_mut() {
        f(map);
    }
    if after != before {
        (p.write_json)(namespace, key, &after);
    }
}

/// Deletes `(namespace, key)` from persistent state (no-op if unconfigured).
pub fn delete(namespace: &str, key: &str) {
    if let Some(p) = PORT.get() {
        (p.delete)(namespace, key);
    }
}

/// Reads document `key` from memory ([`DocLookup::Ready`]`(None)` if unconfigured).
pub fn doc(key: &str) -> DocLookup {
    PORT.get().map_or(DocLookup::Ready(None), |p| (p.doc)(key))
}

/// Requests background loading of document `keys`, calling `done` once in memory (no-op if
/// unconfigured).
pub fn load_docs(keys: &[&str], done: Box<dyn FnOnce() + Send>) {
    if let Some(p) = PORT.get() {
        (p.load_docs)(keys, done);
    }
}

/// Convenience wrapper around [`load_docs`] that wakes `waker` (if provided) once `keys` finish
/// loading into memory (no-op if unconfigured).
pub fn load_docs_wake(keys: &[&str], waker: Option<rune::Waker>) {
    let Some(p) = PORT.get() else { return };
    let done: Box<dyn FnOnce() + Send> = match waker {
        Some(w) => Box::new(move || w()),
        None => Box::new(|| {}),
    };
    (p.load_docs)(keys, done);
}

/// Writes (or clears when `None`) document `key` (no-op if unconfigured).
pub fn set_doc(key: &str, value: Option<String>) {
    if let Some(p) = PORT.get() {
        (p.set_doc)(key, value);
    }
}

/// Hands off draft document `key` to an outbox row and returns its settled journal revision
/// (`0` if unconfigured).
pub fn hand_off_doc(key: &str) -> u64 {
    PORT.get().map_or(0, |p| (p.hand_off_doc)(key))
}
