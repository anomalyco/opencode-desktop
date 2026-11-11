//! OpenCode desktop persistence in one place, as Desktop keeps it (`packages/desktop/AGENTS.md:22`,
//! `packages/desktop/src/main/storage/`): one SQLite database per process on native targets,
//! `localStorage` and `IndexedDB` on `wasm32`. Callers never see a path or a file.
//!
//! - **Reads come from memory.** [`Storage::open`] hydrates what the first frame draws in one
//!   read transaction; everything else loads on the storage thread when asked ([`Lookup::Pending`]).
//! - **Writes go behind.** A write updates memory at once and queues a row: `state` and window
//!   frames coalesce for 250 ms, documents for 500 ms (Desktop's `WriteBehind`,
//!   [`machine::write_behind`]). Outbox rows and plugin enable/trust are Tier 0: committed with
//!   `synchronous = FULL` before their callback runs.
//! - **Flush hooks** ([`Storage::flush`]) land every queued row on hide, close, quit, session
//!   end and update restart; a flush with nothing queued touches nothing.

mod boot_packet;
pub mod cache;
pub mod chunk;
pub mod machine;
pub mod model;
mod stores;

#[cfg(not(target_arch = "wasm32"))]
pub mod native;
#[cfg(target_arch = "wasm32")]
pub mod web;

use std::sync::{Arc, Mutex, MutexGuard};

pub use boot_packet::{BootPacket, BootSource};
pub use cache::Lookup;
pub use model::{FlushReason, FlushReport, ImportGroup, OutboxAttachment, OutboxRow, WindowFrame};
pub use stores::{CacheStore, DocumentStore, OutboxStore, PluginStore, PreviewStore, StateStore, WindowStore};

// The same names on both targets: what differs is behind the backend.
#[cfg(not(target_arch = "wasm32"))]
use native::Backend;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{BootReport, Opening, StorageConfig, desktop_recent_tabs};
#[cfg(target_arch = "wasm32")]
use web::Backend;
#[cfg(target_arch = "wasm32")]
pub use web::{BootReport, Opening, StorageConfig, desktop_recent_tabs};

/// Where the app keeps its state, as the app knows it; each backend takes what it uses. Native:
/// `opencode.sqlite` in `data`, the legacy files there (and the updater's in `updates`) imported
/// for the cut-over `groups`, a fresh profile seeded from Desktop's `official` folders; the old
/// files' server addresses are classified with `sidecar` (the app's own server's). Web: the
/// browser's stores; the folders mean nothing there.
pub struct AppStorage {
    pub data: Option<std::path::PathBuf>,
    pub updates: Option<std::path::PathBuf>,
    pub official: Vec<std::path::PathBuf>,
    pub sidecar: Vec<String>,
    pub groups: Vec<ImportGroup>,
    /// Wall-clock milliseconds since the epoch.
    pub clock: fn() -> i64,
}

/// A storage failure handed to a Tier 0 callback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageError(pub String);

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Tier 0 write's answer: runs on the storage thread once the row is durable (or failed).
pub type Done = Box<dyn FnOnce(Result<(), StorageError>) + Send>;
/// A session preview read's answer: its `updated_at` and bytes.
pub type PreviewRead = Box<dyn FnOnce(Option<(i64, Vec<u8>)>) + Send>;
/// Runs on the storage thread once a load put its rows in memory: wake the UI with it.
pub type Loaded = Box<dyn FnOnce() + Send>;

/// The `state` namespaces the first frame draws, carried whole in the boot record and hydrated
/// at open: the window list and app settings (`opencode.settings`), the app-wide keys
/// (`opencode.global.dat`), the small plugin namespaces read before the first frame
/// (`plugin.file`, `plugin.browser`, `plugin.updater`), the developer tools' (`rune.dev`), and
/// every window's tabs, page and panes (`opencode.window.<id>.dat`). The rest (Luau plugin
/// stores, terminal workspaces, the legacy archive) load on the storage thread when asked.
/// Everything here is read before the first frame on every launch: keep it to what that frame
/// draws, and large values out (a record past 64 KiB costs a positioned read per page).
pub const BOOT_NAMESPACES: [&str; 6] = ["opencode.settings", "opencode.global.dat", "plugin.file", "plugin.browser", "plugin.updater", "rune.dev"];

pub fn is_boot_namespace(name: &str) -> bool {
    BOOT_NAMESPACES.contains(&name) || name.starts_with("opencode.window.")
}

/// Every boot namespace's rows, as SQL (the boot record's rebuild and stage B's read).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn boot_state_query() -> &'static str {
    static SQL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SQL.get_or_init(|| {
        let names: Vec<String> = BOOT_NAMESPACES.iter().map(|n| format!("'{n}'")).collect();
        format!("SELECT name, key, value FROM state WHERE name IN ({}) OR (name >= 'opencode.window.' AND name < 'opencode.window/')", names.join(", "))
    })
}

