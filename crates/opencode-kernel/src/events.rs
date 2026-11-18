//! Typed deferred event queue for decoupled cross-feature communication within a window.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

/// Typed deferred event bus keyed by `TypeId`.
///
/// Features emit strongly-typed events during one phase (or input handler) and downstream
/// features drain them in a later phase without holding direct references to each other.
#[derive(Default)]
pub struct EventBus {
    queues: HashMap<TypeId, Box<dyn Any>>,
}

impl EventBus {
    /// Creates an empty event bus.
    pub fn new() -> Self {
        Self { queues: HashMap::new() }
    }

    /// Appends `event` to the deferred queue for type `E`.
    pub fn emit<E: 'static>(&mut self, event: E) {
        let key = TypeId::of::<E>();
        if !self.queues.get(&key).is_some_and(|q| q.is::<Vec<E>>()) {
            self.queues.insert(key, Box::new(Vec::<E>::new()));
        }
        if let Some(queue) = self.queues.get_mut(&key).and_then(|q| q.downcast_mut::<Vec<E>>()) {
            queue.push(event);
        }
    }

    /// Drains and returns all queued events of type `E` in FIFO order.
    pub fn drain<E: 'static>(&mut self) -> Vec<E> {
        let Some(boxed) = self.queues.remove(&TypeId::of::<E>()) else {
            return Vec::new();
        };
        match boxed.downcast::<Vec<E>>() {
            Ok(queue) => *queue,
            Err(_) => Vec::new(),
        }
    }

    /// Returns `true` if at least one event of type `E` is queued.
    pub fn has<E: 'static>(&self) -> bool {
        self.pending::<E>() > 0
    }

    /// Number of queued events of type `E`.
    pub fn pending<E: 'static>(&self) -> usize {
        self.queues
            .get(&TypeId::of::<E>())
            .and_then(|q| q.downcast_ref::<Vec<E>>())
            .map_or(0, Vec::len)
    }

    /// Drops all queued events of type `E`.
    pub fn clear_type<E: 'static>(&mut self) {
        self.queues.remove(&TypeId::of::<E>());
    }

    /// Drops all queued events of every type.
    pub fn clear(&mut self) {
        self.queues.clear();
    }

    /// Returns `true` when no event queues exist.
    pub fn is_empty(&self) -> bool {
        self.queues.is_empty()
    }
}

impl fmt::Debug for EventBus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventBus").field("queued_types", &self.queues.len()).finish()
    }
}
