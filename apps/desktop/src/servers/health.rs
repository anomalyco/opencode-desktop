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
use std::collections::{HashMap, HashSet};
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

enum Answer {
    Probe { key: ServerKey, attempt: Attempt, result: ProbeResult },
    /// Its home folder, asked for once it answers (`sync.data.path.home`), after the probe's
    /// answer: a server that just started takes seconds over it.
    Home { key: ServerKey, home: String },
}

struct Runtime {
    state: health::State,
    timer: Timer,
    inputs: Vec<Input>,
    /// The connection each probe uses, by server.
    conns: HashMap<ServerKey, ServerConn>,
    /// What each server's `/api/info` last called its machine (`store.hostnames`).
    hostnames: HashMap<ServerKey, String>,
    /// Each server's home folder, inferred from where it runs (`loadPathQuery`), for `~`.
    homes: HashMap<ServerKey, String>,
    answers: (Sender<Answer>, Receiver<Answer>),
    waiters: u64,
    /// `retry` callers whose every probe answered, until they ask ([`resolved`]).
    resolved: HashSet<u64>,
    version: u64,
    /// The reach watcher's comebacks taken so far (`opencode_kit::client::reach_back`).
    reach_seen: u64,
    /// Servers that accept connections again, whose next probe waits out their start ([`PATIENCE`]).
    patient: HashSet<ServerKey>,
}

/// How long a probe of a server that accepts connections again keeps asking while it doesn't
/// answer: a server accepts a while before it answers (1.1-5.4 s for the harness's), and past this
/// the 10 s poll asks anyway.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

