//! One run: the app on the fixture's open session, wired to the sim through the link, played
//! op by op on a virtual clock, with the oracle reading the screen after every frame.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::{Incoming, Request, Write};
use opencode_kit::model::{self, Transcript};
use rune::Runtime;

use super::desk::{self, Desk};
use super::link::{Link, Net, Out};
use super::ops::Op;
use super::oracle::{Failure, Kind, Oracle};
use super::sim::{self, Sim};
use super::trace::{Screen, Trace};
use crate::session_pages::{Key as PageKey, Page, Pages};
use crate::{App, Route, Source};

/// Frames this far apart while something just happened, so input and arrivals land as they
/// would at 60 Hz, and at most this far apart otherwise.
const NEAR: f64 = 1.0 / 20.0;
const FAR: f64 = 0.5;
/// How long after an arrival frames stay near.
const AFTER: f64 = 0.15;
/// How long the link stays good, with the server idle, before the end state is judged.
const SETTLE: f64 = 60.0;
const SETTLE_LIMIT: f64 = 900.0;

pub struct World {
    pub rt: Runtime<App>,
    pub sid: String,
    pub sim: Sim,
    pub link: Link,
    pub t: f64,
    pub oracle: Oracle,
    pub trace: Trace,
    server: Sender<Incoming>,
    writes: Receiver<Write>,
    requests: Receiver<Request>,
    pages: Sender<Result<Arc<Page>, (PageKey, String)>>,
    sent: u32,
    others: u32,
    near_until: f64,
}

/// How a run ended.
pub struct Outcome {
    pub failure: Option<Failure>,
    pub trace: String,
    pub log: Vec<String>,
}

/// Plays `ops` from seed `seed`, stopping at the first failure, then lets the session settle
/// and judges where it ended.
pub fn run(seed: u64, ops: &[Op], quiet: bool) -> Outcome {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut w = World::new(seed);
        w.trace.quiet = quiet;
        for op in ops {
            w.op(*op);
            if w.oracle.failed() {
                break;
            }
        }
        if !w.oracle.failed() {
            w.settle();
        }
        let failure = w.oracle.failures.first().cloned();
        if let Some(f) = &failure {
            w.trace.quiet = false;
            w.trace.note(f.t, format!("FAIL {:?}: {}", f.kind, f.detail));
        }
        Outcome { failure, trace: w.trace.text(), log: std::mem::take(&mut w.link.log) }
    }));
    super::set_wall(None);
    result.unwrap_or_else(|panic| {
        let why = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_default();
        Outcome { failure: Some(Failure { kind: Kind::Panicked, t: 0.0, detail: why }), trace: String::new(), log: vec![] }
    })
}

impl World {
    pub fn new(seed: u64) -> World {
        super::set_wall(Some(sim::T0));
        let mut app = App::fixture_session();
        let Route::Session(sid) = app.route.clone() else { panic!("the fixture opens a session") };
        let mut template = app.session(&sid).cloned().unwrap_or_default();
        template.id = sid.clone();
        template.parent = None;
        template.revert = None;
        template.archived = None;
        let sim = Sim::new(seed, template);
        app.running.clear();
        app.inboxes.clear();
        app.stops = Default::default();
        app.playback.clear();
        app.transcripts.insert(sid.clone(), Transcript { session: sid.clone(), messages: model::parse_messages(&sim.page(80)) });
        match app.sessions.iter_mut().find(|s| s.id == sid) {
            Some(s) => *s = sim.info(&sid),
            None => app.sessions.push(sim.info(&sid)),
        }
        let (wtx, writes) = channel();
        let (rtx, requests) = channel();
        let (server, inbox) = channel();
        app.source = Source::Live { server: opencode_kit::client::Server::new("http://127.0.0.1:9"), requests: Some(rtx), writes: Some(wtx), allow_send: true };
        app.inbox = Some(inbox);
        let (pool, pages) = Pages::fixture(None);
        app.pages.set(pool);
        app.unsent.asked = Some(vec![]);
        let mut rt = app.into_runtime();
        rt.frame(0.0);
        let mut w = World { rt, sid, sim, link: Link::new(), t: 0.0, oracle: Oracle::default(), trace: Trace::default(), server, writes, requests, pages, sent: 0, others: 0, near_until: 0.0 };
        desk::tap(&mut w.rt, "prompt");
        w.tick(1.0 / 60.0);
        w
    }

    /// Does `op`, then lets one frame pass.
    pub fn op(&mut self, op: Op) {
        if !desk::play(self, op) {
            self.other(op);
        }
        self.drain();
        self.tick(1.0 / 60.0);
    }

    /// An op that isn't a key: time, the network, the server, another client.
    fn other(&mut self, op: Op) {
        let t = self.t;
        match op {
            Op::Wait(ms) => self.wait(f64::from(ms) / 1000.0),
            Op::Dead | Op::Slow | Op::Fine => {
                let net = match op {
                    Op::Dead => Net::Dead,
                    Op::Slow => Net::Slow,
                    _ => Net::Fine,
                };
                self.trace.note(t, format!("> network {net:?}"));
                self.link.set_net(net, t);
            }
            Op::DropResponse => {
                self.trace.note(t, "> the next write's reply will be lost");
                self.link.drop_next_response = true;
            }
            Op::LoseWrite => {
                self.trace.note(t, "> the next write will be lost on the way");
                self.link.lose_next_write = true;
            }
            Op::Restart(secs) => {
                self.trace.note(t, format!("> server restarts, gone {secs} s"));
                self.sim.restart(f64::from(secs));
            }
            Op::Other => {
                if self.sim.up {
                    self.others += 1;
                    let text = format!("o{}", self.others);
                    self.trace.note(t, format!("> another client sends {text}"));
                    let id = opencode_kit::client::message_id();
                    self.sim.admit(&id, &text, false);
                }
            }
            Op::Helper(secs) => {
                self.trace.note(t, format!("> a background helper starts for {secs} s"));
                self.sim.helper(f64::from(secs));
            }
            _ => unreachable!("{op:?} is a key"),
        }
    }

