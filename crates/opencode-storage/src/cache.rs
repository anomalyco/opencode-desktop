//! The in-memory copy every read hits: hydrated once at open for what the first frame draws,
//! filled on the storage thread for the rest. A write updates it at once and bumps its
//! namespace's revision, so sibling windows see it on their next poll with no disk round trip.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::model::{OutboxRow, WindowFrame};

/// A `state` namespace in memory.
#[derive(Debug)]
pub(crate) enum Namespace {
    /// Every stored row is here.
    Loaded { rows: BTreeMap<String, Arc<str>>, revision: u64 },
    /// Not read yet (a namespace the first frame doesn't draw). `written` holds what was set
    /// (`Some`) or deleted (`None`) since open; a load never overwrites those.
    Unloaded { written: BTreeMap<String, Option<Arc<str>>>, revision: u64 },
}

impl Namespace {
    pub fn revision(&self) -> u64 {
        match self {
            Namespace::Loaded { revision, .. } | Namespace::Unloaded { revision, .. } => *revision,
        }
    }

    fn set(&mut self, key: String, value: Option<Arc<str>>) {
        match self {
            Namespace::Loaded { rows, revision } => {
                match value {
                    Some(v) => rows.insert(key, v),
                    None => rows.remove(&key),
                };
                *revision += 1;
            }
            Namespace::Unloaded { written, revision } => {
                written.insert(key, value);
                *revision += 1;
            }
        }
    }
}

/// A document in memory, decoded (its text chunks put back).
#[derive(Clone, Debug)]
pub(crate) enum Doc {
    Present(Arc<str>),
    /// Stored but not read yet: it loads on the storage thread.
    Stored,
}

/// What the outbox holds in memory.
#[derive(Debug, Default)]
pub(crate) enum OutboxCache {
    #[default]
    Unloaded,
    Loaded(BTreeMap<String, Vec<OutboxRow>>),
}

#[derive(Debug, Default)]
pub(crate) struct Cache {
    pub state: HashMap<String, Namespace>,
    pub docs: HashMap<String, Doc>,
    pub windows: BTreeMap<String, WindowFrame>,
    pub plugins: BTreeMap<String, bool>,
    pub trust: BTreeMap<String, String>,
    pub outbox: OutboxCache,
    /// What each session preview and cache entry written this run holds (`updated_at` or
    /// version, and xxh3), so writing an unchanged one costs nothing.
    pub preview_sums: HashMap<(String, String), (i64, u64)>,
    pub cache_sums: HashMap<String, (i64, u64)>,
    /// Every namespace is in memory: an in-memory store, or a fresh database.
    pub complete: bool,
    /// Whether every stored document's key is known (the web backend learns them once its
    /// database opens): until then an unknown key may still be stored.
    pub docs_index: DocsIndex,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DocsIndex {
    #[default]
    Ready,
    Loading,
}

impl Cache {
    /// A namespace the cache has no entry for: a boot namespace was read in full at open (it has
    /// no rows); any other may have rows on disk until loaded.
    fn absent(&self, name: &str) -> Namespace {
        if self.complete || crate::is_boot_namespace(name) {
            return Namespace::Loaded { rows: BTreeMap::new(), revision: 0 };
        }
        Namespace::Unloaded { written: BTreeMap::new(), revision: 0 }
    }

    pub fn get(&self, name: &str, key: &str) -> Lookup {
        match self.state.get(name) {
            Some(Namespace::Loaded { rows, .. }) => Lookup::Ready(rows.get(key).cloned()),
            Some(Namespace::Unloaded { written, .. }) => match written.get(key) {
                Some(v) => Lookup::Ready(v.clone()),
                None => Lookup::Pending,
            },
            None if self.complete || crate::is_boot_namespace(name) => Lookup::Ready(None),
            None => Lookup::Pending,
        }
    }

    pub fn set(&mut self, name: &str, key: String, value: Option<Arc<str>>) -> u64 {
        if !self.state.contains_key(name) {
            let fresh = self.absent(name);
            self.state.insert(name.to_owned(), fresh);
        }
        let Some(ns) = self.state.get_mut(name) else { return 0 };
        ns.set(key, value);
        ns.revision()
    }

    /// Rows read from disk for `name`: what was written since open wins.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn hydrate(&mut self, name: &str, mut rows: BTreeMap<String, Arc<str>>) {
        let revision = match self.state.remove(name) {
            Some(Namespace::Loaded { rows: current, revision }) => {
                self.state.insert(name.to_owned(), Namespace::Loaded { rows: current, revision });
                return;
            }
            Some(Namespace::Unloaded { written, revision }) => {
                for (k, v) in written {
                    match v {
                        Some(v) => rows.insert(k, v),
                        None => rows.remove(&k),
                    };
                }
                revision + 1
            }
            None => 1,
        };
        self.state.insert(name.to_owned(), Namespace::Loaded { rows, revision });
    }

    pub fn items(&self, name: &str) -> Option<Vec<(String, Arc<str>)>> {
        match self.state.get(name) {
            Some(Namespace::Loaded { rows, .. }) => Some(rows.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            Some(Namespace::Unloaded { .. }) => None,
            None if self.complete || crate::is_boot_namespace(name) => Some(Vec::new()),
            None => None,
        }
    }

    pub fn clear(&mut self, name: &str) -> u64 {
        let revision = self.state.get(name).map_or(0, Namespace::revision) + 1;
        self.state.insert(name.to_owned(), Namespace::Loaded { rows: BTreeMap::new(), revision });
        revision
    }
}

/// A read from memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Ready(Option<Arc<str>>),
    /// Stored on disk and not read yet; ask the store to load it.
    Pending,
}
