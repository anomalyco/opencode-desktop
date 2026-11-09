//! One event stream for several windows of one app: each window has its own fetch and write
//! threads, answering only it (`connect_workers`), while `/api/event` is read once per server
//! and every event, reconnect and stream error reaches whoever subscribed to its hub.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rune::Waker;

use super::worker::{read_events, spawn_workers};
use super::{Incoming, Request, Server, Write};

/// The shared event stream of one server. Its threads run until it drops and the stream next
/// speaks (the server's heartbeat comes every 15 s).
pub struct EventHub {
    base: String,
    fan: Arc<Mutex<Fan>>,
}

/// Where a hub's stream goes.
enum Fan {
    /// To these subscribers, each woken once per batch.
    Open(Vec<(Sender<Incoming>, Waker)>),
    /// Nowhere: the hub dropped, and its threads end.
    Closed,
}

impl EventHub {
    /// Opens `server`'s event stream, dropped and reopened after `idle` of silence.
    pub fn start(server: Server, idle: Duration) -> EventHub {
        let fan = Arc::new(Mutex::new(Fan::Open(Vec::new())));
        let (events, inbox) = channel();
        let to = fan.clone();
        std::thread::Builder::new()
            .name("opencode-events-fan".into())
            .spawn(move || fan_out(&inbox, &to))
            .expect("spawn event fan-out thread");
        let base = server.base.clone();
        let wake: Waker = Arc::new(|| {});
        std::thread::Builder::new()
            .name("opencode-events".into())
            .spawn(move || read_events(&server, idle, &events, &wake))
            .expect("spawn event thread");
        EventHub { base, fan }
    }

    /// The server whose stream this is.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Everything the stream says from now on, with `wake` called once per batch.
    pub fn subscribe(&self, wake: Waker) -> Receiver<Incoming> {
        let (tx, rx) = channel();
        if let Fan::Open(subs) = &mut *self.fan.lock().unwrap_or_else(|e| e.into_inner()) {
            subs.push((tx, wake));
        }
        rx
    }
}

impl Drop for EventHub {
    fn drop(&mut self) {
        *self.fan.lock().unwrap_or_else(|e| e.into_inner()) = Fan::Closed;
    }
}

/// The key a stream event's data carries the time the hub heard it under, in microseconds from
/// the process's first hub (`heard_at` reads it back), for timing an event to the frame that
/// shows it.
pub const HUB_AT: &str = "hubAt";

static EPOCH: OnceLock<Instant> = OnceLock::new();

/// When the hub heard stream event `data`.
pub fn heard_at(data: &serde_json::Value) -> Option<Instant> {
    Some(*EPOCH.get()? + Duration::from_micros(data[HUB_AT].as_u64()?))
}

/// Every stream message to every subscriber still listening, each woken once per batch, until
/// the hub closes; then the stream thread's next send fails and it ends too.
fn fan_out(inbox: &Receiver<Incoming>, fan: &Mutex<Fan>) {
    let epoch = *EPOCH.get_or_init(Instant::now);
    while let Ok(first) = inbox.recv() {
        let heard = epoch.elapsed().as_micros() as u64;
        let mut batch: Vec<Incoming> = std::iter::once(first).chain(inbox.try_iter()).collect();
        for m in &mut batch {
            if let Incoming::Event { data, .. } = m
                && let Some(map) = data.as_object_mut()
            {
                map.insert(HUB_AT.into(), heard.into());
            }
        }
        let Fan::Open(subs) = &mut *fan.lock().unwrap_or_else(|e| e.into_inner()) else { return };
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
/// answering into the receiver; the event stream comes from its server's [`EventHub`].
pub fn connect_workers(server: Server, wake: Waker) -> (Sender<Request>, Sender<Write>, Receiver<Incoming>) {
    let (inbox_tx, inbox) = channel();
    let (req_tx, write_tx) = spawn_workers(&server, &inbox_tx, &wake);
    (req_tx, write_tx, inbox)
}
