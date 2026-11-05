//! The network between the app and the sim, as `opencode_kit::client::connect` and the session
//! page lanes (`session_pages.rs`) see it: one serial write thread (15 s timeout each), one
//! serial read thread, an open-session page lane that asks a busy server again after 2, 4 then
//! 8 s and a prefetch lane that doesn't, and the event stream.
//!
//! A dead link (packets dropped, as on a Wi-Fi handoff) holds what's in flight as TCP does: a
//! link back within a request's 15 s goes on late, one that isn't times out, and whether the
//! server took it depends on whether it got there before the link died. The event stream holds
//! its events the same way until [`EVENTS_IDLE`] of silence, when the client gives up on it,
//! loses what was held, and reconnects with 1–30 s backoff. A server that restarts resets every
//! connection at once and refuses new ones until it's back.

use std::collections::VecDeque;
use std::sync::Arc;

use opencode_kit::client::{Incoming, Request, Write};
use opencode_kit::model::{self, Delivery};
use serde_json::{Value, json};

use super::sim::Sim;
use crate::session_pages::{Key, Page};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Net {
    Fine,
    Slow,
    Dead,
}

/// `client.rs` `EVENTS_IDLE` (25 s on `reconnect-keeps-page`).
pub const EVENTS_IDLE: f64 = 45.0;
const TIMEOUT: f64 = 15.0;

pub enum Job {
    Write(Write),
    Read(Request),
    Page { key: Key, cursor: Option<String>, limit: usize, foreground: bool, attempt: u32 },
    /// The receipt check for prompts that timed out (`send_failed.rs`): their inbox and message
    /// reads, taken as one request on the read lane.
    Receipts(Vec<(String, String)>),
}

/// What reaches the app.
pub enum Out {
    App(Incoming),
    Page { key: Key, cursor: Option<String>, result: Result<Arc<Page>, String> },
    /// The open lane's read is being asked again after an error (true), or got an answer.
    Stalled(bool),
    /// What the server answered each receipt read with, by path; `Err` for all when the
    /// request failed.
    Receipts { ask: Vec<(String, String)>, answers: Result<std::collections::HashMap<String, Result<Value, String>>, String> },
}

struct Flight {
    job: Job,
    start: f64,
    /// When the server takes it, if it will.
    apply_at: Option<f64>,
    applied: bool,
    /// When the reply (or the error) reaches the app.
    reply_at: f64,
    error: Option<String>,
    /// Held by a dead link: back before `reply_at`, it goes on late.
    stalled: bool,
    /// What the server answered when it took it.
    answer: Option<Out>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stream {
    /// Connected; `held_since` when a dead link started holding its bytes.
    Open { held_since: Option<f64> },
    /// Reconnecting at `at`, then waiting `backoff` after a failure.
    Waiting { at: f64, backoff: f64 },
    /// A connect hanging on a dead link since `since`.
    Hanging { since: f64, backoff: f64 },
}

pub struct Link {
    pub net: Net,
    /// Since when the link has been good: fine, the server up, the stream open and whole.
    pub good_since: Option<f64>,
    /// The last time the link was dead with the server up.
    pub last_dead: Option<f64>,
    pub last_down: Option<f64>,
    pub drop_next_response: bool,
    pub lose_next_write: bool,
    writes: (VecDeque<Job>, Option<Flight>),
    reads: (VecDeque<Job>, Option<Flight>),
    page_fg: Option<Flight>,
    page_bg: Option<Flight>,
    /// Open-session pages waiting out a retry: (when, job).
    retries: Vec<(f64, Job)>,
    stream: Stream,
    /// Events a dead link holds.
    held: Vec<(String, Value)>,
    pending: Vec<(f64, u64, Out)>,
    seq: u64,
    /// Interrupts the app has written, for the ↵ intent checks.
    pub interrupts: usize,
    pub log: Vec<String>,
}

impl Link {
    pub fn health(&self) -> super::oracle::Health {
        super::oracle::Health { good_since: self.good_since, last_dead: self.last_dead, last_down: self.last_down }
    }

