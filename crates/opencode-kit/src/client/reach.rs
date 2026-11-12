//! One thread for every server that couldn't be reached. Each tick it looks at the connection it
//! opened to each of their addresses (non-blocking, all at once) and opens the next, and only when
//! one that kept refusing accepts again does anything else wake: the event streams parked on it
//! ([`super::worker`]) and the windows that asked ([`Server::watch_reach`]). However many servers
//! stay away, that is one wake of this thread a tick and nothing on a window's thread.
//!
//! Desktop retries a dropped stream every second, with no back-off or cap, and starts over after
//! a connection that lasted a second (`packages/client/src/solid/connection.ts:33,158,181`
//! `reconnectDelay`), and its health check asks each server every 10 s
//! (`runtime/server/health-machine.ts:5`), hidden or not. This tries each address every second,
//! then every two once it has refused for [`SLOW_AFTER`]: a server that comes back shows within
//! two seconds either way. A connect still in flight keeps trying meanwhile (Windows sends the
//! SYN again twice, half a second apart, before it gives up on a refusing port), and the thread
//! sleeps in the OS on those connects ([`POLLER`]): the one that connects wakes it at once, so a
//! server that starts listening is found by the next SYN, within half a second, and no sooner
//! wake is spent on one that keeps refusing. One to a listening loopback port has connected by
//! the time `connect` returns, so it's looked at once.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use polling::{Event, Events, Poller};
use rune::Waker;
use rune::time::Instant;
use socket2::{Domain, Protocol, Socket, Type};

use super::Server;

/// How often an address is tried: Desktop's `reconnectDelay`.
const TICK: Duration = Duration::from_secs(1);
/// An address that has refused this long is tried every other tick.
const SLOW_AFTER: Duration = Duration::from_secs(30);
/// An address nothing asked about for this long is no longer tried (health asks every 10 s).
const EXPIRE: Duration = Duration::from_secs(35);
/// How many comebacks the book keeps for windows that haven't polled yet.
const BACKLOG: usize = 32;

/// One watched address, as the watcher last found it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Reach {
    /// No try has run its whole tick yet.
    #[default]
    Unchecked,
    /// Every try since `since` ran its tick without connecting, the last at `missed`.
    Refusing { since: Instant, missed: Instant },
}

/// What a look at an address found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Check {
    Accepted,
    /// A try ran its whole tick without connecting.
    Missed { at: Instant },
    /// Nothing asked about it for [`EXPIRE`].
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReachEffect {
    /// It accepts again after refusing: wake whoever waits on it, and stop trying it.
    Back,
    /// Stop trying it, waking no one: it accepted without having refused (its trouble is past
    /// the connection, which the stream's own back-off and the health poll handle), or no one asks.
    Forget,
}

impl Reach {
    pub fn step(&mut self, check: Check) -> Option<ReachEffect> {
        match (*self, check) {
            (Reach::Refusing { .. }, Check::Accepted) => Some(ReachEffect::Back),
            (Reach::Unchecked, Check::Accepted) | (_, Check::Expired) => Some(ReachEffect::Forget),
            (Reach::Unchecked, Check::Missed { at }) => {
                *self = Reach::Refusing { since: at, missed: at };
                None
            }
            (Reach::Refusing { since, .. }, Check::Missed { at }) => {
                *self = Reach::Refusing { since, missed: at };
                None
            }
        }
    }

    /// Tried every other tick.
    fn slow(&self, now: Instant) -> bool {
        matches!(self, Reach::Refusing { since, .. } if now.duration_since(*since) >= SLOW_AFTER)
    }
}

/// The try in flight for an address.
enum Try {
    Idle,
    Pending(Armed),
    /// No socket could be made, or its connect failed before the tick ended.
    Failed,
}

/// A connect in flight, watched by [`POLLER`] when there is one, so the thread wakes the moment
/// it connects rather than at the next tick.
struct Armed(Socket);

impl Drop for Armed {
    fn drop(&mut self) {
        if let Some(p) = poller() {
            let _ = p.delete(&self.0);
        }
    }
}

struct Entry {
    server: Server,
    /// Resolved on the watcher's thread, the first time it's tried.
    addr: Option<SocketAddr>,
    reach: Reach,
    attempt: Try,
    asked: Instant,
    wakers: Vec<Waker>,
}

struct Book {
    entries: Vec<Entry>,
    /// Addresses that came back, each with the generation it came back in.
    back: Vec<(u64, String)>,
    generation: u64,
    started: bool,
}

