//! Every event stream this process reads, so a network change or a wake from sleep can check
//! them all at once ([`verify`]). After either, no stream that looks up can be trusted: a
//! connection that died while the machine slept, or whose route went with the old network, just
//! goes quiet, and its read would wait out the idle limit (25 s) before anyone knew. So each is
//! suspect: its server is asked once, on a fresh connection with a 1 s bound, whether it answers,
//! and the stream is replaced unless it has spoken since. A server that doesn't answer is said to
//! be unreachable at once (`Incoming::Unanswered`, typed), and its new stream parks on the reach
//! watcher. Nothing polls: it runs when the OS says the network changed or the machine woke.
//!
//! A replaced stream's thread can't be stopped mid-read, so it's left to end on its own: the
//! stream it reads no longer speaks for its server ([`Stream::live`]), sends nothing more, and
//! ends at its next line or its idle limit.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rune::Waker;
use rune::time::Instant;

use super::{Incoming, Server, http};
use crate::wire_error::{WireError, transport};

/// How long a verifying request may take: past it, the server counts as not answering.
const CHECK: Duration = Duration::from_secs(1);

/// One server's event stream for one inbox (a window, or a hub of several).
pub(super) struct Stream {
    pub server: Server,
    pub idle: Duration,
    pub inbox: Sender<Incoming>,
    pub wake: Waker,
    /// Which of the stream's threads speaks for it; a replaced one stops at its next turn.
    epoch: AtomicU64,
    /// When the stream last read a line, heartbeats included (µs since [`Stream::born`]).
    heard: AtomicU64,
    born: Instant,
}

static STREAMS: Mutex<Vec<Weak<Stream>>> = Mutex::new(Vec::new());

impl Stream {
    /// A stream to read, known to [`verify`] for as long as a thread reads it.
    pub fn new(server: Server, idle: Duration, inbox: Sender<Incoming>, wake: Waker) -> Arc<Stream> {
        let s = Arc::new(Stream { server, idle, inbox, wake, epoch: AtomicU64::new(0), heard: AtomicU64::new(0), born: Instant::now() });
        let mut all = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|w| w.strong_count() > 0);
        all.push(Arc::downgrade(&s));
        s
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Whether the thread reading as `epoch` still speaks for the stream.
    pub fn live(&self, epoch: u64) -> bool {
        self.epoch() == epoch
    }

    pub fn heard(&self) {
        self.heard.store(self.born.elapsed().as_micros() as u64, Ordering::Release);
    }

    fn heard_since(&self, at: Instant) -> bool {
        self.born + Duration::from_micros(self.heard.load(Ordering::Acquire)) > at
    }
}

/// After a network change or a wake: each stream's server is asked once whether it answers, and
/// each stream that hasn't spoken since is replaced by a fresh one (one whose server didn't
/// answer says so first). Returns at once; the checks run on their own threads.
pub fn verify() {
    let at = Instant::now();
    let all: Vec<Arc<Stream>> = STREAMS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade).collect();
    for s in all {
        std::thread::Builder::new().name("opencode-verify".into()).spawn(move || check(&s, at)).ok();
    }
}

fn check(s: &Arc<Stream>, at: Instant) {
    let answer = answers(&s.server);
    if s.heard_since(at) {
        return;
    }
    if let Err(e) = answer {
        // Said before the new stream's own try, which may take its time to fail.
        if s.inbox.send(Incoming::Unanswered(e)).is_err() {
            return;
        }
        (s.wake)();
    }
    replace(s);
}

/// Whether `server` answers at all, on a connection of its own (a pooled one may be the dead
/// one): any HTTP answer counts, a refusal or silence past [`CHECK`] doesn't.
fn answers(server: &Server) -> Result<(), WireError> {
    let config = ureq::Agent::config_builder().timeout_global(Some(CHECK)).http_status_as_error(false).max_redirects(0).max_idle_connections(0);
    let agent: ureq::Agent = if http::loopback(&server.base) { config.proxy(None) } else { config }.build().into();
    let mut req = agent.get(format!("{}/api/info", server.base));
    if let Some(a) = &server.auth {
        req = req.header("Authorization", a);
    }
    req.call().map(drop).map_err(|e| WireError::new("GET", "/api/info", transport(e)))
}

/// A new thread speaks for the stream from now; the old one ends quietly.
fn replace(s: &Arc<Stream>) {
    let epoch = s.epoch.fetch_add(1, Ordering::AcqRel) + 1;
    let s = s.clone();
    std::thread::Builder::new().name("opencode-events".into()).spawn(move || super::worker::run_stream(&s, epoch, true)).ok();
}
