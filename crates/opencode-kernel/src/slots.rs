//! Per-window typed state slots so features store their own state without adding fields to the
//! host application struct.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

/// Typed heterogeneous container keyed by `TypeId`.
///
/// Features store and retrieve per-window state through `slot`, `get`, `get_mut`, `insert`, and
/// `remove`, keeping the host application struct free of feature-specific fields.
#[derive(Default)]
pub struct StateSlots {
    entries: HashMap<TypeId, Box<dyn Any>>,
}

impl StateSlots {
    /// Creates an empty slot table.
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// Returns a mutable reference to the slot for `T`, inserting `T::default()` if absent.
    pub fn slot<T: Default + 'static>(&mut self) -> &mut T {
        let key = TypeId::of::<T>();
        if !self.entries.get(&key).is_some_and(|v| v.is::<T>()) {
            self.entries.insert(key, Box::new(T::default()));
        }
        match self.entries.get_mut(&key).and_then(|v| v.downcast_mut::<T>()) {
            Some(value) => value,
            None => unreachable!("TypeId key always maps to a boxed T"),
        }
    }

    /// Returns a shared reference to the slot for `T`, if present.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.entries.get(&TypeId::of::<T>())?.downcast_ref::<T>()
    }

    /// Returns a mutable reference to the slot for `T`, if present.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.entries.get_mut(&TypeId::of::<T>())?.downcast_mut::<T>()
    }

    /// Inserts or replaces the slot for `T`.
    pub fn insert<T: 'static>(&mut self, val: T) {
        self.entries.insert(TypeId::of::<T>(), Box::new(val));
    }

    /// Removes and returns the slot for `T`, if present.
    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.entries.remove(&TypeId::of::<T>())?.downcast::<T>().ok().map(|b| *b)
    }

    /// Returns `true` if a slot of type `T` is currently stored.
    pub fn contains<T: 'static>(&self) -> bool {
        self.entries.contains_key(&TypeId::of::<T>())
    }

    /// Number of occupied slots.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no slots are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes all stored slots.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl fmt::Debug for StateSlots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateSlots").field("len", &self.entries.len()).finish()
    }
}