    pub fn new() -> Self {
        Link {
            net: Net::Fine,
            good_since: Some(0.0),
            last_dead: None,
            last_down: None,
            drop_next_response: false,
            lose_next_write: false,
            writes: (VecDeque::new(), None),
            reads: (VecDeque::new(), None),
            page_fg: None,
            page_bg: None,
            retries: vec![],
            stream: Stream::Open { held_since: None },
            held: vec![],
            pending: vec![],
            seq: 0,
            interrupts: 0,
            log: vec![],
        }
    }

    fn latency(&self) -> f64 {
        if self.net == Net::Slow { 2.0 } else { 0.05 }
    }

    fn event_latency(&self) -> f64 {
        if self.net == Net::Slow { 1.5 } else { 0.03 }
    }

    fn flights(&mut self) -> impl Iterator<Item = &mut Flight> + '_ {
        [&mut self.writes.1, &mut self.reads.1, &mut self.page_fg, &mut self.page_bg].into_iter().flatten()
    }

    pub fn set_net(&mut self, net: Net, t: f64) {
        let was = self.net;
        self.net = net;
        if net == Net::Dead && was != Net::Dead {
            if let Stream::Open { held_since: None } = self.stream {
                self.stream = Stream::Open { held_since: Some(t) };
            }
            for f in self.flights() {
                if f.error.is_none() {
                    if !f.applied {
                        f.apply_at = None;
                    }
                    f.stalled = true;
                    f.error = Some(format!("{}: timeout: global", call(&f.job)));
                    f.reply_at = (f.start + TIMEOUT).max(t);
                }
            }
        } else if net != Net::Dead && was == Net::Dead {
            let lat = self.latency();
            for f in self.flights() {
                if f.stalled && f.reply_at > t {
                    f.stalled = false;
                    f.error = None;
                    if !f.applied {
                        f.apply_at = Some(t + lat / 2.0);
                    }
                    f.reply_at = t + lat;
                }
            }
            if let Stream::Open { held_since: Some(_) } = self.stream {
                self.stream = Stream::Open { held_since: None };
                let lat = self.event_latency();
                for (ty, data) in std::mem::take(&mut self.held) {
                    self.queue_out(t + lat, Out::App(Incoming::Event { ty, data }));
                }
            }
        }
    }

    pub fn write(&mut self, w: Write) {
        if matches!(w, Write::Interrupt(_)) {
            self.interrupts += 1;
        }
        self.writes.0.push_back(Job::Write(w));
    }

    pub fn read(&mut self, r: Request) {
        self.reads.0.push_back(Job::Read(r));
    }

    pub fn receipts(&mut self, ask: Vec<(String, String)>) {
        self.reads.0.push_back(Job::Receipts(ask));
    }

    pub fn lane_free(&self, foreground: bool) -> bool {
        if foreground { self.page_fg.is_none() && self.retries.is_empty() } else { self.page_bg.is_none() }
    }

    pub fn page(&mut self, key: Key, cursor: Option<String>, limit: usize, foreground: bool, t: f64, sim: &Sim) {
        let f = self.start(Job::Page { key, cursor, limit, foreground, attempt: 0 }, t, sim);
        if foreground { self.page_fg = Some(f) } else { self.page_bg = Some(f) }
    }

    /// Whether nothing is on its way either way.
    pub fn quiet(&self) -> bool {
        self.writes.0.is_empty()
            && self.writes.1.is_none()
            && self.reads.0.is_empty()
            && self.reads.1.is_none()
            && self.page_fg.is_none()
            && self.page_bg.is_none()
            && self.retries.is_empty()
            && self.pending.is_empty()
    }

    pub fn stream_open(&self) -> bool {
        matches!(self.stream, Stream::Open { held_since: None })
    }