/// A window's `state` namespace (Desktop's `windowStorage`, `persistence/storage.ts:366-370`).
pub fn window_namespace(window_id: &str) -> String {
    let safe: String = window_id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    format!("opencode.window.{}.dat", if safe.is_empty() { "browser" } else { &safe })
}

/// The document a window's draft for tab `tab_key` (its encoded `TabKey`) is kept under.
pub fn draft_document(window_id: &str, tab_key: &str) -> String {
    format!("{}:draft:{tab_key}", window_namespace(window_id))
}

/// A plugin's own `state` namespace (Desktop's `plugin/storage.ts:31`).
pub fn plugin_namespace(plugin_id: &str) -> String {
    format!("plugin.{plugin_id}")
}

/// The sent-prompt history document (normal and shell lists).
pub const PROMPT_HISTORY_DOCUMENT: &str = "opencode.global.dat:prompt-history.v1";

pub(crate) enum OutboxOp {
    Put { row: Box<OutboxRow>, blobs: Vec<(String, Arc<[u8]>)> },
    Delete { server: String, id: String },
}

/// What the stores send the storage thread (or the web backend).
pub(crate) enum Cmd {
    State { name: String, key: String, value: Option<Arc<str>> },
    ClearState { name: String },
    Doc { key: String, value: Option<Arc<str>> },
    Blob { id: String, data: Arc<[u8]> },
    Window { id: String, frame: Option<WindowFrame> },
    Plugin { id: String, enabled: Option<bool>, done: Option<Done> },
    Trust { id: String, sha256: Option<String>, done: Option<Done> },
    Outbox { op: OutboxOp, done: Option<Done> },
    LoadState { names: Vec<String>, done: Option<Loaded> },
    LoadDocs { keys: Vec<String>, done: Option<Loaded> },
    LoadOutbox { done: Option<Loaded> },
    GetBlob { id: String, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send> },
    Preview { server: String, session: String, value: Option<(i64, Arc<[u8]>)> },
    GetPreview { server: String, session: String, done: PreviewRead },
    Cache { key: String, value: Option<(i64, Arc<[u8]>)> },
    GetCache { key: String, version: i64, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send> },
}

/// The process's storage. Clone it freely: every clone shares one cache and one writer.
#[derive(Clone)]
pub struct Storage {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) cache: Arc<Mutex<cache::Cache>>,
    pub(crate) backend: Backend,
}

impl Storage {
    /// Storage that keeps everything in memory for this run: fixtures, tests, unattended runs.
    pub fn memory() -> Storage {
        let cache = Arc::new(Mutex::new(cache::Cache { complete: true, outbox: cache::OutboxCache::Loaded(Default::default()), ..Default::default() }));
        Storage { inner: Arc::new(Inner { backend: Backend::memory(), cache }) }
    }

    pub(crate) fn from_parts(cache: Arc<Mutex<cache::Cache>>, backend: Backend) -> Storage {
        Storage { inner: Arc::new(Inner { cache, backend }) }
    }

    pub fn state(&self) -> StateStore<'_> {
        StateStore { storage: self }
    }

    pub fn documents(&self) -> DocumentStore<'_> {
        DocumentStore { storage: self }
    }

    pub fn windows(&self) -> WindowStore<'_> {
        WindowStore { storage: self }
    }

    pub fn plugins(&self) -> PluginStore<'_> {
        PluginStore { storage: self }
    }

    pub fn outbox(&self) -> OutboxStore<'_> {
        OutboxStore { storage: self }
    }

    pub fn previews(&self) -> PreviewStore<'_> {
        PreviewStore { storage: self }
    }

    pub fn caches(&self) -> CacheStore<'_> {
        CacheStore { storage: self }
    }

    /// Lands every queued row and waits up to `deadline` for it. Call on window hide, close,
    /// app quit, session end and update restart. Nothing queued: returns at once, touching nothing.
    pub fn flush(&self, reason: FlushReason, deadline: std::time::Duration) -> FlushReport {
        self.inner.backend.flush(reason, deadline)
    }

    /// The first frame is on screen: the work that waited for it (removing imported files, the
    /// first-run seed from Desktop's database, the blob sweep's clock) starts on the storage
    /// thread. `done` runs there once a seed put new rows in memory.
    pub fn first_frame_presented(&self, done: Option<Loaded>) {
        self.inner.backend.first_frame(done);
    }

    /// Flushes and stops the writer. The database closes; later writes stay in memory only.
    pub fn close(&self) -> FlushReport {
        self.inner.backend.close()
    }

    pub(crate) fn cache(&self) -> MutexGuard<'_, cache::Cache> {
        self.inner.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn send(&self, cmd: Cmd) {
        self.inner.backend.send(cmd);
    }
}
