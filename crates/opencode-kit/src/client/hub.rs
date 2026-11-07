//! One event stream for several windows of one app: each window has its own fetch and write
//! threads, answering only it, while `/api/event` is read once and every event, reconnect and
//! stream error reaches every window that subscribed.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::Waker;

use super::worker::{read_events, spawn_workers};
use super::{Incoming, Request, Server, Write};

/// The shared event stream of one server. Its thread runs until the process ends.
pub struct EventHub {
    base: String,
    subscribers: Arc<Mutex<Vec<(Sender<Incoming>, Waker)>>>,
}

impl EventHub {
    /// Opens `server`'s event stream, dropped and reopened after `idle` of silence.
    pub fn start(server: Server, idle: Duration) -> Arc<EventHub> {
        let subscribers: Arc<Mutex<Vec<(Sender<Incoming>, Waker)>>> = Arc::default();
        let (events, inbox) = channel();
        let fan = subscribers.clone();
        std::thread::Builder::new()
            .name("opencode-events-fan".into())
            .spawn(move || fan_out(&inbox, &fan))
            .expect("spawn event fan-out thread");
        let base = server.base.clone();
        let wake: Waker = Arc::new(|| {});
        std::thread::Builder::new()
            .name("opencode-events".into())
            .spawn(move || read_events(&server, idle, &events, &wake))
            .expect("spawn event thread");
        Arc::new(EventHub { base, subscribers })
    }

    /// The server whose stream this is.
    pub fn base(&self) -> &str {
        &self.base
    }

    fn subscribe(&self, inbox: Sender<Incoming>, wake: Waker) {
        self.subscribers.lock().unwrap().push((inbox, wake));
    }
}

/// Every stream message to every window still listening, each woken once per batch.
fn fan_out(inbox: &Receiver<Incoming>, subscribers: &Mutex<Vec<(Sender<Incoming>, Waker)>>) {
    while let Ok(first) = inbox.recv() {
        let batch: Vec<Incoming> = std::iter::once(first).chain(inbox.try_iter()).collect();
        let mut subs = subscribers.lock().unwrap();
        subs.retain(|(tx, wake)| {
            let alive = batch.iter().all(|m| tx.send(m.clone()).is_ok());
            if alive {
                wake();
            }
            alive
        });
    }
}

/// [`super::connect_with_idle`] for one window of several: its own fetch and write threads,
/// and `hub`'s event stream in place of one of its own.
pub fn connect_shared(server: Server, wake: Waker, hub: &EventHub) -> (Sender<Request>, Sender<Write>, Receiver<Incoming>) {
    let (inbox_tx, inbox) = channel();
    let (req_tx, write_tx) = spawn_workers(&server, &inbox_tx, &wake);
    hub.subscribe(inbox_tx, wake);
    (req_tx, write_tx, inbox)
}
