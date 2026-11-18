//! A subagent steered by later calls: each call that names the same child gets its own window
//! of the child's work. A call's window opens when the call began and closes when the child next
//! ended, or when the next call to it began, whichever came first; a call cut short by the next
//! is Steered.

use opencode_kit::model::{self, Message, Part, ToolStatus};

use super::{Run, State};
use crate::App;

/// Where one call's share of its child's work begins, and whether a later call followed it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Window {
    /// When the call began, when known.
    pub from: Option<i64>,
    /// When the next call to the same child began (`Some(None)` when that isn't known), or None
    /// for the newest call.
    pub next: Option<Option<i64>>,
}

impl App {
    /// When each `subagent` or `task` call in `parent` that names `child` began, in order, by
    /// tool part id.
    pub(crate) fn calls_to(&self, parent: &str, child: &str) -> Vec<(&str, Option<i64>)> {
        let mut out = Vec::new();
        for m in self.transcripts.get(parent).map_or(&[][..], |t| &t.messages[..]) {
            let Message::Assistant(a) = m else { continue };
            for p in &a.parts {
                if let Part::Tool { id, name, input, metadata, .. } = p
                    && matches!(name.as_str(), "task" | "subagent")
                    && model::subagent_session(input, metadata) == Some(child)
                {
                    let began = model::tool_ran(metadata).or_else(|| self.agent_rows.called.get(id).copied());
                    out.push((id.as_str(), began));
                }
            }
        }
        out
    }

    /// Call `call`'s window of `child`'s work, or None when it is the child's only call.
    pub(crate) fn window(&self, parent: &str, call: &str, child: &str) -> Option<Window> {
        let calls = self.calls_to(parent, child);
        if calls.len() < 2 {
            return None;
        }
        let k = calls.iter().position(|c| c.0 == call)?;
        let created = self.session(child).map(|i| i.created).filter(|c| *c > 0);
        let from = if k == 0 { calls[0].1.or(created) } else { calls[k].1 };
        Some(Window { from, next: calls.get(k + 1).map(|c| c.1) })
    }

    /// The run of a call a later one followed: done or failed when the child ended before the
    /// next call began, else Steered, its clock stopped there.
    pub(crate) fn earlier_run(&self, child: &str, w: &Window, next: &Option<i64>, status: &ToolStatus) -> Run {
        let to = *next;
        let inside = |t: i64| w.from.is_none_or(|f| t >= f) && to.is_none_or(|n| t < n);
        let ended = self.agent_rows.ends.get(child).and_then(|e| e.iter().find(|e| inside(e.0)).copied()).or_else(|| self.finished(child, w.from, to));
        let (state, end) = match ended {
            Some((at, true)) => (State::Failed, Some(at)),
            Some((at, false)) => (State::Done, Some(at)),
            None if *status == ToolStatus::Error => (State::Failed, None),
            None => (State::Steered, to),
        };
        let (steps, taken) = match self.steps_in(child, w.from, end.or(to)).filter(|s| !s.0.is_empty()) {
            Some(s) => s,
            None => (self.steps_of(child, w.from, end.or(to)).find_map(|a| a.parts.iter().rev().find_map(tool_step)).into_iter().collect(), 0),
        };
        let ms = w.from.zip(end).map(|(f, e)| (e - f).max(0));
        let result = (state == State::Done).then(|| self.answer(child, w.from, to)).flatten();
        Run { state, steps, taken, ms, result, error: None }
    }

    /// When the child's transcript shows it finished between `from` and `to`: its last step
    /// there answered in words and called nothing.
    fn finished(&self, child: &str, from: Option<i64>, to: Option<i64>) -> Option<(i64, bool)> {
        let last = self.steps_of(child, from, to).next()?;
        let answered = last.parts.iter().any(|p| matches!(p, Part::Text { text } if !text.trim().is_empty()));
        let called = last.parts.iter().any(|p| matches!(p, Part::Tool { .. }));
        let at = last.completed.filter(|_| answered && !called && (from.is_some() || to.is_some()))?;
        Some((at, last.error.is_some()))
    }
}

fn tool_step(p: &Part) -> Option<String> {
    match p {
        Part::Tool { name, input, .. } => Some(super::step_of(name, input)),
        _ => None,
    }
}
