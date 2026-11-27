//! A page asked for before its reader existed: the launch asks for the restored session's newest
//! page beside its first reads (`launch::boot`), a round trip before the window connects and its
//! reader can ask.

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use opencode_kit::client::Server;
use rune::time::Instant;
use serde_json::Value;

use super::Fetch;

/// The reader's ask for `path` takes this read's answer instead of asking again, while it is
/// fresh: the answer may still be on its way, and the reader waits for it as it would for its own.
pub struct Early {
    path: String,
    /// The answer when it comes: none when the read failed (the reader then asks itself).
    answer: Receiver<Option<Value>>,
    asked: Instant,
}

/// How long an early page stands in for a read: the launch's own connect comes milliseconds
/// after it; a session opened later than this reads its page anew.
const FRESH: Duration = Duration::from_secs(10);

impl Early {
    /// Asks `server` for `path` on a thread of its own.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn ask(server: &Server, path: String) -> Option<Early> {
        let (out, answer) = std::sync::mpsc::channel();
        let (server, asked_path) = (server.clone(), path.clone());
        std::thread::Builder::new()
            .name("opencode-first-page".into())
            .spawn(move || {
                out.send(server.read(&asked_path).ok()).ok();
            })
            .ok()?;
        Some(Early { path, answer, asked: Instant::now() })
    }
}

/// The live reader's reads of `server`, the first for `early`'s path answered by it.
pub(super) fn fetch(server: Server, early: Option<Early>) -> Fetch {
    let early = Mutex::new(early);
    Arc::new(move |path| {
        let taken = {
            let mut slot = early.lock().unwrap_or_else(PoisonError::into_inner);
            match &*slot {
                Some(e) if e.asked.elapsed() >= FRESH => {
                    *slot = None;
                    None
                }
                Some(e) if e.path == path => slot.take(),
                _ => None,
            }
        };
        // Its sender is gone when its thread is: a read that never answers ends with its timeout.
        if let Some(Ok(Some(value))) = taken.map(|early| early.answer.recv()) {
            return Ok(value);
        }
        server.read_timed(path).map(|(value, t)| {
            crate::telemetry::note_read(t.first_byte, t.bytes, t.parsing);
            value
        })
    })
}