//! A deterministic OpenCode server for one session, as `packages/core/src/session` (opencode
//! dev) runs it: prompts are admitted to an inbox (idempotent by id) and delivered when a run
//! takes them up; steers join at the next step boundary, queued prompts once the run ends; an
//! interrupt fails the open step and ends the run, leaving the inbox as it was. It emits the
//! same `/api/event` payloads the real one does and answers reads from its own state.
//!
//! Checked against a real server's streams (`recorded/`, `recorded_tests.rs`): a queued prompt
//! joins the same run once its last step ends; a tool step ends with `finish: tool-calls` and a
//! new step says the words; a stop closes the open text before the step fails; every run ends
//! with an `idle` message; a background helper's result comes back as a synthetic inbox item
//! that wakes its parent.
//!
//! Still guesses: the shutdown on a restart closes the open step as interrupted.

use std::collections::{HashMap, HashSet, VecDeque};

use opencode_kit::model::SessionInfo;
use serde_json::{Value, json};

use super::Rng;
use super::oracle::Truth;

/// The epoch the sim's clock starts at, in ms; `time.created` and the app's wall clock count
/// from here.
pub const T0: i64 = 1_800_000_000_000;

#[derive(Clone, Debug)]
pub enum Msg {
    User { id: String, text: String, created: i64 },
    Asst { id: String, created: i64, text: String, tool: Option<Tool>, completed: Option<i64>, error: bool },
    /// A finished background helper's result, delivered to its parent.
    Synthetic { id: String, created: i64, child: String },
    /// The server's marker that a run ended (`outcome`: succeeded or interrupted).
    Idle { id: String, created: i64, outcome: &'static str },
}

impl Msg {
    pub fn id(&self) -> &str {
        match self {
            Msg::User { id, .. } | Msg::Asst { id, .. } | Msg::Synthetic { id, .. } | Msg::Idle { id, .. } => id,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tool {
    pub id: String,
    pub name: &'static str,
    pub done: bool,
    pub failed: bool,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub text: String,
    pub queue: bool,
    /// A finished helper's result (its session id), not a prompt anyone typed.
    pub synthetic: Option<String>,
}

/// What the current step does over time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Streams words.
    Reply,
    /// Calls a tool that runs; the step ends with it, and the next one says the words.
    Tool,
    /// Calls `subagent`: a subagent session runs under it until the tool returns.
    Subagent,
    /// Calls `subagent` in the background: the tool returns at once and the helper runs on.
    Background,
}

struct Step {
    asst: String,
    kind: Kind,
    /// Timed pieces still to come, earliest first.
    plan: Vec<(f64, Piece)>,
    child: Option<String>,
    /// `session.text.started` went out, so the text needs ending.
    text_open: bool,
    secs: f64,
}

#[derive(Clone, Debug)]
enum Piece {
    TextStart,
    Delta(String),
    ToolCall,
    ToolDone,
    End,
}

struct Run {
    step: Option<Step>,
    steps_left: u32,
}

pub struct Child {
    pub id: String,
    until: f64,
    step: String,
    /// The parent step's tool it runs under (blocking), or None for a background helper.
    blocking: bool,
}

pub struct Sim {
    pub sid: String,
    pub t: f64,
    pub up: bool,
    down_until: f64,
    pub msgs: Vec<Msg>,
    pub inbox: Vec<Item>,
    run: Option<Run>,
    pub children: Vec<Child>,
    pub child_ids: Vec<String>,
    /// Every prompt id admitted, for idempotent admission.
    admitted: HashSet<String>,
    created: i64,
    pub updated: i64,
    pub idle: Option<i64>,
    seed: u64,
    runs: u64,
    /// Events emitted since the link last took them: (time, type, data).
    pub out: Vec<(f64, String, Value)>,
    template: SessionInfo,
    /// The next steps' kinds and lengths, for replaying a recorded scenario; a tool step in it
    /// is followed by one more step, a reply ends the run. Empty: seeded.
    pub script: VecDeque<(Kind, f64)>,
}

const STEP_SECS: (f64, f64) = (1.0, 6.0);

impl Sim {
    pub fn new(seed: u64, template: SessionInfo) -> Self {
        let sid = template.id.clone();
        let mut sim = Sim {
            sid,
            t: 0.0,
            up: true,
            down_until: 0.0,
            msgs: vec![],
            inbox: vec![],
            run: None,
            children: vec![],
            child_ids: vec![],
            admitted: HashSet::new(),
            created: T0 - 60_000,
            updated: T0,
            idle: Some(T0),
            seed,
            runs: 0,
            out: vec![],
            template,
            script: VecDeque::new(),
        };
        let h = opencode_kit::client::message_id();
        let created = sim.next_created();
        sim.msgs.push(Msg::User { id: h, text: "h1".into(), created });
        let a = opencode_kit::client::message_id();
        let created = sim.next_created();
        sim.msgs.push(Msg::Asst { id: a, created, text: "Earlier answer.".into(), tool: None, completed: Some(created + 1000), error: false });
        sim
    }

    pub fn now_ms(&self) -> i64 {
        T0 + (self.t * 1000.0) as i64
    }

    fn next_created(&mut self) -> i64 {
        self.created = (self.created + 1).max(self.now_ms());
        self.created
    }

    fn emit(&mut self, ty: &str, mut data: Value) {
        if data["sessionID"].is_null() {
            data["sessionID"] = self.sid.clone().into();
        }
        self.updated = self.now_ms();
        self.out.push((self.t, ty.to_owned(), data));
    }

    pub fn busy(&self) -> bool {
        self.run.is_some()
    }

    pub fn child_busy(&self, id: &str) -> bool {
        self.children.iter().any(|c| c.id == id)
    }

    /// Whether the server has prompt `id`, delivered or waiting.
    pub fn has(&self, id: &str) -> bool {
        self.delivered(id) || self.prompts().any(|i| i.id == id)
    }

    pub fn delivered(&self, id: &str) -> bool {
        self.msgs.iter().any(|m| matches!(m, Msg::User { id: u, .. } if u == id))
    }

    /// `POST /prompt`: admits the prompt (once per id) and wakes an idle session.
    pub fn admit(&mut self, id: &str, text: &str, queue: bool) {
        if !self.admitted.insert(id.to_owned()) {
            return;
        }
        self.inbox.push(Item { id: id.into(), text: text.into(), queue, synthetic: None });
        let delivery = if queue { "queue" } else { "steer" };
        self.emit("session.inbox.enqueued", json!({ "inboxID": id, "item": { "type": "user", "delivery": delivery, "payload": { "text": text } } }));
        if self.run.is_none() {
            self.wake();
        }
    }

    /// `POST /interrupt`: whether a run was interrupted. As on the real server, the step has
    /// failed and the run ended before it answers; the inbox stays as it was.
    pub fn interrupt(&mut self) -> bool {
        let Some(run) = self.run.take() else { return false };
        let child = run.step.as_ref().and_then(|s| s.child.clone());
        if let Some(step) = run.step {
            self.close_step(&step, true);
        }
        self.emit("session.execution.interrupted", json!({ "reason": "user" }));
        self.end_run("interrupted");
        // The subagent it waited on stops after it.
        if let Some(at) = child.and_then(|id| self.children.iter().position(|c| c.id == id && c.blocking)) {
            self.end_child(at, "interrupted");
        }
        true
    }

    /// `DELETE …/inbox/:item`.
    pub fn cancel(&mut self, id: &str) {
        if let Some(at) = self.inbox.iter().position(|i| i.id == id) {
            self.inbox.remove(at);
            self.emit("session.inbox.cancelled", json!({ "inboxID": id }));
        }
    }

    /// `PATCH …/inbox/:item`.
    pub fn set_delivery(&mut self, id: &str, queue: bool) {
        if let Some(i) = self.inbox.iter_mut().find(|i| i.id == id)
            && i.queue != queue
        {
            i.queue = queue;
            let delivery = if queue { "queue" } else { "steer" };
            self.emit("session.inbox.delivery.changed", json!({ "inboxID": id, "delivery": delivery }));
        }
    }

    /// A background helper (a detached subagent) starts under this session and runs `secs`.
    pub fn helper(&mut self, secs: f64) {
        if !self.up {
            return;
        }
        let id = self.new_child();
        self.start_child(&id, secs, false);
    }

    fn new_child(&mut self) -> String {
        let id = format!("ses_fuzzchild{:04}aaaaaaaaaaaaa", self.child_ids.len() + 1);
        self.child_ids.push(id.clone());
        id
    }

    fn start_child(&mut self, id: &str, secs: f64, blocking: bool) {
        let step = opencode_kit::client::message_id();
        let prompt = opencode_kit::client::message_id();
        let item = json!({ "type": "user", "delivery": "steer", "payload": { "text": "You are a subagent spawned by another session.\nhelp" } });
        self.emit("session.inbox.enqueued", json!({ "sessionID": id, "inboxID": prompt, "item": item }));
        self.emit("session.execution.started", json!({ "sessionID": id }));
        self.emit("session.inbox.delivered", json!({ "sessionID": id, "inboxID": prompt }));
        self.emit("session.step.started", json!({ "sessionID": id, "assistantMessageID": step, "agent": "general", "model": { "id": "fuzz", "providerID": "fuzz" } }));
        self.emit("session.text.started", json!({ "sessionID": id, "assistantMessageID": step, "ordinal": 0 }));
        self.children.push(Child { id: id.into(), until: self.t + secs, step, blocking });
    }

    fn end_child(&mut self, at: usize, how: &str) {
        let c = self.children.remove(at);
        let error = json!({ "type": "aborted", "message": "Step interrupted" });
        self.emit("session.text.ended", json!({ "sessionID": c.id, "assistantMessageID": c.step, "ordinal": 0, "text": "done" }));
        self.emit("session.step.streamed", json!({ "sessionID": c.id, "assistantMessageID": c.step }));
        match how {
            "succeeded" => self.emit("session.step.ended", json!({ "sessionID": c.id, "assistantMessageID": c.step, "finish": "stop" })),
            _ => self.emit("session.step.failed", json!({ "sessionID": c.id, "assistantMessageID": c.step, "error": error })),
        }
        let ty = format!("session.execution.{how}");
        match how {
            "interrupted" => self.emit(&ty, json!({ "sessionID": c.id, "reason": "user" })),
            _ => self.emit(&ty, json!({ "sessionID": c.id })),
        }
        // A background helper's result goes back to its parent, which takes it up as a steer.
        if !c.blocking && how == "succeeded" {
            let id = opencode_kit::client::message_id();
            let text = format!("<subagent sessionID=\"{}\" state=\"completed\" description=\"fuzz helper\">\ndone\n</subagent>", c.id);
            let item = json!({ "type": "synthetic", "delivery": "steer", "payload": { "text": text, "description": "fuzz helper", "metadata": { "source": "subagent", "childID": c.id } } });
            self.emit("session.inbox.enqueued", json!({ "inboxID": id, "item": item }));
            self.inbox.push(Item { id, text, queue: false, synthetic: Some(c.id) });
            if self.run.is_none() {
                self.wake();
            }
        }
    }

    /// A run ended: the server records an `idle` message.
    fn end_run(&mut self, outcome: &'static str) {
        self.run = None;
        let created = self.next_created();
        self.msgs.push(Msg::Idle { id: opencode_kit::client::message_id(), created, outcome });
        self.idle = Some(self.now_ms());
    }

    /// The process stops (its stream closes after a shutdown interrupt) and comes back `secs` later.
    pub fn restart(&mut self, secs: f64) {
        if !self.up {
            return;
        }
        if let Some(run) = self.run.take() {
            if let Some(step) = run.step {
                self.close_step(&step, true);
            }
            self.emit("session.execution.interrupted", json!({ "reason": "shutdown" }));
            self.end_run("interrupted");
        }
        while !self.children.is_empty() {
            let c = self.children.remove(0);
            self.emit("session.execution.interrupted", json!({ "sessionID": c.id, "reason": "shutdown" }));
        }
        self.up = false;
        self.down_until = self.t + secs;
    }

    fn rng(&self, salt: u64) -> Rng {
        Rng::new(self.seed ^ (self.runs.wrapping_mul(0x9e37_79b9_7f4a_7c15)) ^ salt)
    }

    /// Starts a run if anything waits: every steer, or else the first queued prompt.
    fn wake(&mut self) {
        if self.run.is_some() || !self.up {
            return;
        }
        let steers: Vec<String> = self.inbox.iter().filter(|i| !i.queue).map(|i| i.id.clone()).collect();
        let take = if steers.is_empty() { self.inbox.iter().find(|i| i.queue).map(|i| vec![i.id.clone()]).unwrap_or_default() } else { steers };
        if take.is_empty() {
            return;
        }
        self.runs += 1;
        let steps_left = 1 + self.rng(1).below(3) as u32;
        self.run = Some(Run { step: None, steps_left });
        self.idle = None;
        self.emit("session.execution.started", json!({}));
        for id in take {
            self.deliver(&id);
        }
        self.begin_step();
    }

    fn deliver(&mut self, id: &str) {
        let Some(at) = self.inbox.iter().position(|i| i.id == id) else { return };
        let item = self.inbox.remove(at);
        let created = self.next_created();
        match item.synthetic {
            Some(child) => self.msgs.push(Msg::Synthetic { id: item.id.clone(), created, child }),
            None => self.msgs.push(Msg::User { id: item.id.clone(), text: item.text, created }),
        }
        self.emit("session.inbox.delivered", json!({ "inboxID": item.id }));
    }

    fn begin_step(&mut self) {
        let n = self.msgs.len() as u64;
        let mut rng = self.rng(n);
        let scripted = self.script.pop_front();
        let Some(run) = &mut self.run else { return };
        run.steps_left = run.steps_left.saturating_sub(1);
        let last = run.steps_left == 0;
        let secs = STEP_SECS.0 + rng.unit() * (STEP_SECS.1 - STEP_SECS.0);
        // A run's last step says the words; a tool step always has another after it.
        let (kind, secs) = match (scripted, rng.below(5)) {
            (Some((kind, secs)), _) => {
                run.steps_left = u32::from(kind != Kind::Reply);
                (kind, secs)
            }
            (None, _) if last => (Kind::Reply, secs),
            (None, 0 | 1) => (Kind::Reply, secs),
            (None, 2) => (Kind::Tool, secs),
            (None, 3) => (Kind::Subagent, secs),
            (None, _) => (Kind::Background, secs * 2.0),
        };
        let asst = opencode_kit::client::message_id();
        let created = self.next_created();
        let t = self.t;
        let mut plan = vec![];
        if kind == Kind::Reply {
            plan.push((t + 0.05, Piece::TextStart));
            for k in 0..(secs / 0.4) as usize {
                plan.push((t + 0.1 + k as f64 * 0.4, Piece::Delta(format!("w{k} "))));
            }
            plan.push((t + secs, Piece::End));
        } else if kind == Kind::Background {
            // `secs` is how long the helper runs; the tool itself returns at once.
            plan.push((t + 0.3, Piece::ToolCall));
            plan.push((t + 0.4, Piece::ToolDone));
            plan.push((t + 0.41, Piece::End));
        } else {
            plan.push((t + 0.3, Piece::ToolCall));
            plan.push((t + secs - 0.3, Piece::ToolDone));
            plan.push((t + secs - 0.29, Piece::End));
        }
        plan.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.msgs.push(Msg::Asst { id: asst.clone(), created, text: String::new(), tool: None, completed: None, error: false });
        self.emit("session.step.started", json!({ "assistantMessageID": asst, "agent": "build", "model": { "id": "fuzz", "providerID": "fuzz" } }));
        if let Some(run) = &mut self.run {
            run.step = Some(Step { asst, kind, plan, child: None, text_open: false, secs });
        }
    }

    fn asst_mut(&mut self, id: &str) -> Option<(&mut String, &mut Option<Tool>, &mut Option<i64>, &mut bool)> {
        self.msgs.iter_mut().rev().find_map(|m| match m {
            Msg::Asst { id: a, text, tool, completed, error, .. } if a == id => Some((text, tool, completed, error)),
            _ => None,
        })
    }

    /// Ends `step`: its open text, then (on a stop) its unfinished tool, then the step itself.
    fn close_step(&mut self, step: &Step, failed: bool) {
        let asst = step.asst.as_str();
        let now = self.now_ms();
        if step.text_open {
            let text = self.asst_mut(asst).map(|(t, ..)| t.clone()).unwrap_or_default();
            self.emit("session.text.ended", json!({ "assistantMessageID": asst, "ordinal": 0, "text": text }));
        }
        let mut cut_tool = None;
        if let Some((_, tool, completed, error)) = self.asst_mut(asst) {
            *completed = Some(now);
            *error = failed;
            if let Some(t) = tool.as_mut().filter(|t| failed && !t.done) {
                t.done = true;
                t.failed = true;
                cut_tool = Some(t.id.clone());
            }
        }
        if let Some(id) = cut_tool {
            self.emit("session.tool.failed", json!({ "assistantMessageID": asst, "id": id, "error": { "type": "aborted", "message": "Tool interrupted" } }));
        }
        // A tool step said it was streamed when it called the tool.
        if step.kind == Kind::Reply {
            self.emit("session.step.streamed", json!({ "assistantMessageID": asst }));
        }
        if failed {
            let error = json!({ "type": "aborted", "message": "Step interrupted" });
            self.emit("session.step.failed", json!({ "assistantMessageID": asst, "error": error }));
        } else {
            let finish = if step.kind == Kind::Reply { "stop" } else { "tool-calls" };
            self.emit("session.step.ended", json!({ "assistantMessageID": asst, "finish": finish }));
        }
    }

    /// Runs the server forward to `to`.
    pub fn advance(&mut self, to: f64) {
        loop {
            let next = self.next_due();
            match next {
                Some(at) if at <= to => {
                    self.t = self.t.max(at);
                    self.fire();
                }
                _ => break,
            }
        }
        self.t = to;
    }

    pub fn next_due(&self) -> Option<f64> {
        let mut due: Vec<f64> = vec![];
        if !self.up {
            due.push(self.down_until);
        }
        if let Some(s) = self.run.as_ref().and_then(|r| r.step.as_ref()) {
            due.extend(s.plan.first().map(|p| p.0));
        }
        due.extend(self.children.iter().filter(|c| !c.blocking).map(|c| c.until));
        due.into_iter().reduce(f64::min)
    }

    fn fire(&mut self) {
        let t = self.t;
        if !self.up {
            if self.down_until <= t {
                self.up = true;
            }
            return;
        }
        if let Some(at) = self.children.iter().position(|c| !c.blocking && c.until <= t) {
            return self.end_child(at, "succeeded");
        }
        let Some(run) = &mut self.run else { return };
        let Some(step) = &mut run.step else { return };
        if step.plan.is_empty() {
            return;
        }
        let (_, piece) = step.plan.remove(0);
        let asst = step.asst.clone();
        let kind = step.kind;
        match piece {
            Piece::TextStart => {
                if let Some(Run { step: Some(s), .. }) = &mut self.run {
                    s.text_open = true;
                }
                self.emit("session.text.started", json!({ "assistantMessageID": asst, "ordinal": 0 }))
            }
            Piece::Delta(w) => {
                if let Some((text, ..)) = self.asst_mut(&asst) {
                    text.push_str(&w);
                }
                self.emit("session.text.delta", json!({ "assistantMessageID": asst, "ordinal": 0, "delta": w }));
            }
            Piece::ToolCall => {
                let id = format!("call_{asst}");
                let helper = matches!(kind, Kind::Subagent | Kind::Background);
                let name = if helper { "subagent" } else { "shell" };
                if let Some((_, tool, ..)) = self.asst_mut(&asst) {
                    *tool = Some(Tool { id: id.clone(), name, done: false, failed: false });
                }
                let input = match kind {
                    Kind::Subagent => json!({ "agent": "general", "description": "fuzz helper", "prompt": "help" }),
                    Kind::Background => json!({ "agent": "general", "description": "fuzz helper", "prompt": "help", "background": true }),
                    _ => json!({ "command": "sleep 1" }),
                };
                self.emit("session.tool.input.started", json!({ "assistantMessageID": asst, "id": id, "name": name }));
                self.emit("session.tool.input.ended", json!({ "assistantMessageID": asst, "id": id, "text": input.to_string() }));
                self.emit("session.tool.called", json!({ "assistantMessageID": asst, "id": id, "input": input, "executed": false }));
                self.emit("session.step.streamed", json!({ "assistantMessageID": asst }));
                if helper {
                    let child = self.new_child();
                    let mut secs = 1e9;
                    if let Some(Run { step: Some(s), .. }) = &mut self.run {
                        s.child = Some(child.clone());
                        if kind == Kind::Background {
                            secs = s.secs;
                        }
                    }
                    self.emit("session.tool.progress", json!({ "assistantMessageID": asst, "id": id, "metadata": { "sessionID": child, "status": "running" } }));
                    self.start_child(&child, secs, kind == Kind::Subagent);
                }
            }
            Piece::ToolDone => {
                let child = self.run.as_mut().and_then(|r| r.step.as_mut()).and_then(|s| s.child.take());
                let blocking = child.as_ref().and_then(|id| self.children.iter().position(|c| c.id == *id && c.blocking));
                if let Some(at) = blocking {
                    self.end_child(at, "succeeded");
                }
                let id = format!("call_{asst}");
                if let Some((_, Some(tool), ..)) = self.asst_mut(&asst) {
                    tool.done = true;
                }
                let mut done = json!({ "assistantMessageID": asst, "id": id, "content": [{ "type": "text", "text": "done" }] });
                if let Some(child) = child {
                    // A background helper is still running when its call returns.
                    let status = if kind == Kind::Background { "running" } else { "completed" };
                    done["metadata"] = json!({ "sessionID": child, "status": status });
                }
                self.emit("session.tool.success", done);
            }
            Piece::End => {
                let Some(step) = self.run.as_mut().and_then(|r| r.step.take()) else { return };
                self.close_step(&step, false);
                self.boundary();
            }
        }
    }

    /// A step ended: steers join the next step; with none and no steps left, the first queued
    /// prompt does, in the same run; with nothing waiting the run ends.
    fn boundary(&mut self) {
        let steers: Vec<String> = self.inbox.iter().filter(|i| !i.queue).map(|i| i.id.clone()).collect();
        let more = self.run.as_ref().is_some_and(|r| r.steps_left > 0);
        if !steers.is_empty() || more {
            for id in steers {
                self.deliver(&id);
            }
            return self.begin_step();
        }
        if let Some(next) = self.inbox.iter().find(|i| i.queue).map(|i| i.id.clone()) {
            self.deliver(&next);
            let steps = 1 + self.rng(2).below(3) as u32;
            if let Some(run) = &mut self.run {
                run.steps_left = steps;
            }
            return self.begin_step();
        }
        self.emit("session.execution.succeeded", json!({}));
        self.end_run("succeeded");
    }

    /// The latest `limit` messages, newest first, as `GET …/message?order=desc` pages them.
    pub fn page(&self, limit: usize) -> Value {
        let start = self.msgs.len().saturating_sub(limit);
        let data: Vec<Value> = self.msgs[start..].iter().rev().map(message_json).collect();
        json!({ "data": data, "cursor": { "next": null } })
    }

    /// `GET …/message/{id}`: the message, or none (a 404).
    pub fn message(&self, id: &str) -> Option<Value> {
        self.msgs.iter().find(|m| m.id() == id).map(|m| json!({ "data": message_json(m) }))
    }

    /// `GET …/inbox`'s items.
    pub fn inbox_json(&self) -> Value {
        let items: Vec<Value> = self
            .inbox
            .iter()
            .map(|i| {
                let ty = if i.synthetic.is_some() { "synthetic" } else { "user" };
                json!({ "id": i.id, "type": ty, "delivery": if i.queue { "queue" } else { "steer" }, "payload": { "text": i.text } })
            })
            .collect();
        Value::Array(items)
    }

    fn prompts(&self) -> impl Iterator<Item = &Item> {
        self.inbox.iter().filter(|i| i.synthetic.is_none())
    }

    /// The prompts the server has delivered, in transcript order.
    pub fn users(&self) -> Vec<String> {
        self.msgs.iter().filter_map(|m| matches!(m, Msg::User { .. }).then(|| m.id().to_owned())).collect()
    }

    /// Undelivered steers, then queued prompts, in admission order.
    pub fn waiting(&self, queue: bool) -> Vec<String> {
        self.prompts().filter(|i| i.queue == queue).map(|i| i.id.clone()).collect()
    }

    /// Settled: no run, no helper, nothing about to happen.
    pub fn idle_now(&self) -> bool {
        self.up && self.run.is_none() && self.children.is_empty()
    }

    pub fn active(&self) -> Vec<String> {
        let mut out: Vec<String> = self.run.iter().map(|_| self.sid.clone()).collect();
        out.extend(self.children.iter().map(|c| c.id.clone()));
        out
    }

    pub fn info(&self, id: &str) -> SessionInfo {
        let mut info = self.template.clone();
        info.updated = self.updated;
        if id == self.sid {
            info.idle = self.idle;
        } else {
            info.id = id.into();
            info.parent = Some(self.sid.clone());
            info.title = format!("Helper {id}");
            info.agent = "general".into();
            info.idle = (!self.child_busy(id)).then_some(self.updated);
        }
        info
    }

    pub fn sessions(&self) -> Vec<SessionInfo> {
        std::iter::once(self.sid.clone()).chain(self.child_ids.iter().cloned()).map(|id| self.info(&id)).collect()
    }
}

impl Truth for Sim {
    fn users(&self) -> Vec<String> {
        Sim::users(self)
    }

    fn inbox(&self) -> Vec<(String, String, bool)> {
        self.prompts().map(|i| (i.id.clone(), i.text.clone(), i.queue)).collect()
    }

    fn busy(&self) -> bool {
        Sim::busy(self)
    }

    fn up(&self) -> bool {
        self.up
    }

    fn helpers(&self) -> usize {
        self.children.iter().filter(|c| !c.blocking).count()
    }

    fn words(&self) -> HashMap<String, String> {
        let mut words: HashMap<String, String> = self
            .msgs
            .iter()
            .filter_map(|m| match m {
                Msg::User { id, text, .. } => Some((id.clone(), text.clone())),
                _ => None,
            })
            .collect();
        for i in self.prompts() {
            words.insert(i.id.clone(), i.text.clone());
        }
        words
    }

    fn delivered(&self, id: &str) -> bool {
        Sim::delivered(self, id)
    }

    fn has(&self, id: &str) -> bool {
        Sim::has(self, id)
    }
}

fn message_json(m: &Msg) -> Value {
    match m {
        Msg::User { id, text, created } => json!({ "id": id, "type": "user", "text": text, "time": { "created": created } }),
        Msg::Asst { id, created, text, tool, completed, error } => {
            let mut content = vec![];
            if let Some(t) = tool {
                let status = match (t.done, t.failed) {
                    (_, true) => "error",
                    (true, _) => "completed",
                    _ => "running",
                };
                content.push(json!({ "type": "tool", "id": t.id, "name": t.name, "executed": false, "state": { "status": status, "input": {}, "metadata": {} } }));
            }
            if !text.is_empty() {
                content.push(json!({ "type": "text", "text": text }));
            }
            let mut v = json!({ "id": id, "type": "assistant", "time": { "created": created }, "content": content });
            if let Some(c) = completed {
                v["time"]["completed"] = (*c).into();
            }
            if *error {
                v["error"] = json!({ "type": "aborted", "message": "Step interrupted" });
            }
            v
        }
        Msg::Synthetic { id, created, child } => json!({
            "id": id,
            "type": "synthetic",
            "time": { "created": created },
            "text": format!("<subagent sessionID=\"{child}\" state=\"completed\" description=\"fuzz helper\">\ndone\n</subagent>"),
            "description": "fuzz helper",
            "metadata": { "source": "subagent", "childID": child, "agent": "General", "state": "completed" },
        }),
        Msg::Idle { id, created, outcome } => json!({ "id": id, "type": "idle", "time": { "created": created }, "outcome": outcome }),
    }
}