static BOOK: Mutex<Book> = Mutex::new(Book { entries: Vec::new(), back: Vec::new(), generation: 0, started: false });
/// The thread's sleep where `polling` can't be had: ticks only, and a new address rings it.
static BELL: Condvar = Condvar::new();
/// The thread's sleep: until the next tick, a new address (`notify`), or a connect in flight
/// connecting or failing. Windows sends a refused SYN again half a second later, so an address
/// that starts listening is found within half a second of it, not at the next tick.
static POLLER: OnceLock<Option<Poller>> = OnceLock::new();

fn poller() -> Option<&'static Poller> {
    POLLER.get_or_init(|| Poller::new().ok()).as_ref()
}

/// A wake this close to the tick runs the tick: a connect that failed just before it (Windows
/// gives up on a refusing port about a second after the SYN) costs no wake of its own.
const SLACK: Duration = Duration::from_millis(100);

fn book() -> MutexGuard<'static, Book> {
    BOOK.lock().unwrap_or_else(|e| e.into_inner())
}

impl Server {
    /// Tries this server's address each tick until it accepts a connection; `wake` runs (and
    /// every parked event stream retries) when it accepts again after refusing. Each call keeps
    /// it watched for another [`EXPIRE`].
    pub fn watch_reach(&self, wake: Option<Waker>) {
        let mut b = book();
        if let Some(e) = b.entries.iter_mut().find(|e| e.server.base == self.base) {
            e.asked = Instant::now();
            if let Some(w) = wake
                && !e.wakers.iter().any(|x| std::sync::Arc::ptr_eq(x, &w))
            {
                e.wakers.push(w);
            }
            return;
        }
        b.entries.push(Entry { server: self.clone(), addr: None, reach: Reach::Unchecked, attempt: Try::Idle, asked: Instant::now(), wakers: wake.into_iter().collect() });
        // A new address is tried now, not at the next tick.
        if b.started {
            ring();
            return;
        }
        b.started = true;
        let _ = std::thread::Builder::new().name("opencode-reach".into()).spawn(run);
    }

    /// Whether the watcher's last try at this server's address ran its whole tick without
    /// connecting: nothing would answer a request now.
    pub fn refusing(&self) -> bool {
        let now = Instant::now();
        book().entries.iter().any(|e| e.server.base == self.base && matches!(e.reach, Reach::Refusing { missed, .. } if now.duration_since(missed) < TICK * 3))
    }
}

/// The servers (their bases) that came back since generation `seen`, which moves up to now.
pub fn reach_back(seen: &mut u64) -> Vec<String> {
    let b = book();
    let back = b.back.iter().filter(|(g, _)| *g > *seen).map(|(_, base)| base.clone()).collect();
    *seen = b.generation;
    back
}

/// Set by [`ring`], so a wake without events tells a new address from the poller's own returns
/// (it may wake with nothing to say, a cancelled wait of a deleted socket among them).
static RUNG: AtomicBool = AtomicBool::new(false);

fn ring() {
    match poller() {
        Some(p) => {
            RUNG.store(true, Ordering::Release);
            drop(p.notify());
        }
        None => BELL.notify_all(),
    }
}

fn run() {
    let mut b = book();
    let mut tick: u64 = 0;
    loop {
        while b.entries.is_empty() {
            b = sleep(b, None).0;
        }
        let started = Instant::now();
        b = resolve(b);
        let woken = look(&mut b, tick);
        // Every address left tried every other tick: the odd one is skipped.
        let ticks = if !b.entries.is_empty() && b.entries.iter().all(|e| e.reach.slow(started)) && tick % 2 == 0 { 2 } else { 1 };
        tick += ticks;
        drop(b);
        for w in woken {
            w();
        }
        b = book();
        let due = started + TICK * ticks as u32;
        // Between ticks only a connect that connects is acted on; anything else (a new address,
        // the tick itself, a wake within SLACK of it) runs the next tick.
        loop {
            let left = due.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let (next, events) = sleep(b, Some(left));
            b = next;
            if due.saturating_duration_since(Instant::now()) <= SLACK || (!events && (poller().is_none() || RUNG.swap(false, Ordering::AcqRel))) {
                break;
            }
            if !events {
                continue;
            }
            let woken = connected_early(&mut b);
            drop(b);
            for w in woken {
                w();
            }
            b = book();
        }
    }
}

/// Sleeps off the lock for `wait` (forever with `None`), or until [`ring`] or (with a poller) a
/// watched connect changes; whether a connect did.
fn sleep(b: MutexGuard<'static, Book>, wait: Option<Duration>) -> (MutexGuard<'static, Book>, bool) {
    let Some(p) = poller() else {
        let b = match wait {
            Some(w) => BELL.wait_timeout(b, w).unwrap_or_else(|e| e.into_inner()).0,
            None => BELL.wait(b).unwrap_or_else(|e| e.into_inner()),
        };
        return (b, false);
    };
    drop(b);
    let mut events = Events::new();
    let _ = p.wait(&mut events, wait);
    (book(), !events.is_empty())
}