    /// The next moment something on the link happens, if anything waits.
    pub fn next_due(&self, t: f64) -> Option<f64> {
        let mut due: Vec<f64> = self.pending.iter().map(|p| p.0).collect();
        if !self.writes.0.is_empty() || !self.reads.0.is_empty() {
            due.push(t);
        }
        for f in [&self.writes.1, &self.reads.1, &self.page_fg, &self.page_bg].into_iter().flatten() {
            due.push(f.reply_at);
            if !f.applied {
                due.extend(f.apply_at);
            }
        }
        due.extend(self.retries.iter().map(|r| r.0));
        due.push(match self.stream {
            Stream::Open { held_since: Some(at) } => at + EVENTS_IDLE,
            Stream::Open { held_since: None } => f64::INFINITY,
            Stream::Waiting { at, .. } => at,
            Stream::Hanging { since, .. } => since + EVENTS_IDLE,
        });
        due.into_iter().filter(|d| d.is_finite()).reduce(f64::min)
    }

    pub fn describe(&self, sim: &Sim) -> String {
        let net = if !sim.up {
            "server down"
        } else {
            match self.net {
                Net::Fine => "fine",
                Net::Slow => "slow",
                Net::Dead => "dead",
            }
        };
        let stream = match self.stream {
            Stream::Open { held_since: None } => "",
            Stream::Open { .. } => ", stream held",
            Stream::Waiting { .. } | Stream::Hanging { .. } => ", stream reconnecting",
        };
        format!("{net}{stream}")
    }

    fn start(&mut self, job: Job, t: f64, sim: &Sim) -> Flight {
        let is_write = matches!(job, Job::Write(_));
        let name = call(&job);
        let mut f = Flight { job, start: t, apply_at: None, applied: false, reply_at: t, error: None, stalled: false, answer: None };
        if !sim.up && self.net != Net::Dead {
            f.reply_at = t + 0.01;
            f.error = Some(format!("{name}: io: Connection refused (os error 61)"));
        } else if self.net == Net::Dead {
            f.stalled = true;
            f.reply_at = t + TIMEOUT;
            f.error = Some(format!("{name}: timeout: global"));
        } else if is_write && std::mem::take(&mut self.lose_next_write) {
            self.log.push(format!("{t:.2}s lost on the way: {name}"));
            f.reply_at = t + TIMEOUT;
            f.error = Some(format!("{name}: timeout: global"));
        } else {
            let lat = self.latency();
            f.apply_at = Some(t + lat / 2.0);
            f.reply_at = t + lat;
            if is_write && std::mem::take(&mut self.drop_next_response) {
                self.log.push(format!("{t:.2}s reply will be lost: {name}"));
                f.reply_at = t + TIMEOUT;
                f.error = Some(format!("{name}: timeout: global"));
            }
        }
        f
    }

    /// Runs the link and the server to `t`: starts what's queued, applies what reached the
    /// server, and returns what reaches the app by then, in order.
    pub fn advance(&mut self, t: f64, sim: &mut Sim) -> Vec<Out> {
        loop {
            let mut progressed = false;
            for lane in 0..4 {
                progressed |= self.step_lane(lane, t, sim);
            }
            let due: Vec<usize> = self.retries.iter().enumerate().filter(|(_, (at, _))| *at <= t).map(|(k, _)| k).collect();
            for k in due.into_iter().rev() {
                let (at, job) = self.retries.remove(k);
                self.page_fg = Some(self.start(job, at, sim));
                progressed = true;
            }
            self.pump(sim);
            if !progressed {
                break;
            }
        }
        sim.advance(t.max(sim.t));
        self.pump(sim);
        self.observe(t, sim);
        self.stream_step(t, sim);
        let good = self.net == Net::Fine && sim.up && self.stream_open();
        match (good, self.good_since) {
            (true, None) => self.good_since = Some(t),
            (false, _) => self.good_since = None,
            _ => {}
        }
        if self.net == Net::Dead && sim.up {
            self.last_dead = Some(t);
        }
        if !sim.up {
            self.last_down = Some(t);
        }
        self.pending.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let n = self.pending.iter().take_while(|(at, ..)| *at <= t).count();
        self.pending.drain(..n).map(|(.., o)| o).collect()
    }

