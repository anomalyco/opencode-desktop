//! A stretch of work that called on subagents, gathered into one row (Rune-only).
//!
//! Official gives every subagent event a row of its own (`groupContent`'s `subagent`,
//! anomalyco/opencode#64017), so a long turn reads as "Completed General Agent …" rows
//! interleaved with its "Ran N commands" groups. Here, a deliberate divergence agreed by the team,
//! the groups and subagent events one after another with nothing else between them (a reply, a
//! thought or call on its own row) join one group: its commands' steps as the group's, and its
//! agents beside them, each once, by its session, named by the call that started it and in the
//! latest state the turn knows (`activity::agents` draws the line). A stretch without a subagent
//! keeps its rows as they were.

use std::collections::{HashMap, HashSet};

use super::{Block, Step};
use crate::activity::{events, label::step_event};

/// A subagent a stretch called on: the turn's first event about it, which names its task (a
/// follow-up's words name the follow-up), and its latest, which says where it is.
#[derive(Clone, Debug)]
pub struct Called<'a> {
    pub first: Step<'a>,
    pub latest: Step<'a>,
}

/// Who subagent event `st` is about: its session, else its own call or report. Nothing for a
/// call still being written, which has nothing to show yet.
pub fn who(st: &Step) -> Option<String> {
    let e = step_event(st).filter(|e| e.kind == events::Kind::Subagent)?;
    if let Some((_, id)) = events::work_key(&e) {
        return Some(format!("s:{id}"));
    }
    match st {
        Step::Tool { id, .. } => Some(format!("c:{id}")),
        Step::Notice(n) => Some(format!("n:{}", n.id)),
        Step::Thought { .. } | Step::Text { .. } | Step::Change { .. } => None,
    }
}

/// `out` with each stretch of groups and subagent events that holds a subagent event as one
/// group, keyed by its first row's key (a subagent's `a-` key as a group's, `c-a-`), so the row
/// stays put while the stretch grows and its open state holds.
pub fn merge(out: Vec<Block<'_>>) -> Vec<Block<'_>> {
    if !out.iter().any(|b| matches!(b, Block::Subagent { .. })) {
        return out;
    }
    // Each agent's first and latest event in the turn: a stretch shows where the agent is now,
    // not where it was when the stretch called on it.
    let mut called: HashMap<String, Called<'_>> = HashMap::new();
    for b in &out {
        if let Block::Subagent { step, .. } = b
            && let Some(w) = who(step)
        {
            called.entry(w).and_modify(|c| c.latest = step.clone()).or_insert_with(|| Called { first: step.clone(), latest: step.clone() });
        }
    }
    let mut merged = Vec::with_capacity(out.len());
    let mut run = Vec::new();
    for b in out {
        if matches!(b, Block::Group { .. } | Block::Subagent { .. }) {
            run.push(b);
            continue;
        }
        close(&mut merged, &mut run, &called);
        merged.push(b);
    }
    close(&mut merged, &mut run, &called);
    merged
}

/// Ends stretch `run`: one group where it called on a subagent, else its rows as they are.
fn close<'a>(merged: &mut Vec<Block<'a>>, run: &mut Vec<Block<'a>>, called: &HashMap<String, Called<'a>>) {
    if !run.iter().any(|b| matches!(b, Block::Subagent { .. })) {
        merged.append(run);
        return;
    }
    let (mut key, mut steps, mut at, mut agents, mut seen) = (None, Vec::new(), Vec::new(), Vec::new(), HashSet::new());
    for b in run.drain(..) {
        match b {
            Block::Group { key: k, steps: s, at: a, .. } => {
                key.get_or_insert(k);
                steps.extend(s);
                at.extend(a);
            }
            Block::Subagent { key: k, step } => {
                key.get_or_insert_with(|| format!("c-{k}"));
                let Some(w) = who(&step) else { continue };
                if seen.insert(w.clone()) {
                    agents.push(called.get(&w).cloned().unwrap_or_else(|| Called { first: step.clone(), latest: step }));
                }
            }
            _ => {}
        }
    }
    merged.push(Block::Group { key: key.unwrap_or_default(), steps, at, agents });
}
