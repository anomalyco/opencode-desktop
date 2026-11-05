//! A shrunk failure played once against a real OpenCode server, so every bug the sim finds is
//! confirmed on the real thing. The app runs as in `launch.rs` (`client::connect` and live page
//! lanes) but through [`Proxy`], which plays the network; what the server has is read straight
//! from it, past the proxy, four times a second. Time is real.
//!
//! The server is an isolated Drive instance on a worker (`scripts/interaction-fuzz/serve.ts`
//! writes `{url, password, directory}`), never Kit's. `Restart` is an outage at the proxy: the app
//! sees refused connections, the server keeps its state. `Helper` needs a subagent and is
//! skipped.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opencode_kit::client::{Server, Write};
use opencode_kit::model::{self, Delivery, Message, Transcript};
use rune::Runtime;
use serde_json::Value;

use super::desk::{self, Desk};
use super::ops::Op;
use super::oracle::{Health, Oracle, Truth};
use super::proxy::{Mode, Proxy};
use super::trace::{Screen, Trace};
use super::world::Outcome;
use crate::session_pages::Pages;
use crate::{App, Route, Source};

const FRAME: Duration = Duration::from_millis(33);
const SETTLE: f64 = 60.0;
const SETTLE_LIMIT: f64 = 300.0;

/// Where the real server is: `{"url": "http://127.0.0.1:PORT", "password": "…", "directory": "…",
/// "model": {"providerID", "id"}}`.
/// Each replay starts a session of its own in `directory`.
#[derive(Clone, Debug)]
pub struct Config {
    pub url: String,
    pub password: Option<String>,
    pub directory: String,
    pub model: Option<model::ModelRef>,
}

impl Config {
    pub fn read(path: &str) -> Config {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        let url = v["url"].as_str().expect("url").trim_end_matches('/').to_owned();
        assert!(!url.ends_with(":4096"), "never Kit's server: {url}");
        let model = v["model"]["id"].as_str().map(|id| model::ModelRef { id: id.into(), provider: v["model"]["providerID"].as_str().unwrap_or_default().into(), variant: String::new() });
        Config { url, password: v["password"].as_str().map(str::to_owned), directory: v["directory"].as_str().expect("directory").to_owned(), model }
    }

    fn server(&self, base: &str) -> Server {
        let s = Server::new(base);
        match &self.password {
            Some(p) => s.with_password(p),
            None => s,
        }
    }
}

/// What the server had at the last read.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    users: Vec<String>,
    inbox: Vec<(String, String, bool)>,
    active: Vec<String>,
    sid: String,
    up: bool,
    words: std::collections::HashMap<String, String>,
}

impl Truth for Snapshot {
    fn users(&self) -> Vec<String> {
        self.users.clone()
    }

    fn inbox(&self) -> Vec<(String, String, bool)> {
        self.inbox.clone()
    }

    fn busy(&self) -> bool {
        self.active.iter().any(|a| *a == self.sid)
    }

    fn up(&self) -> bool {
        self.up
    }

    fn helpers(&self) -> usize {
        self.active.iter().filter(|a| **a != self.sid).count()
    }

    fn words(&self) -> std::collections::HashMap<String, String> {
        self.words.clone()
    }
}

fn read_truth(server: &Server, sid: &str) -> Snapshot {
    let mut snap = Snapshot { sid: sid.into(), ..Default::default() };
    let messages = server.get(&format!("/api/session/{sid}/message?limit=200&order=asc"));
    let inbox = server.get(&format!("/api/session/{sid}/inbox"));
    let active = server.get("/api/session/active");
    let (Ok(messages), Ok(inbox), Ok(active)) = (messages, inbox, active) else { return snap };
    snap.up = true;
    for m in model::parse_messages(&messages) {
        if let Message::User { id, text } = m {
            snap.words.insert(id.clone(), text);
            snap.users.push(id);
        }
    }
    let items = inbox["data"].as_array().or_else(|| inbox.as_array()).cloned().unwrap_or_default();
    for item in items.iter().filter_map(model::parse_inbox_item) {
        snap.words.insert(item.id.clone(), item.text.clone());
        snap.inbox.push((item.id, item.text, item.delivery == Delivery::Queue));
    }
    let map = active.get("data").unwrap_or(&active);
    snap.active = map.as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect();
    snap
}