/// Between ticks: each connect in flight that has connected came back, now; one that failed is
/// done until the next tick, which counts it missed and tries again. Who to wake.
fn connected_early(b: &mut Book) -> Vec<Waker> {
    let mut woken = Vec::new();
    let mut came = Vec::new();
    b.entries.retain_mut(|e| {
        let Try::Pending(armed) = &e.attempt else { return true };
        if connected(&armed.0) {
            return keep(e, Check::Accepted, &mut came, &mut woken);
        }
        if matches!(armed.0.take_error(), Ok(Some(_)) | Err(_)) {
            e.attempt = Try::Failed;
        } else if let Some(p) = poller() {
            // Woken for something else: watch it again (a poller's interest lasts one event).
            let _ = p.modify(&armed.0, Event::writable(0));
        }
        true
    });
    came_back(b, came);
    woken
}

/// Resolves addresses not yet known, off the lock (a name may take a while).
fn resolve(b: MutexGuard<'static, Book>) -> MutexGuard<'static, Book> {
    let todo: Vec<Server> = b.entries.iter().filter(|e| e.addr.is_none()).map(|e| e.server.clone()).collect();
    if todo.is_empty() {
        return b;
    }
    drop(b);
    let found: Vec<(String, Option<SocketAddr>)> = todo.into_iter().map(|s| (s.base.clone(), s.addr())).collect();
    let mut b = book();
    for (base, addr) in found {
        if let Some(e) = b.entries.iter_mut().find(|e| e.server.base == base) {
            e.addr = addr;
        }
    }
    b
}

/// Tick `tick`: each address due looks at the try in flight (one that hasn't connected missed),
/// then starts the next and looks at it at once. Returns who to wake for those that came back.
fn look(b: &mut Book, tick: u64) -> Vec<Waker> {
    let now = Instant::now();
    let mut woken = Vec::new();
    let mut came = Vec::new();
    b.entries.retain_mut(|e| {
        if now.duration_since(e.asked) > EXPIRE {
            return keep(e, Check::Expired, &mut came, &mut woken);
        }
        if e.reach.slow(now) && tick % 2 == 1 {
            return true;
        }
        let check = match &e.attempt {
            Try::Pending(armed) if connected(&armed.0) => Some(Check::Accepted),
            Try::Idle if e.addr.is_some() => None,
            _ => Some(Check::Missed { at: now }),
        };
        if let Some(check) = check
            && !keep(e, check, &mut came, &mut woken)
        {
            return false;
        }
        e.attempt = e.addr.map_or(Try::Idle, start);
        match &e.attempt {
            Try::Pending(armed) if connected(&armed.0) => keep(e, Check::Accepted, &mut came, &mut woken),
            _ => true,
        }
    });
    came_back(b, came);
    woken
}

/// Books the addresses that came back and lets the parked event streams retry.
fn came_back(b: &mut Book, came: Vec<String>) {
    if came.is_empty() {
        return;
    }
    b.generation += 1;
    let g = b.generation;
    b.back.extend(came.into_iter().map(|base| (g, base)));
    let over = b.back.len().saturating_sub(BACKLOG);
    b.back.drain(..over);
    super::worker::wake_reconnects();
}

/// Steps `e` by `check`; false when it's no longer watched (its wakers and base, when it came
/// back, go to `woken` and `came`).
fn keep(e: &mut Entry, check: Check, came: &mut Vec<String>, woken: &mut Vec<Waker>) -> bool {
    match e.reach.step(check) {
        Some(ReachEffect::Back) => {
            came.push(e.server.base.clone());
            woken.append(&mut e.wakers);
            false
        }
        Some(ReachEffect::Forget) => false,
        None => true,
    }
}

/// A zero-length send succeeds only on a connected socket. (Windows names the peer of a connect
/// still in flight, so `peer_addr` can't tell.)
fn connected(sock: &Socket) -> bool {
    sock.send(&[]).is_ok()
}

/// A non-blocking connect to `addr`. Whatever `connect` says (in progress, or refused at once on
/// a loopback port), [`connected`] says later whether it connected.
fn start(addr: SocketAddr) -> Try {
    let Ok(sock) = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP)) else { return Try::Failed };
    if sock.set_nonblocking(true).is_err() {
        return Try::Failed;
    }
    let _ = sock.connect(&addr.into());
    if let Some(p) = poller() {
        // SAFETY: `Armed` deletes it from the poller before the socket closes.
        #[allow(unsafe_code)]
        let _ = unsafe { p.add(&sock, Event::writable(0)) };
    }
    Try::Pending(Armed(sock))
}
