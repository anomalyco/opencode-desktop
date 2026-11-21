//! Weak read-only handle to a window's `Pages` prefetch cache so another window's `App::rest`
//! preserves process-global user-message metadata for pages prefetched in this window.

use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};

use opencode_kit::model::Message;

use super::{QueueState, State};

pub(crate) struct CacheRef(Weak<(Mutex<QueueState>, Condvar)>);

impl CacheRef {
    pub(crate) fn user_message_ids(&self) -> Vec<String> {
        let Some(shared) = self.0.upgrade() else { return Vec::new() };
        let guard = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
        let QueueState::Running(q) = &*guard else { return Vec::new() };
        q.ready
            .iter()
            .flat_map(|p| p.messages.iter())
            .filter_map(|m| match m {
                Message::User { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }
}

impl State {
    pub(crate) fn cache_ref(&self) -> Option<CacheRef> {
        self.pool.as_ref().map(|p| CacheRef(Arc::downgrade(&p.shared)))
    }

    pub(crate) fn same_pool(&self, r: &CacheRef) -> bool {
        self.pool.as_ref().is_some_and(|p| r.0.as_ptr() == Arc::as_ptr(&p.shared))
    }
}