pub struct RealWorld {
    rt: Runtime<App>,
    sid: String,
    proxy: Proxy,
    direct: Server,
    truth: Arc<Mutex<Snapshot>>,
    writes: Receiver<Write>,
    wire: Sender<Write>,
    interrupts: usize,
    start: Instant,
    last: Instant,
    t: f64,
    health: Health,
    /// A proxy outage standing in for a restart: until when, and the network after.
    outage: Option<(f64, Mode)>,
    oracle: Oracle,
    trace: Trace,
    sent: u32,
    others: u32,
}

/// Plays `ops` against the real server in `cfg`, then lets it settle and judges the end.
pub fn run(cfg: &Config, ops: &[Op]) -> Outcome {
    let mut w = RealWorld::new(cfg);
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
        w.trace.note(f.t, format!("FAIL {:?}: {}", f.kind, f.detail));
    }
    Outcome { failure, trace: w.trace.text(), log: w.proxy.take_log() }
}

impl RealWorld {
    pub fn new(cfg: &Config) -> RealWorld {
        let direct = cfg.server(&cfg.url);
        let upstream = cfg.url.split("://").nth(1).and_then(|h| h.split('/').next()).and_then(|h| std::net::ToSocketAddrs::to_socket_addrs(&h.replace("localhost", "127.0.0.1")).ok()?.next()).expect("the server's address");
        let proxy = Proxy::start(upstream);
        let via = cfg.server(&format!("http://127.0.0.1:{}", proxy.port));
        let sid = direct.create_session_as(&opencode_kit::client::session_id(), &cfg.directory, Some("build"), cfg.model.as_ref()).unwrap_or_else(|e| panic!("start a session: {e}")).id;

        let mut app = App::fixture_session();
        app.running.clear();
        app.inboxes.clear();
        app.stops = Default::default();
        app.playback.clear();
        app.transcripts.clear();
        let info = direct.get(&format!("/api/session/{sid}")).unwrap_or_else(|e| panic!("the real session {sid}: {e}"));
        app.sessions.retain(|s| s.id != sid);
        app.sessions.push(model::parse_session(info.get("data").unwrap_or(&info)));
        if let Ok(page) = direct.read(&format!("/api/session/{sid}/message?limit={}&order=desc", crate::session_pages::LATEST)) {
            app.transcripts.insert(sid.clone(), Transcript { session: sid.clone(), messages: model::parse_messages(&page) });
        }
        app.route = Route::Session(sid.clone());
        let waker: rune::Waker = Arc::new(|| {});
        app.waker = Some(waker.clone());
        app.pages.set(Pages::live(via.clone(), waker.clone()));
        let (requests, wire, inbox) = opencode_kit::client::connect(via.clone(), waker);
        let (wtx, writes) = channel();
        app.source = Source::Live { server: via, requests: Some(requests), writes: Some(wtx), allow_send: true };
        app.inbox = Some(inbox);
        let mut rt = app.into_runtime();
        let open = sid.clone();
        rt.update(move |s, tx| s.open(&open, tx));
        rt.frame(0.0);

        let truth = Arc::new(Mutex::new(read_truth(&direct, &sid)));
        {
            let (truth, server, sid) = (truth.clone(), direct.clone(), sid.clone());
            std::thread::spawn(move || {
                // Ends with the test process.
                loop {
                    let snap = read_truth(&server, &sid);
                    *truth.lock().unwrap() = snap;
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
        }
        let now = Instant::now();
        let mut w = RealWorld {
            rt,
            sid,
            proxy,
            direct,
            truth,
            writes,
            wire,
            interrupts: 0,
            start: now,
            last: now,
            t: 0.0,
            health: Health { good_since: Some(0.0), last_dead: None, last_down: None },
            outage: None,
            oracle: Oracle::default(),
            trace: Trace::default(),
            sent: 0,
            others: 0,
        };
        desk::tap(&mut w.rt, "prompt");
        w.wait(2.0);
        w
    }

    pub fn op(&mut self, op: Op) {
        if !desk::play(self, op) {
            self.other(op);
        }
        self.flush();
        Desk::tick(self, 0.0);
    }

    fn other(&mut self, op: Op) {
        let t = self.t;
        match op {
            Op::Wait(ms) => self.wait(f64::from(ms) / 1000.0),
            Op::Dead | Op::Slow | Op::Fine => {
                let mode = match op {
                    Op::Dead => Mode::Dead,
                    Op::Slow => Mode::Slow,
                    _ => Mode::Fine,
                };
                self.trace.note(t, format!("> network {mode:?}"));
                match &mut self.outage {
                    Some((_, after)) => *after = mode,
                    None => self.proxy.set(mode),
                }
            }
            Op::DropResponse => {
                self.trace.note(t, "> the next write's reply will be lost");
                self.proxy.drop_next_response();
            }
            Op::LoseWrite => {
                self.trace.note(t, "> the next write will be lost on the way");
                self.proxy.lose_next_write();
            }
            Op::Restart(secs) => {
                self.trace.note(t, format!("> server unreachable {secs} s (a proxy outage; the server keeps running)"));
                let after = self.outage.map_or(self.proxy.mode(), |o| o.1);
                self.proxy.set(Mode::Down);
                self.outage = Some((t + f64::from(secs), after));
            }
            Op::Other => {
                self.others += 1;
                let text = format!("o{}", self.others);
                self.trace.note(t, format!("> another client sends {text}"));
                if let Err(e) = self.direct.send_prompt(&self.sid, &text) {
                    self.trace.note(t, format!("  (it failed: {e})"));
                }
            }
            Op::Helper(secs) => self.trace.note(t, format!("> (skipped: a {secs} s helper needs a subagent)")),
            _ => unreachable!("{op:?} is a key"),
        }
    }

    pub fn wait(&mut self, secs: f64) {
        let end = self.t + secs;
        while self.t < end && !self.oracle.failed() {
            std::thread::sleep(FRAME);
            Desk::tick(self, 0.0);
        }
    }

    pub fn settle(&mut self) {
        if let Some((_, after)) = &mut self.outage {
            *after = Mode::Fine;
        } else if self.proxy.mode() != Mode::Fine {
            self.trace.note(self.t, "> network Fine (settling)");
            self.proxy.set(Mode::Fine);
        }
        let limit = self.t + SETTLE_LIMIT;
        loop {
            let snap = self.truth.lock().unwrap().clone();
            let calm = snap.up && !snap.busy() && snap.inbox.is_empty() && snap.helpers() == 0 && self.health.good_since.is_some_and(|g| self.t - g >= SETTLE);
            if calm {
                break;
            }
            if self.t > limit {
                self.oracle.never_settled(self.t, format!("still not settled {SETTLE_LIMIT:.0} s after the last step: server {}", super::trace::server_line(&snap)));
                return;
            }
            self.wait(1.0);
            if self.oracle.failed() {
                return;
            }
        }
        // The last read may predate the last frame.
        std::thread::sleep(Duration::from_millis(600));
        Desk::tick(self, 0.0);
        let snap = self.truth.lock().unwrap().clone();
        let screen = Screen::read(&self.rt, &self.sid);
        self.oracle.settled(self.t, &screen, &snap);
    }

    fn mode_now(&mut self) -> Mode {
        if let Some((until, after)) = self.outage
            && self.t >= until
        {
            self.outage = None;
            self.trace.note(self.t, format!("> the server is reachable again (network {after:?})"));
            self.proxy.set(after);
        }
        self.proxy.mode()
    }
}

impl Desk for RealWorld {
    fn rt(&mut self) -> &mut Runtime<App> {
        &mut self.rt
    }

    fn now(&self) -> f64 {
        self.t
    }

    /// One real frame: however long has passed since the last (`dt` is ignored).
    fn tick(&mut self, _dt: f64) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.t = now.duration_since(self.start).as_secs_f64();
        let t = self.t;
        let mode = self.mode_now();
        match (mode, self.health.good_since) {
            (Mode::Fine, None) => self.health.good_since = Some(t),
            (Mode::Fine, Some(_)) => {}
            _ => self.health.good_since = None,
        }
        if mode == Mode::Dead {
            self.health.last_dead = Some(t);
        }
        if mode == Mode::Down {
            self.health.last_down = Some(t);
        }
        self.rt.frame(dt);
        self.flush();
        let mut snap = self.truth.lock().unwrap().clone();
        snap.up &= mode != Mode::Down;
        let screen = Screen::read(&self.rt, &self.sid);
        self.oracle.check(t, &screen, &snap, self.health);
        let link = format!("{mode:?}").to_lowercase();
        self.trace.frame(t, &screen, &snap, &link);
    }

    fn flush(&mut self) {
        while let Ok(w) = self.writes.try_recv() {
            if matches!(w, Write::Interrupt(_)) {
                self.interrupts += 1;
            }
            let _ = self.wire.send(w);
        }
    }

    fn interrupts(&self) -> usize {
        self.interrupts
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