    /// Lets `secs` pass, frame by frame.
    pub fn wait(&mut self, secs: f64) {
        let end = self.t + secs;
        while self.t < end - 1e-9 {
            let dt = self.next_dt().min(end - self.t);
            self.tick(dt);
            if self.oracle.failed() {
                return;
            }
        }
    }

    fn next_dt(&self) -> f64 {
        let cap = if self.t < self.near_until { NEAR } else { FAR };
        let due = [self.link.next_due(self.t), self.sim.next_due()].into_iter().flatten().reduce(f64::min);
        let to_due = due.map_or(cap, |d| (d - self.t).max(1.0 / 120.0));
        cap.min(to_due).max(1.0 / 120.0)
    }

    /// One frame: the network and server run to the new time, what arrives goes to the app,
    /// the app draws, and what it sent goes out.
    fn tick(&mut self, dt: f64) {
        self.t += dt;
        let t = self.t;
        super::set_wall(Some(sim::T0 + (t * 1000.0) as i64));
        let outs = self.link.advance(t, &mut self.sim);
        if !outs.is_empty() {
            self.near_until = t + AFTER;
        }
        for out in outs {
            self.deliver(out);
        }
        self.rt.frame(dt);
        self.drain();
        let screen = Screen::read(&self.rt, &self.sid);
        self.oracle.check(t, &screen, &self.sim, self.link.health());
        if !self.trace.quiet {
            self.trace.frame(t, &screen, &self.sim, &self.link.describe(&self.sim));
        }
    }

    fn deliver(&mut self, out: Out) {
        match out {
            Out::App(incoming) => {
                let _ = self.server.send(incoming);
            }
            Out::Page { key, cursor, result } => {
                if let Some(pool) = self.rt.state.pages.pool() {
                    pool.finish_job(&key, &cursor, result.as_ref().ok());
                }
                let _ = self.pages.send(result.map_err(|e| (key, e)));
            }
            Out::Stalled(failing) => {
                if let Some(pool) = self.rt.state.pages.pool() {
                    pool.note_stall(failing);
                }
            }
            Out::Receipts { ask, answers } => {
                let fetch = |path: &str| match &answers {
                    Ok(by_path) => by_path.get(path).cloned().unwrap_or_else(|| Err(format!("GET {path}: http status: 404"))),
                    Err(e) => Err(e.clone()),
                };
                self.rt.update(|s, _| s.receipts_from(&fetch, ask));
            }
        }
    }

    /// What the app wrote, asked for or queued to read goes onto the link.
    fn drain(&mut self) {
        let mut any = false;
        while let Ok(w) = self.writes.try_recv() {
            self.link.write(w);
            any = true;
        }
        while let Ok(r) = self.requests.try_recv() {
            self.link.read(r);
            any = true;
        }
        if let Some(ask) = self.rt.state.unsent.asked.as_mut().map(std::mem::take)
            && !ask.is_empty()
        {
            self.link.receipts(ask);
            any = true;
        }
        for foreground in [true, false] {
            if !self.link.lane_free(foreground) {
                continue;
            }
            let job = self.rt.state.pages.pool().and_then(|p| p.take_job(foreground));
            if let Some((key, cursor, limit)) = job {
                self.link.page(key, cursor, limit, foreground, self.t, &self.sim);
                any = true;
            }
        }
        if any {
            self.near_until = self.t + AFTER;
        }
    }

    /// The network comes back for good; once it has stayed good for [`SETTLE`] s with the
    /// server idle and nothing in flight, the end state is judged.
    pub fn settle(&mut self) {
        if self.link.net != Net::Fine {
            self.trace.note(self.t, "> network Fine (settling)");
            self.link.set_net(Net::Fine, self.t);
        }
        let limit = self.t + SETTLE_LIMIT;
        loop {
            let calm = self.sim.idle_now() && self.link.quiet() && self.link.good_since.is_some_and(|g| self.t - g >= SETTLE);
            if calm {
                break;
            }
            if self.t > limit {
                let why = format!("still not settled {SETTLE_LIMIT:.0} s after the last step: server {}", super::trace::server_line(&self.sim));
                self.oracle.never_settled(self.t, why);
                return;
            }
            self.wait(1.0);
            if self.oracle.failed() {
                return;
            }
        }
        let screen = Screen::read(&self.rt, &self.sid);
        self.oracle.settled(self.t, &screen, &self.sim);
    }
}

impl Desk for World {
    fn rt(&mut self) -> &mut Runtime<App> {
        &mut self.rt
    }

    fn now(&self) -> f64 {
        self.t
    }

    fn tick(&mut self, dt: f64) {
        World::tick(self, dt);
    }

    fn flush(&mut self) {
        self.drain();
    }

    fn interrupts(&self) -> usize {
        self.link.interrupts
    }

    fn trace(&mut self) -> &mut Trace {
        &mut self.trace
    }

    fn oracle(&mut self) -> &mut Oracle {
        &mut self.oracle
    }

    fn next_prompt(&mut self) -> String {
        self.sent += 1;
        format!("p{}", self.sent)
    }
}
