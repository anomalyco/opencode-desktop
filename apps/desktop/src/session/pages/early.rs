//! A page read before its reader existed: the launch reads the restored session's newest page
//! beside its first reads (`launch::boot`), a round trip before the window connects and its
//! reader can ask.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use opencode_kit::client::Server;
use rune::time::Instant;
use serde_json::Value;

use super::Fetch;

/// The reader's ask for `path` takes `value` instead of reading it again, while it is fresh.
pub struct Early {
    pub path: String,
    pub value: Value,
    pub read: Instant,
}

/// How long an early page stands in for a read: the launch's own connect comes milliseconds
/// after it; a session opened later than this reads its page anew.
const FRESH: Duration = Duration::from_secs(10);

/// The live reader's reads of `server`, the first for `early`'s path answered by it.
pub(super) fn fetch(server: Server, early: Option<Early>) -> Fetch {
    let early = Mutex::new(early);
    Arc::new(move |path| {
        let taken = {
            let mut slot = early.lock().unwrap_or_else(PoisonError::into_inner);
            match &*slot {
                Some(e) if e.read.elapsed() >= FRESH => {
                    *slot = None;
                    None
                }
                Some(e) if e.path == path => slot.take(),
                _ => None,
            }
        };
        if let Some(early) = taken {
            return Ok(early.value);
        }
        server.read_timed(path).map(|(value, t)| {
            crate::telemetry::note_read(t.first_byte, t.bytes, t.parsing);
            value
        })
    })
}
