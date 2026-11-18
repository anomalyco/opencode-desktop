//! Explicit per-server state pool keyed by [`opencode_kit::servers::ServerKey`].
//!
//! Replaces `thread_local` globals and `ServerData::swap` checkout shuffles with a direct
//! read-through store: each server keeps its own entry `T` addressed explicitly by key, while an
//! explicit active key tracks whichever server the caller currently targets.

use std::collections::HashMap;
use std::collections::hash_map;

use opencode_kit::servers::ServerKey;

/// Per-server state container keyed by [`ServerKey`] with an explicit active server selection.
#[derive(Clone, Debug)]
pub struct ServerPool<T> {
    active: ServerKey,
    entries: HashMap<ServerKey, T>,
}

impl<T> Default for ServerPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ServerPool<T> {
    /// Creates an empty pool whose active server starts as [`ServerKey::sidecar()`].
    pub fn new() -> Self {
        Self { active: ServerKey::sidecar(), entries: HashMap::new() }
    }

    /// Creates an empty pool whose active server starts as `active`.
    pub fn with_active(active: ServerKey) -> Self {
        Self { active, entries: HashMap::new() }
    }

    /// Key of the currently active server.
    pub fn active_key(&self) -> &ServerKey {
        &self.active
    }

    /// Switches the active server to `key`. Returns `true` when the active key changed.
    pub fn set_active(&mut self, key: ServerKey) -> bool {
        if self.active == key {
            return false;
        }
        self.active = key;
        true
    }

    /// Returns a shared reference to the entry for `key`, if present.
    pub fn get(&self, key: &ServerKey) -> Option<&T> {
        self.entries.get(key)
    }

    /// Returns a mutable reference to the entry for `key`, if present.
    pub fn get_mut(&mut self, key: &ServerKey) -> Option<&mut T> {
        self.entries.get_mut(key)
    }

    /// Returns a shared reference to the active server's entry, if present.
    pub fn active(&self) -> Option<&T> {
        self.entries.get(&self.active)
    }

    /// Returns a mutable reference to the active server's entry, if present.
    pub fn active_mut(&mut self) -> Option<&mut T> {
        self.entries.get_mut(&self.active)
    }

    /// Returns a mutable reference to the entry for `key`, inserting `T::default()` if absent.
    pub fn entry_or_default(&mut self, key: &ServerKey) -> &mut T
    where
        T: Default,
    {
        if !self.entries.contains_key(key) {
            self.entries.insert(key.clone(), T::default());
        }
        match self.entries.get_mut(key) {
            Some(entry) => entry,
            None => unreachable!("key was inserted above"),
        }
    }

    /// Returns a mutable reference to the active server's entry, inserting `T::default()` if
    /// absent.
    pub fn active_or_default(&mut self) -> &mut T
    where
        T: Default,
    {
        if !self.entries.contains_key(&self.active) {
            self.entries.insert(self.active.clone(), T::default());
        }
        match self.entries.get_mut(&self.active) {
            Some(entry) => entry,
            None => unreachable!("active key was inserted above"),
        }
    }

    /// Inserts or replaces the entry for `key`, returning the previous value if any.
    pub fn insert(&mut self, key: ServerKey, value: T) -> Option<T> {
        self.entries.insert(key, value)
    }

    /// Removes and returns the entry for `key`, if present.
    pub fn remove(&mut self, key: &ServerKey) -> Option<T> {
        self.entries.remove(key)
    }

    /// Returns `true` if the pool contains an entry for `key`.
    pub fn contains(&self, key: &ServerKey) -> bool {
        self.entries.contains_key(key)
    }

    /// Iterates over the keys of all stored entries.
    pub fn keys(&self) -> hash_map::Keys<'_, ServerKey, T> {
        self.entries.keys()
    }

    /// Iterates over `(&ServerKey, &T)` pairs for all stored entries.
    pub fn iter(&self) -> hash_map::Iter<'_, ServerKey, T> {
        self.entries.iter()
    }

    /// Iterates mutably over `(&ServerKey, &mut T)` pairs for all stored entries.
    pub fn iter_mut(&mut self) -> hash_map::IterMut<'_, ServerKey, T> {
        self.entries.iter_mut()
    }

    /// Number of stored server entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no server entries are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes all stored entries while keeping the current active key.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl<'a, T> IntoIterator for &'a ServerPool<T> {
    type Item = (&'a ServerKey, &'a T);
    type IntoIter = hash_map::Iter<'a, ServerKey, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut ServerPool<T> {
    type Item = (&'a ServerKey, &'a mut T);
    type IntoIter = hash_map::IterMut<'a, ServerKey, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
