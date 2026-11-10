//! Every listed server's health, app-wide, as Desktop's health checks run
//! (`runtime/server/health-machine.ts`, `health.ts`): one `/api/info` probe per server when it is
//! listed or its endpoint changes, then every 10 s, and a `retry` that probes all of them now.
//! The machine is MS-1's [`opencode_kit::servers::health`]; this runs its effects: each probe on a
//! worker thread, its answer back through a channel any window's poll drains, the poll's timer
//! on the app's clock. A probe that answers 401 tells the server's controller, which stops its
//! connection until a probe with the same credentials answers again (`machine.ts:203-242`).
//!
//! Home's status marks, its Authenticate button, the cross-server searches and the location
//! dialog read it ([`get`], [`indicator`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::servers::health::{self, Effect, Event, Health, Indicator, Input, ProbeResult};
use opencode_kit::servers::{ServerConn, ServerKey};
use rune::Tx;
use rune::machine::Attempt;

use crate::App;

/// The poll's timer (`TimerStart` / `TimerStop`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Timer {
    #[default]
    Stopped,
    /// The next poll is due at this app clock time.
    Every { due: f64, interval: f64 },
}

struct Answer {
    key: ServerKey,
    attempt: Attempt,
    result: ProbeResult,
}

struct Runtime {
    state: health::State,
    timer: Timer,
    inputs: Vec<Input>,
    /// The connection each probe uses, by server.
    conns: HashMap<ServerKey, ServerConn>,
    /// What each server's `/api/info` last called its machine (`store.hostnames`).
    hostnames: HashMap<ServerKey, String>,
    answers: (Sender<Answer>, Receiver<Answer>),
    waiters: u64,
    version: u64,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime { state: health::State::default(), timer: Timer::Stopped, inputs: Vec::new(), conns: HashMap::new(), hostnames: HashMap::new(), answers: channel(), waiters: 0, version: 0 }
    }
}

thread_local! {
    static HEALTH: RefCell<Runtime> = RefCell::default();
}

/// Server `key`'s health; None while nothing is known.
pub fn get(key: &ServerKey) -> Option<Health> {
    HEALTH.with_borrow(|h| h.state.health(key))
}

/// The status mark a row shows for server `key` (`selectServerIndicator`).
pub fn indicator(key: &ServerKey) -> Indicator {
    health::indicator(get(key).as_ref())
}

/// The hostname server `key` last reported.
pub fn hostname(key: &ServerKey) -> Option<String> {
    HEALTH.with_borrow(|h| h.hostnames.get(key).cloned())
}

/// Changes whenever any server's health does.
pub fn version() -> u64 {
    HEALTH.with_borrow(|h| h.version)
}

/// Probes every server now (`retry`), as Home's Retry does.
pub fn retry(s: &App, tx: &mut Tx) {
    if !s.live() {
        return;
    }
    let fx = HEALTH.with_borrow_mut(|h| {
        h.waiters += 1;
        health::step(&mut h.state, Event::RetryRequested(h.waiters))
    });
    run(s, fx, tx);
}

/// Each poll: the registry's changes reach the machine, answers that came in land, and a due
/// poll probes again. True when a server's health changed.
pub fn poll(s: &App, tx: &mut Tx) -> bool {
    if !s.live() {
        return false;
    }
    let before = version();
    let entries = super::registry::list(s);
    let inputs: Vec<Input> = entries.iter().map(|e| Input::of(&e.conn)).collect();
    let synced = HEALTH.with_borrow_mut(|h| {
        h.conns = entries.into_iter().map(|e| (e.key(), e.conn)).collect();
        if h.inputs == inputs {
            return None;
        }
        h.inputs = inputs.clone();
        Some(health::step(&mut h.state, Event::ServersSynced(inputs)))
    });
    if let Some(fx) = synced {
        run(s, fx, tx);
    }
    let answers: Vec<Answer> = HEALTH.with_borrow(|h| h.answers.1.try_iter().collect());
    for a in answers {
        let fx = HEALTH.with_borrow_mut(|h| health::step(&mut h.state, Event::ProbeSettled { key: a.key, attempt: a.attempt, result: a.result }));
        run(s, fx, tx);
    }
    let due = HEALTH.with_borrow(|h| match h.timer {
        Timer::Every { due, .. } => Some(due),
        Timer::Stopped => None,
    });
    if let Some(due) = due {
        if due <= s.clock {
            let fx = HEALTH.with_borrow_mut(|h| {
                if let Timer::Every { interval, .. } = h.timer {
                    h.timer = Timer::Every { due: s.clock + interval, interval };
                }
                health::step(&mut h.state, Event::PollDue)
            });
            run(s, fx, tx);
        }
        if let Timer::Every { due, .. } = HEALTH.with_borrow(|h| h.timer) {
            tx.wake_at(due);
        }
    }
    version() != before
}

fn run(s: &App, fx: rune::machine::Effects<Effect>, tx: &mut Tx) {
    let mut changed = false;
    for effect in fx {
        match effect {
            Effect::Probe { key, attempt, .. } => probe(s, key, attempt),
            Effect::NotifyProbe(probe) => {
                if let ProbeResult::Healthy { hostname: Some(name), .. } = &probe.result {
                    HEALTH.with_borrow_mut(|h| h.hostnames.insert(probe.key.clone(), name.clone()));
                }
                crate::servers::runtime::reported(&probe);
                changed = true;
            }
            Effect::TimerStart { interval_ms } => {
                let interval = interval_ms as f64 / 1000.0;
                HEALTH.with_borrow_mut(|h| h.timer = Timer::Every { due: s.clock + interval, interval });
                tx.wake_at(s.clock + interval);
            }
            Effect::TimerStop => HEALTH.with_borrow_mut(|h| h.timer = Timer::Stopped),
            Effect::ResolveWaiters(_) => changed = true,
        }
    }
    // Every step that settles a slot changes what rows show.
    HEALTH.with_borrow_mut(|h| h.version += 1);
    if changed {
        tx.instant();
    }
}

/// `checkServerHealth` on a worker: healthy with the version and hostname `/api/info` reports,
/// unauthorized on 401, unreachable otherwise.
fn probe(s: &App, key: ServerKey, attempt: Attempt) {
    let Some(server) = HEALTH.with_borrow(|h| h.conns.get(&key).cloned()).and_then(|c| super::registry::server_for(&c)) else { return };
    let send = HEALTH.with_borrow(|h| h.answers.0.clone());
    let wake = s.waker.clone();
    crate::plugin_host::tasks::run(move || {
        let result = match server.get("/api/info") {
            Ok(v) => {
                let text = |k: &str| v[k].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
                ProbeResult::Healthy { version: text("version"), hostname: text("hostname") }
            }
            Err(e) if e.ends_with("http status: 401") => ProbeResult::Unauthorized,
            Err(_) => ProbeResult::Unreachable,
        };
        let _ = send.send(Answer { key, attempt, result });
        if let Some(w) = wake {
            w();
        }
    });
}
