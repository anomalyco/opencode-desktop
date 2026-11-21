//! Live session transcript state, page merging, gap barrier, and `/api/event` application.

use std::collections::HashSet;

use serde_json::Value;

use super::{
    Assistant, CompactionStatus, Message, Part, Retry, StepError, Tokens, ToolStatus, compaction,
    keep_tool_error, keep_tool_output, model_of, notice, parse_attachments, replace_tool_metadata,
    s, set_user_created, strings, tool_title,
};

/// In a dropped stream's gapped steps (`Transcript::apply_caught`): a step not known yet when it
/// dropped is gapped too, until the page read after it lands.
pub const UNSEEN: &str = "\u{0}unseen";

/// One open transcript, kept current from events.
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub session: String,
    pub messages: Vec<Message>,
}

impl Transcript {
    fn assistant(&mut self, id: &str) -> Option<&mut Assistant> {
        self.messages.iter_mut().rev().find_map(|m| match m {
            Message::Assistant(a) if a.id == id => Some(a),
            _ => None,
        })
    }

    /// The assistant step still producing output, if any.
    pub fn busy(&self) -> bool {
        // A notice (a model switched with the prompt) doesn't end the turn it lands in.
        let last = self.messages.iter().rev().find(|m| !matches!(m, Message::Notice(_)));
        matches!(last, Some(Message::Assistant(a)) if a.completed.is_none() && a.error.is_none())
            || matches!(last, Some(Message::User { .. }))
    }

