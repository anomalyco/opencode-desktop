//! One event stream for several windows of one app: each window has its own fetch and write
//! threads, answering only it (`connect_workers`), while `/api/event` is read once per server
//! and every event, reconnect and stream error reaches whoever subscribed to its hub.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rune::Waker;
use rune::time::Instant;

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
    Open(Vec<Sub>),
    /// Nowhere: the hub dropped, and its threads end.
    Closed,
}

/// One subscriber: its channel, its waker, and the one event type it takes, if it takes only one.
struct Sub {
    tx: Sender<Incoming>,
    wake: Waker,
    only: Option<&'static str>,
}

impl Sub {
    fn wants(&self, m: &Incoming) -> bool {
        self.only.is_none_or(|ty| matches!(m, Incoming::Event { ty: t, .. } if t == ty))
    }

    /// Sends what it wants of `batch`, woken once if that's anything; false once it stopped listening.
    fn send(&self, batch: &[Incoming]) -> bool {
        let mut sent = false;
        for m in batch.iter().filter(|m| self.wants(m)) {
            if self.tx.send(m.clone()).is_err() {
                return false;
            }
            sent = true;
        }
        if sent {
            (self.wake)();
        }
        true
    }
}

impl EventHub {
    /// Opens `server`'s event stream, dropped and reopened after `idle` of silence.
    pub fn start(server: Server, idle: Duration) -> EventHub {
        let fan = Arc::new(Mutex::new(Fan::Open(Vec::new())));
        let (events, inbox) = channel();
        let to = fan.clone();
        let base = server.base.clone();
        let wake: Waker = Arc::new(|| {});
        // The page has no threads: the stream is a task on its event loop, and Rune drains what
        // it heard to the subscribers.
        #[cfg(target_arch = "wasm32")]
        {
            if let Err(e) = rune::task::serve("opencode-events-fan", inbox, move |m| fan_one(m, &to)) {
                eprintln!("opencode-kit: couldn't start opencode-events-fan task: {e}");
                *fan.lock().unwrap_or_else(|err| err.into_inner()) = Fan::Closed;
            } else {
                read_events(&server, idle, &events, &wake);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Err(e) = std::thread::Builder::new().name("opencode-events-fan".into()).spawn(move || fan_out(&inbox, &to)) {
                eprintln!("opencode-kit: couldn't spawn opencode-events-fan thread: {e}");
                *fan.lock().unwrap_or_else(|err| err.into_inner()) = Fan::Closed;
            } else if let Err(e) = std::thread::Builder::new()
                .name("opencode-events".into())
                .spawn(move || read_events(&server, idle, &events, &wake))
            {
                eprintln!("opencode-kit: couldn't spawn opencode-events thread: {e}");
            }
        }
        EventHub { base, fan }
    }

    /// The server whose stream this is.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Everything the stream says from now on, with `wake` called once per batch.
    pub fn subscribe(&self, wake: Waker) -> Receiver<Incoming> {
        self.add(wake, None)
    }

    /// Only the stream's events of type `ty` from now on, with `wake` called once per batch that
    /// has one: for a reader that answers them sooner than a window's next frame. Its main
    /// subscription still gets them too.
    pub fn subscribe_to(&self, ty: &'static str, wake: Waker) -> Receiver<Incoming> {
        self.add(wake, Some(ty))
    }

    fn add(&self, wake: Waker, only: Option<&'static str>) -> Receiver<Incoming> {
        let (tx, rx) = channel();
        if let Fan::Open(subs) = &mut *self.fan.lock().unwrap_or_else(|e| e.into_inner()) {
            subs.push(Sub { tx, wake, only });
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
#[cfg(not(target_arch = "wasm32"))]
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
        subs.retain(|sub| sub.send(&batch));
    }
}

/// [`fan_out`] for one message, on the web, where Rune drains the stream's channel.
#[cfg(target_arch = "wasm32")]
fn fan_one(mut m: Incoming, fan: &Mutex<Fan>) -> std::ops::ControlFlow<()> {
    let epoch = *EPOCH.get_or_init(Instant::now);
    if let Incoming::Event { data, .. } = &mut m
        && let Some(map) = data.as_object_mut()
    {
        map.insert(HUB_AT.into(), (epoch.elapsed().as_micros() as u64).into());
    }
    let Fan::Open(subs) = &mut *fan.lock().unwrap_or_else(|e| e.into_inner()) else { return std::ops::ControlFlow::Break(()) };
    subs.retain(|sub| sub.send(std::slice::from_ref(&m)));
    std::ops::ControlFlow::Continue(())
}

/// [`super::connect_with_idle`] for one window of several: its own fetch and write threads,
/// answering into the receiver; the event stream comes from its server's [`EventHub`].
pub fn connect_workers(server: Server, wake: Waker) -> (Sender<Request>, Sender<Write>, Receiver<Incoming>) {
    let (inbox_tx, inbox) = channel();
    let (req_tx, write_tx) = spawn_workers(&server, &inbox_tx, &wake);
    (req_tx, write_tx, inbox)
}