    fn queue_out(&mut self, at: f64, out: Out) {
        self.seq += 1;
        self.pending.push((at, self.seq, out));
    }

    /// Hands the server's new events to the stream: on their way while it's open, held while
    /// the link is dead, lost while it's reconnecting.
    fn pump(&mut self, sim: &mut Sim) {
        let lat = self.event_latency();
        for (at, ty, data) in std::mem::take(&mut sim.out) {
            match self.stream {
                Stream::Open { held_since: None } => self.queue_out(at + lat, Out::App(Incoming::Event { ty, data })),
                Stream::Open { held_since: Some(_) } => self.held.push((ty, data)),
                _ => self.log.push(format!("{at:.2}s lost {ty} (stream down)")),
            }
        }
    }

    /// The server went away: unless the link is dead too (and so can't tell), its stream ends
    /// and every request on its way fails at once.
    fn observe(&mut self, t: f64, sim: &Sim) {
        if sim.up {
            return;
        }
        self.held.clear();
        if self.net == Net::Dead {
            return;
        }
        if let Stream::Open { .. } = self.stream {
            self.queue_out(t, Out::App(Incoming::Status("events: stream ended".into())));
            self.stream = Stream::Waiting { at: t + 1.0, backoff: 2.0 };
        }
        for f in self.flights() {
            if f.error.is_none() || f.stalled {
                if !f.applied {
                    f.apply_at = None;
                }
                f.stalled = false;
                f.answer = None;
                f.error = Some(format!("{}: io: Connection reset by peer (os error 54)", call(&f.job)));
                f.reply_at = t + 0.01;
            }
        }
    }

    fn stream_step(&mut self, t: f64, sim: &Sim) {
        match self.stream {
            Stream::Open { held_since: Some(at) } if t >= at + EVENTS_IDLE => {
                self.log.push(format!("{t:.2}s stream gave up, {} held events lost", self.held.len()));
                self.held.clear();
                self.queue_out(t, Out::App(Incoming::Status("events: timeout: per read".into())));
                self.stream = Stream::Waiting { at: t + 1.0, backoff: 2.0 };
            }
            Stream::Waiting { at, backoff } if t >= at => {
                if self.net == Net::Dead {
                    self.stream = Stream::Hanging { since: t, backoff };
                } else if !sim.up {
                    self.queue_out(t, Out::App(Incoming::Status("events: io: Connection refused (os error 61)".into())));
                    self.stream = Stream::Waiting { at: t + backoff, backoff: (backoff * 2.0).min(30.0) };
                } else {
                    self.open(t);
                }
            }
            Stream::Hanging { since, backoff } => {
                if self.net != Net::Dead && sim.up {
                    self.open(t);
                } else if t >= since + EVENTS_IDLE {
                    self.queue_out(t, Out::App(Incoming::Status("events: io: Operation timed out (os error 60)".into())));
                    self.stream = Stream::Waiting { at: t + backoff, backoff: (backoff * 2.0).min(30.0) };
                }
            }
            _ => {}
        }
    }

    fn open(&mut self, t: f64) {
        let lat = self.event_latency();
        self.queue_out(t + lat, Out::App(Incoming::Reconnected));
        self.stream = Stream::Open { held_since: None };
    }

    /// Moves lane `lane` along: starts its next job, applies a job that reached the server,
    /// and finishes one whose reply is due. True when something changed.
    fn step_lane(&mut self, lane: usize, t: f64, sim: &mut Sim) -> bool {
        let mut flight = match lane {
            0 => self.writes.1.take(),
            1 => self.reads.1.take(),
            2 => self.page_fg.take(),
            _ => self.page_bg.take(),
        };
        let mut changed = false;
        if flight.is_none() && lane < 2 {
            let next = if lane == 0 { self.writes.0.pop_front() } else { self.reads.0.pop_front() };
            if let Some(job) = next {
                flight = Some(self.start(job, t, sim));
                changed = true;
            }
        }
        if let Some(f) = &mut flight
            && !f.applied
            && let Some(at) = f.apply_at.filter(|at| *at <= t)
        {
            f.applied = true;
            sim.advance(at.max(sim.t));
            f.answer = apply(&f.job, sim, &mut self.log);
            changed = true;
        }
        if let Some(f) = flight.take_if(|f| f.reply_at <= t) {
            self.finish(f);
            changed = true;
        }
        match lane {
            0 => self.writes.1 = flight,
            1 => self.reads.1 = flight,
            2 => self.page_fg = flight,
            _ => self.page_bg = flight,
        }
        changed
    }