    /// The latest assistant step while it's unfinished: the reply whose calls are running. Only
    /// the newest step can be; an older one left without its end (a stream that dropped) isn't
    /// the one at work. It stops at the newest assistant step, so it reads a few messages at any
    /// transcript size.
    pub fn open_reply(&self) -> Option<&Assistant> {
        self.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        }).filter(|a| a.completed.is_none())
    }

    /// What a stream that drops now leaves mid-text: the assistant steps still open, and
    /// ([`UNSEEN`]) any step the drop hides the start of, until the next page read says.
    pub fn open_steps(&self) -> HashSet<String> {
        self.messages.iter().filter_map(|m| match m {
            Message::Assistant(a) if a.completed.is_none() && a.error.is_none() => Some(a.id.clone()),
            _ => None,
        }).chain(std::iter::once(UNSEEN.to_owned())).collect()
    }

    /// [`Transcript::apply`] after the stream dropped while the steps in `gapped` were open (the
    /// catching-up barrier). OpenCode's deltas carry no offset, so one heard after the gap
    /// can't be told apart from one the gap swallowed: a gapped step's deltas are held back, and
    /// its text grows again only from whole text (`*.ended`, a page read). What it shows stays a
    /// prefix of what it ends as: nothing lost into the middle, nothing said twice. Returns
    /// whether it changed, and whether the step ended without its whole text (its page should be
    /// read again).
    pub fn apply_caught(&mut self, ty: &str, d: &Value, now: i64, gapped: &mut HashSet<String>) -> (bool, bool) {
        let mid = s(&d["assistantMessageID"]);
        // A step heard of only after the drop, mid-text, may have started in the gap: its start
        // is gone, so it's gapped too. One whose start is heard now began after it.
        if gapped.contains(UNSEEN) && !mid.is_empty() && self.assistant(&mid).is_none() && matches!(ty, "session.text.delta" | "session.reasoning.delta") {
            gapped.insert(mid.clone());
        }
        if !gapped.contains(&mid) {
            return (self.apply(ty, d, now), false);
        }
        match ty {
            "session.text.delta" | "session.reasoning.delta" => (false, false),
            "session.step.ended" | "session.step.failed" => {
                gapped.remove(&mid);
                (self.apply(ty, d, now), true)
            }
            _ => (self.apply(ty, d, now), false),
        }
    }

    /// [`Transcript::merge`] after a dropped stream (which, like any page read, keeps the text a
    /// step shows where the page's copy is only a start of it): an open step this page brings
    /// that the drop hid joins the barrier, and a step the page says ended leaves it.
    pub fn merge_caught(&mut self, fresh: Vec<Message>, gapped: &mut HashSet<String>) {
        let unseen = gapped.remove(UNSEEN);
        for m in &fresh {
            let Message::Assistant(new) = m else { continue };
            let open = new.completed.is_none() && new.error.is_none();
            // An open step this page brings that the drop hid: what streams next is its middle.
            if unseen && open && !self.messages.iter().any(|o| o.id() == new.id) {
                gapped.insert(new.id.clone());
            }
            if !open {
                gapped.remove(&new.id);
            }
        }
        self.merge(fresh);
    }

    pub fn merge(&mut self, fresh: Vec<Message>) {
        let mut prev: Option<String> = None;
        for mut m in fresh {
            let id = m.id().to_owned();
            let after = prev.replace(id).and_then(|p| self.messages.iter().position(|o| o.id() == p));
            if let Some(slot) = self.messages.iter_mut().find(|o| o.id() == m.id()) {
                // Keep live-streamed text when the fetched copy is behind.
                if let (Message::Assistant(old), Message::Assistant(new)) = (&*slot, &mut m)
                    && new.completed.is_none()
                {
                    if old.parts.len() > new.parts.len() {
                        continue;
                    }
                    // OpenCode writes a part's text when the part ends: a page read mid-reply has
                    // it empty or short, and taking it would wipe what streamed (and the deltas
                    // after would run on from nothing).
                    for (fresh_part, shown) in new.parts.iter_mut().zip(&old.parts) {
                        match (fresh_part, shown) {
                            (Part::Text { text: f }, Part::Text { text: o }) | (Part::Reasoning { text: f, .. }, Part::Reasoning { text: o, .. }) if o.starts_with(f.as_str()) => *f = o.clone(),
                            _ => {}
                        }
                    }
                    for &(idx, phase) in &old.text_phases {
                        if new.text_phase(idx).is_none() {
                            new.set_text_phase(idx, phase);
                        }
                    }
                }
                // Nor rewind a compaction that ended to a copy read while it ran.
                if let (Message::Compaction(old), Message::Compaction(new)) = (&*slot, &m)
                    && old.status != CompactionStatus::Running
                    && new.status == CompactionStatus::Running
                {
                    continue;
                }
                *slot = m;
            } else {
                // After the page's previous message, so a steered prompt stays where the server put it.
                let at = after.map_or_else(|| self.messages.partition_point(|o| o.id() < m.id()), |a| a + 1);
                self.messages.insert(at, m);
            }
        }
    }

    /// How many messages show under a staged revert at `boundary`: those before it (ids sort by time).
    pub fn visible_len(&self, boundary: Option<&str>) -> usize {
        boundary.map_or(self.messages.len(), |b| self.messages.partition_point(|m| m.id() < b))
    }

    /// Applies one `/api/event` payload for this session; returns whether anything changed.
    pub fn apply(&mut self, ty: &str, d: &Value, now: i64) -> bool {
        if d["sessionID"].as_str() != Some(self.session.as_str()) {
            return false;
        }
        let mid = s(&d["assistantMessageID"]);
        let ordinal = d["ordinal"].as_u64().map(|o| o as usize);
        match ty {
            "session.prompted" | "session.user.message" => {
                let id = s(&d["messageID"]);
                if !id.is_empty() && !self.messages.iter().any(|m| m.id() == id) {
                    set_user_created(&id, d["time"]["created"].as_i64().unwrap_or(now));
                    let text = s(&d["text"]);
                    crate::mention::record(&id, &text, d);
                    self.messages.push(Message::User { id, text });
                }
            }
            "session.step.started" => {
                let (model, agent) = (model_of(&d["model"]), d["agent"].as_str().filter(|a| !a.is_empty()).map(str::to_owned));
                match self.assistant(&mid) {
                    // A retried step starts again under its own id.
                    Some(a) => {
                        a.retry = None;
                        a.error = None;
                        a.failure = None;
                        a.first_token = None;
                        a.streamed = None;
                        a.model = model.or(a.model.take());
                        a.agent = agent.or(a.agent.take());
                    }
                    None => self.messages.push(Message::Assistant(Assistant { id: mid, created: now, model, agent, ..Default::default() })),
                }
            }
            "session.step.first-token" => {
                let Some(a) = self.assistant(&mid) else { return false };
                a.first_token = Some(now);
            }
            "session.step.streamed" => {
                let Some(a) = self.assistant(&mid) else { return false };
                a.streamed = Some(now);
            }
            "session.retry.scheduled" => {
                let Some(a) = self.assistant(&mid) else { return false };
                a.retry = Retry::parse(d);
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                // The run ends, as the idle message the server records says (a shutdown resumes it).
                let idle = !(ty == "session.execution.interrupted" && d["reason"] == "shutdown") && self.apply_idle(d);
                let Some(Message::Assistant(a)) = self.messages.iter_mut().rev().find(|m| matches!(m, Message::Assistant(_))) else { return idle };
                if a.retry.take().is_none() {
                    return idle;
                }
            }
            ty if notice::is_notice_event(ty) => return self.apply_notice(ty, d),
            "session.text.started" | "session.reasoning.started" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let is_text = ty == "session.text.started";
                let part = if is_text {
                    Part::Text { text: String::new() }
                } else {
                    Part::Reasoning { text: String::new(), started: now, completed: None }
                };
                let at = slot(&a.parts, is_text, ordinal);
                let idx = at.unwrap_or(a.parts.len());
                place(&mut a.parts, at, part);
                if is_text && let Some(phase) = super::TextPhase::parse(&d["native"]["phase"]) {
                    a.set_text_phase(idx, phase);
                }
            }
            "session.text.delta" | "session.reasoning.delta" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let delta = d["delta"].as_str().unwrap_or("");
                let want_text = ty == "session.text.delta";
                let at = slot(&a.parts, want_text, ordinal).or_else(|| slot(&a.parts, want_text, None));
                match at.map(|k| &mut a.parts[k]) {
                    Some(Part::Text { text }) => text.push_str(delta),
                    Some(Part::Reasoning { text, .. }) => text.push_str(delta),
                    _ => {
                        let part = if want_text {
                            Part::Text { text: delta.to_owned() }
                        } else {
                            Part::Reasoning { text: delta.to_owned(), started: now, completed: None }
                        };
                        a.parts.push(part);
                    }
                }
            }
            "session.text.ended" | "session.reasoning.ended" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let full = d["text"].as_str().map(str::to_owned);
                let want_text = ty == "session.text.ended";
                let idx = slot(&a.parts, want_text, ordinal).or_else(|| slot(&a.parts, want_text, None));
                match idx.map(|k| (k, &mut a.parts[k])) {
                    Some((k, Part::Text { text })) => {
                        if let Some(f) = full {
                            *text = f;
                        }
                        if let Some(phase) = super::TextPhase::parse(&d["native"]["phase"]) {
                            a.set_text_phase(k, phase);
                        }
                    }
                    Some((_, Part::Reasoning { text, completed, .. })) => {
                        if let Some(f) = full {
                            *text = f;
                        }
                        *completed = Some(now);
                    }
                    _ => return false,
                }
            }
            "session.tool.input.started" | "session.tool.called" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let id = s(&d["id"]);
                let input = d["input"].clone();
                let found = a.parts.iter_mut().find_map(|p| match p {
                    Part::Tool { id: tid, name, status, title, input: inp, .. } if *tid == id => {
                        Some((name, status, title, inp))
                    }
                    _ => None,
                });
                match found {
                    Some((name, status, title, inp)) => {
                        if ty == "session.tool.called" {
                            *status = ToolStatus::Running;
                            *title = tool_title(name, &input);
                            *inp = input;
                        }
                    }
                    None => {
                        let name = s(&d["name"]);
                        a.parts.push(Part::Tool {
                            id,
                            title: tool_title(&name, &input),
                            name,
                            status: if ty == "session.tool.called" { ToolStatus::Running } else { ToolStatus::Pending },
                            input,
                            files: Vec::new(),
                            metadata: Value::Null,
                        });
                    }
                }
            }
            "session.tool.progress" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let id = s(&d["id"]);
                let mut hit = false;
                for p in &mut a.parts {
                    // Progress counts only while the call runs, as the official reducer takes it:
                    // a late report never rewrites a finished call (a completed script reading
                    // as backgrounded).
                    if let Part::Tool { id: tid, metadata, status, .. } = p
                        && *tid == id
                        && *status == ToolStatus::Running
                        && d["metadata"].is_object()
                    {
                        replace_tool_metadata(metadata, &d["metadata"]);
                        hit = true;
                    }
                }
                return hit;
            }
            "session.tool.success" | "session.tool.failed" => {
                let Some(a) = self.assistant(&mid) else { return false };
                let id = s(&d["id"]);
                for p in &mut a.parts {
                    if let Part::Tool { id: tid, status, files, metadata, .. } = p
                        && *tid == id
                    {
                        *status = if ty == "session.tool.success" { ToolStatus::Completed } else { ToolStatus::Error };
                        if d["metadata"].is_object() {
                            replace_tool_metadata(metadata, &d["metadata"]);
                        }
                        if ty == "session.tool.failed" {
                            keep_tool_error(metadata, &d["error"]);
                        }
                        keep_tool_output(metadata, &d["content"]);
                        let got = parse_attachments(&d["content"]);
                        if !got.is_empty() {
                            *files = got;
                        }
                    }
                }
            }
            "session.step.ended" | "session.step.failed" => {
                let Some(a) = self.assistant(&mid) else { return false };
                a.completed = Some(now);
                a.cost = d["cost"].as_f64().unwrap_or(a.cost);
                if d["tokens"].is_object() {
                    a.tokens = Some(Tokens::parse(&d["tokens"]));
                }
                a.retry = None;
                if d["files"].is_array() {
                    a.changed = strings(&d["files"]);
                }
                if ty == "session.step.failed" {
                    a.error = Some(d["error"]["message"].as_str().unwrap_or("The step failed").to_owned());
                    a.failure = StepError::parse(&d["error"]);
                }
            }
            "session.compaction.started" | "session.compaction.delta" | "session.compaction.ended" | "session.compaction.failed" => {
                return compaction::apply(&mut self.messages, ty, d, now);
            }
            "session.revert.committed" => {
                let at = self.visible_len(d["to"].as_str());
                if at == self.messages.len() {
                    return false;
                }
                self.messages.truncate(at);
            }
            _ => return false,
        }
        true
    }
}

/// Where a step's text or reasoning part with `ordinal` sits in its mixed `parts`. The server
/// counts each kind on its own (a step's first thought and first answer are both 0), so it's
/// the ordinal-th part of that kind. Without an ordinal, the last part if it's that kind.
fn slot(parts: &[Part], text: bool, ordinal: Option<usize>) -> Option<usize> {
    let kind = |p: &Part| if text { matches!(p, Part::Text { .. }) } else { matches!(p, Part::Reasoning { .. }) };
    match ordinal {
        Some(o) => parts.iter().enumerate().filter(|(_, p)| kind(p)).nth(o).map(|(i, _)| i),
        None => parts.last().filter(|p| kind(p)).map(|_| parts.len() - 1),
    }
}

fn place(parts: &mut Vec<Part>, at: Option<usize>, part: Part) {
    match at {
        Some(i) => parts[i] = part,
        None => parts.push(part),
    }
}