impl Default for Runtime {
    fn default() -> Self {
        Runtime { state: health::State::default(), timer: Timer::Stopped, inputs: Vec::new(), conns: HashMap::new(), hostnames: HashMap::new(), homes: HashMap::new(), answers: channel(), waiters: 0, resolved: HashSet::new(), version: 0, reach_seen: 0, patient: HashSet::new() }
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

/// Server `key`'s home folder, as Desktop infers it from where the server runs
/// (`inferHomeDirectory(location.directory) || inferHomeDirectory(location.project.directory)`);
/// None until it has answered, or when it runs outside a home.
pub fn home(key: &ServerKey) -> Option<String> {
    HEALTH.with_borrow(|h| h.homes.get(key).cloned()).filter(|h| !h.is_empty())
}

/// Changes whenever any server's health does.
pub fn version() -> u64 {
    HEALTH.with_borrow(|h| h.version)
}

/// Probes every server now (`retry`), as Home's and Settings' Retry do; [`resolved`] says when
/// each answered. None without a server.
pub fn retry(s: &App, tx: &mut Tx) -> Option<u64> {
    if !s.live() {
        return None;
    }
    let (id, fx) = HEALTH.with_borrow_mut(|h| {
        h.waiters += 1;
        (h.waiters, health::step(&mut h.state, Event::RetryRequested(h.waiters)))
    });
    run(s, fx, tx);
    Some(id)
}

/// Probes server `key` now: its event stream dropped or came back, so Home's mark and the
/// server's tabs follow at once rather than at the next 10 s poll.
pub fn nudge(s: &App, key: &ServerKey, tx: &mut Tx) {
    if !s.live() {
        return;
    }
    let fx = HEALTH.with_borrow_mut(|h| health::step(&mut h.state, Event::Nudged(key.clone())));
    run(s, fx, tx);
}

/// Whether `retry` caller `id`'s probes have all answered (it's forgotten once asked).
pub fn resolved(id: u64) -> bool {
    HEALTH.with_borrow_mut(|h| h.resolved.remove(&id))
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
    // A server that refused connections accepts again: probe it now, not at the next poll.
    let back = HEALTH.with_borrow_mut(|h| opencode_kit::client::reach_back(&mut h.reach_seen));
    if !back.is_empty() {
        let keys: Vec<ServerKey> = HEALTH.with_borrow(|h| h.conns.iter().filter(|(_, c)| super::registry::server_for(c).is_some_and(|sv| back.contains(&sv.base))).map(|(k, _)| k.clone()).collect());
        for key in keys {
            HEALTH.with_borrow_mut(|h| h.patient.insert(key.clone()));
            nudge(s, &key, tx);
            // A server the nudge didn't probe (no endpoint) leaves nothing for a later one.
            HEALTH.with_borrow_mut(|h| h.patient.remove(&key));
        }
    }
    let answers: Vec<Answer> = HEALTH.with_borrow(|h| h.answers.1.try_iter().collect());
    for a in answers {
        let (key, attempt, result) = match a {
            Answer::Home { key, home } => {
                HEALTH.with_borrow_mut(|h| {
                    h.homes.insert(key, home);
                    h.version += 1;
                });
                continue;
            }
            Answer::Probe { key, attempt, result } => (key, attempt, result),
        };
        let fx = HEALTH.with_borrow_mut(|h| health::step(&mut h.state, Event::ProbeSettled { key, attempt, result }));
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
    // Only what rows show moves the version and asks for a frame: an answer that says what the
    // last one did (an unreachable server's every 10 s) changes nothing on screen.
    let before = HEALTH.with_borrow(|h| shown(h));
    let mut changed = false;
    let mut known = Vec::new();
    for effect in fx {
        match effect {
            Effect::Probe { key, attempt, .. } => {
                if let Some(result) = probe(s, &key, attempt) {
                    known.push((key, attempt, result));
                }
            }
            Effect::NotifyProbe(probe) => {
                if let ProbeResult::Healthy { hostname: Some(name), .. } = &probe.result {
                    HEALTH.with_borrow_mut(|h| h.hostnames.insert(probe.key.clone(), name.clone()));
                }
                crate::servers::runtime::reported(&probe);
            }
            Effect::TimerStart { interval_ms } => {
                let interval = interval_ms as f64 / 1000.0;
                HEALTH.with_borrow_mut(|h| h.timer = Timer::Every { due: s.clock + interval, interval });
                tx.wake_at(s.clock + interval);
            }
            Effect::TimerStop => HEALTH.with_borrow_mut(|h| h.timer = Timer::Stopped),
            Effect::ResolveWaiters(ids) => {
                HEALTH.with_borrow_mut(|h| h.resolved.extend(ids));
                changed = true;
            }
        }
    }
    changed |= HEALTH.with_borrow(|h| shown(h)) != before;
    if changed {
        HEALTH.with_borrow_mut(|h| h.version += 1);
        tx.instant();
    }
    for (key, attempt, result) in known {
        let fx = HEALTH.with_borrow_mut(|h| health::step(&mut h.state, Event::ProbeSettled { key, attempt, result }));
        run(s, fx, tx);
    }
}

/// What rows show of every server's health.
fn shown(h: &Runtime) -> Vec<(Health, Option<String>)> {
    h.state.entries.iter().map(|(k, slot)| (slot.health(), h.hostnames.get(k).cloned())).collect()
}

/// `checkServerHealth` on a worker: healthy with the version and hostname `/api/info` reports,
/// unauthorized on 401, unreachable otherwise. A server that answers for the first time is asked
/// where it runs too (`/api/location`, `loadPathQuery`), for its home.
///
/// A server whose address refused every connection the reach watcher tried in the last second
/// answers Unreachable at once, with no request (the watcher sees it come back sooner than this
/// poll would): the answer, for the caller to settle.
fn probe(s: &App, key: &ServerKey, attempt: Attempt) -> Option<ProbeResult> {
    let server = HEALTH.with_borrow(|h| h.conns.get(key).cloned()).and_then(|c| super::registry::server_for(&c))?;
    // The windows' quiet poll: answers landing together wake the window once, and draw only if
    // something changed.
    let wake = s.launch.inbox_wake.clone().or_else(|| s.waker.clone());
    let patient = HEALTH.with_borrow_mut(|h| h.patient.remove(key));
    if !patient && server.refusing() {
        server.watch_reach(wake);
        return Some(ProbeResult::Unreachable);
    }
    let ask_home = HEALTH.with_borrow(|h| !h.homes.contains_key(key));
    let send = HEALTH.with_borrow(|h| h.answers.0.clone());
    let key = key.clone();
    crate::plugin_host::tasks::run(move || {
        let started = rune::time::Instant::now();
        let result = loop {
            let result = match server.get("/api/info") {
                Ok(v) => {
                    let text = |k: &str| v[k].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
                    ProbeResult::Healthy { version: text("version"), hostname: text("hostname") }
                }
                Err(e) if e.ends_with("http status: 401") => ProbeResult::Unauthorized,
                Err(_) => ProbeResult::Unreachable,
            };
            if !patient || result != ProbeResult::Unreachable || started.elapsed() >= PATIENCE {
                break result;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        };
        if result == ProbeResult::Unreachable {
            server.watch_reach(wake.clone());
        }
        let healthy = matches!(result, ProbeResult::Healthy { .. });
        let _ = send.send(Answer::Probe { key: key.clone(), attempt, result });
        if let Some(w) = &wake {
            w();
        }
        let Some(v) = (ask_home && healthy).then(|| server.get("/api/location").ok()).flatten() else { return };
        let dir = |v: &serde_json::Value| v["directory"].as_str().unwrap_or_default().to_owned();
        let home = Some(opencode_kit::path::infer_home(&dir(&v))).filter(|h| !h.is_empty()).unwrap_or_else(|| opencode_kit::path::infer_home(&dir(&v["project"])));
        let _ = send.send(Answer::Home { key, home });
        if let Some(w) = &wake {
            w();
        }
    });
    None
}