    fn finish(&mut self, f: Flight) {
        let at = f.reply_at;
        let Some(e) = f.error else {
            if matches!(f.job, Job::Page { foreground: true, .. }) {
                self.queue_out(at, Out::Stalled(false));
            }
            if let Some(out) = f.answer {
                self.queue_out(at, out);
            }
            return;
        };
        match f.job {
            Job::Write(w) => match w {
                Write::Send { session, prompt } | Write::SendMovingHome { session, prompt } => {
                    self.queue_out(at, Out::App(Incoming::SendFailed { session, id: prompt.id.unwrap_or_default(), error: e }));
                }
                Write::Background { quiet: true, .. } => {}
                _ => self.queue_out(at, Out::App(Incoming::Status(e))),
            },
            // As `client.rs`: a failed pending or config read answers empty; others report.
            Job::Read(r) => {
                let out = match r {
                    Request::Pending(session) => Incoming::Pending { session, items: vec![] },
                    Request::Config(dir) => Incoming::Event { ty: "config.listed".into(), data: json!({ "directory": dir, "backgroundCompaction": false }) },
                    _ => Incoming::Status(e),
                };
                self.queue_out(at, Out::App(out));
            }
            Job::Receipts(ask) => self.queue_out(at, Out::Receipts { ask, answers: Err(e) }),
            Job::Page { key, cursor, limit, foreground, attempt } => {
                if foreground && opencode_kit::plain_error::is_transient(&e) {
                    let wait = 2.0 * f64::from(1u32 << attempt.min(2));
                    self.log.push(format!("{at:.2}s page read failed, again in {wait}s: {e}"));
                    self.queue_out(at, Out::Stalled(true));
                    self.retries.push((at + wait, Job::Page { key, cursor, limit, foreground, attempt: attempt + 1 }));
                } else {
                    self.queue_out(at, Out::Page { key, cursor, result: Err(e) });
                }
            }
        }
    }
}

/// `METHOD /path`, as the client's errors start.
fn call(job: &Job) -> String {
    match job {
        Job::Write(w) => match w {
            Write::Send { session, .. } | Write::SendMovingHome { session, .. } | Write::Prompt { session, .. } => format!("POST /api/session/{session}/prompt"),
            Write::Interrupt(session) => format!("POST /api/session/{session}/interrupt"),
            Write::Background { session, .. } => format!("POST /api/session/{session}/background"),
            Write::Cancel { session, item } => format!("DELETE /api/session/{session}/inbox/{item}"),
            Write::Deliver { session, item, .. } => format!("PATCH /api/session/{session}/inbox/{item}"),
            Write::View { session, .. } => format!("POST /api/session/{session}/view"),
            _ => "POST /api/other".into(),
        },
        Job::Read(r) => match r {
            Request::Session(id) => format!("GET /api/session/{id}/message?limit=80&order=desc"),
            Request::Sessions => "GET /api/session?limit=120".into(),
            Request::ProjectSessions(p) => format!("GET /api/session?project={p}"),
            Request::Projects => "GET /api/project".into(),
            Request::Info(id) => format!("GET /api/session/{id}"),
            Request::Pending(id) => format!("GET /api/session/{id}/pending"),
            Request::Inbox(id) => format!("GET /api/session/{id}/inbox"),
            Request::Active => "GET /api/session/active".into(),
            Request::Config(_) => "GET /api/config".into(),
        },
        Job::Receipts(ask) => format!("GET /api/session/{}/inbox", ask.first().map_or("", |a| a.0.as_str())),
        Job::Page { key, cursor: None, limit, .. } => format!("GET /api/session/{}/message?limit={limit}&order=desc", key.id),
        Job::Page { key, cursor: Some(c), limit, .. } => format!("GET /api/session/{}/message?limit={limit}&cursor={c}", key.id),
    }
}

/// The server takes `job`; returns what it answers.
fn apply(job: &Job, sim: &mut Sim, log: &mut Vec<String>) -> Option<Out> {
    let t = sim.t;
    match job {
        Job::Write(w) => match w {
            Write::Send { session, prompt } | Write::SendMovingHome { session, prompt } if *session == sim.sid => {
                let id = prompt.id.clone().unwrap_or_default();
                log.push(format!("{t:.2}s server took {}", prompt.text));
                sim.admit(&id, &prompt.text, prompt.delivery == Some(Delivery::Queue));
                None
            }
            Write::Interrupt(session) => {
                let interrupted = *session == sim.sid && sim.interrupt();
                log.push(format!("{t:.2}s server took the interrupt (interrupted: {interrupted})"));
                Some(Out::App(Incoming::Event { ty: "session.interrupt.acknowledged".into(), data: json!({ "sessionID": session, "interrupted": interrupted }) }))
            }
            Write::Cancel { item, .. } => {
                sim.cancel(item);
                None
            }
            Write::Deliver { item, delivery, .. } => {
                sim.set_delivery(item, *delivery == Delivery::Queue);
                None
            }
            _ => None,
        },
        Job::Read(r) => Some(Out::App(match r {
            Request::Session(id) if *id == sim.sid => Incoming::Messages { session: id.clone(), messages: model::parse_messages(&sim.page(80)) },
            Request::Session(id) => Incoming::Messages { session: id.clone(), messages: vec![] },
            Request::Sessions | Request::ProjectSessions(_) => Incoming::Sessions(sim.sessions()),
            Request::Projects => return None,
            Request::Info(id) => Incoming::Info(sim.info(id)),
            Request::Pending(id) => Incoming::Pending { session: id.clone(), items: vec![] },
            Request::Inbox(id) => {
                let items = if *id == sim.sid { sim.inbox_json() } else { json!([]) };
                Incoming::Event { ty: "session.inbox.listed".into(), data: json!({ "sessionID": id, "items": items }) }
            }
            Request::Active => Incoming::Active(sim.active()),
            Request::Config(dir) => Incoming::Event { ty: "config.listed".into(), data: json!({ "directory": dir, "backgroundCompaction": false }) },
        })),
        Job::Receipts(ask) => {
            let mut answers = std::collections::HashMap::new();
            for (session, id) in ask {
                let ours = *session == sim.sid;
                answers.insert(format!("/api/session/{session}/inbox"), Ok(json!({ "data": if ours { sim.inbox_json() } else { json!([]) } })));
                let path = format!("/api/session/{session}/message/{id}");
                let found = if ours { sim.message(id) } else { None };
                answers.insert(path.clone(), found.ok_or_else(|| format!("GET {path}: http status: 404")));
            }
            Some(Out::Receipts { ask: ask.clone(), answers: Ok(answers) })
        }
        // The latest `limit` messages, with no cursor: older pages are never read, which keeps
        // a run free of the wall-clock history timer.
        Job::Page { key, cursor, limit, .. } => {
            let value = if key.id == sim.sid { sim.page(*limit) } else { json!({ "data": [] }) };
            let page = Page {
                key: key.clone(),
                messages: model::parse_messages(&value),
                cursor: None,
                cached: false,
                history: cursor.is_some(),
                images: vec![],
                bytes: 0,
                read: Default::default(),
            };
            Some(Out::Page { key: key.clone(), cursor: cursor.clone(), result: Ok(Arc::new(page)) })
        }
    }
}
