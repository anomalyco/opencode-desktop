//! The storybook's Subagent rows story (Timeline › Subagent rows): agents start one after
//! another and work through their steps; the first finishes, the newest fails, and the rest
//! finish later. S starts another, F finishes the oldest, E fails the newest, R replays.

use opencode_kit::model::{Assistant, Message, Part, SessionInfo, ToolStatus, Transcript};
use rune::{Play, Tx};
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

/// How many agents the story starts, three first. Under Working, one agent runs in a turn that
/// is still working, so Working sits under its card.
pub const VARIANTS: &[(&str, usize)] = &[("3 agents", 1), ("1 agent", 0), ("6 agents", 2), ("Under Working", 3)];
const COUNTS: [usize; 4] = [1, 3, 6, 1];

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 's', label: "Start another", run: |s, tx| spawn(s, tx) },
    Trigger { key: 'f', label: "Finish the oldest", run: |s, tx| finish_oldest(s, tx) },
    Trigger { key: 'e', label: "Fail the newest", run: |s, tx| fail_newest(s, tx) },
];

/// Seconds between one agent's steps.
const STEP: f64 = 0.9;

/// Each agent's task and the tool calls it works through, as (tool, input field, value).
type Agent = (&'static str, &'static str, &'static [(&'static str, &'static str, &'static str)]);
const AGENTS: &[Agent] = &[
    ("general", "Review the reconnect change", &[("read", "filePath", "src/net/backoff.ts"), ("read", "filePath", "src/net/backoff.test.ts"), ("shell", "description", "Run the net tests"), ("grep", "pattern", "retryBudget")]),
    ("explore", "Find every caller of reconnect()", &[("grep", "pattern", "reconnect("), ("read", "filePath", "src/net/stream.ts"), ("read", "filePath", "src/app/session/live.ts"), ("glob", "pattern", "src/**/*.test.ts")]),
    ("general", "Write tests for jittered backoff with a fake clock and a dropped socket", &[("read", "filePath", "test/utils/clock.ts"), ("write", "filePath", "src/net/jitter.test.ts"), ("shell", "description", "bun test src/net/jitter.test.ts"), ("edit", "filePath", "src/net/jitter.test.ts")]),
    ("explore", "Map how session titles reach the sidebar", &[("grep", "pattern", "session.renamed"), ("read", "filePath", "src/sidebar/list.tsx"), ("read", "filePath", "src/sync/sessions.ts")]),
    ("general", "Profile the timeline's scroll path", &[("shell", "description", "Record a scroll trace"), ("read", "filePath", "src/timeline/virtualizer.tsx"), ("grep", "pattern", "measureRow")]),
    ("explore", "Check the event stream for frames dropped under load", &[("read", "filePath", "src/net/events.ts"), ("grep", "pattern", "onFrame"), ("webfetch", "url", "https://example.com/sse-spec")]),
];

/// What each agent answers when it finishes.
const ANSWERS: &[&str] = &[
    "Looks right; one nit on the retry cap in backoff.ts",
    "7 callers; two skip the backoff",
    "Added 6 tests, all passing",
    "Titles arrive by session.updated, then the list re-sorts",
    "measureRow runs twice a frame while resizing",
    "No drops below 2k events a second",
];

fn child(k: usize) -> String {
    format!("ses_story_agent_{k:02}")
}

/// The story's agents so far, in the order they started.
fn started(s: &App) -> Vec<String> {
    (0..).map(child).take_while(|c| s.session(c).is_some()).collect()
}

/// The agents start a beat apart and step along. At 4.5 s the oldest finishes and the newest
/// fails, so one of each shows from 5.4 s until the rest finish at 9 s.
fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let n = COUNTS[s.variant.min(3)];
    let working = s.variant == 3;
    if working {
        let id = sid(s);
        if let Some(Message::Assistant(a)) = s.transcripts.get_mut(&id).and_then(|t| t.messages.last_mut()) {
            a.completed = None;
        }
        s.running.insert(id);
    }
    for k in 0..n {
        tx.after(0.3 + k as f64 * 0.25, |s, tx| spawn(s, tx));
    }
    let mut t = 0.3 + STEP;
    while t < 20.0 {
        tx.after(t, |s, tx| advance(s, tx));
        t += STEP;
    }
    if working {
        return;
    }
    tx.after(4.5, |s, tx| finish_oldest(s, tx));
    if n > 1 {
        tx.after(5.4, |s, tx| fail_newest(s, tx));
    }
    for k in 0..n.saturating_sub(2) {
        tx.after(9.0 + k as f64 * 0.6, |s, tx| finish_oldest(s, tx));
    }
}

/// The last answer in the open session, where its calls go.
fn last_answer<'a>(s: &'a mut App, parent: &str) -> Option<&'a mut Assistant> {
    let t = s.transcripts.get_mut(parent)?;
    t.messages.iter_mut().rev().find_map(|m| match m {
        Message::Assistant(a) => Some(a),
        _ => None,
    })
}

/// The open session starts writing a call to the next agent: "Delegating agent…" until
/// [`spawn`] starts it.
pub fn delegate(s: &mut App, tx: &mut Tx) {
    let parent = sid(s);
    let k = started(s).len();
    let call = Part::Tool {
        id: format!("call_story_agent_{k:02}"),
        name: "subagent".into(),
        status: ToolStatus::Pending,
        title: String::new(),
        input: json!({}),
        files: Vec::new(),
        metadata: json!({}),
    };
    if let Some(a) = last_answer(s, &parent) {
        a.parts.push(call);
    }
    tx.wake_after(std::time::Duration::ZERO);
}

/// Whether a call [`delegate`] started is still being written.
pub fn delegating(s: &mut App) -> bool {
    let parent = sid(s);
    last_answer(s, &parent).is_some_and(|a| a.parts.iter().any(|p| matches!(p, Part::Tool { status: ToolStatus::Pending, .. })))
}

/// The open session calls another agent in the background: its card joins the timeline (or the
/// call [`delegate`] started is written), and its session starts running.
pub fn spawn(s: &mut App, tx: &mut Tx) {
    let parent = sid(s);
    let k = started(s).len();
    let (agent, title, _) = AGENTS[k % AGENTS.len()];
    let id = child(k);
    let call_id = format!("call_story_agent_{k:02}");
    let from = s.session(&parent).cloned().unwrap_or_default();
    s.sessions.push(SessionInfo {
        id: id.clone(),
        parent: Some(parent.clone()),
        project: from.project,
        directory: from.directory,
        title: title.into(),
        agent: agent.into(),
        created: s.now_ms,
        updated: s.now_ms,
        ..Default::default()
    });
    let call = Part::Tool {
        id: call_id.clone(),
        name: "subagent".into(),
        status: ToolStatus::Completed,
        title: title.into(),
        input: json!({ "agent": agent, "description": title, "background": true }),
        files: Vec::new(),
        metadata: json!({ "sessionID": id, "status": "running" }),
    };
    if let Some(a) = last_answer(s, &parent) {
        match a.parts.iter_mut().find(|p| matches!(p, Part::Tool { id, .. } if *id == call_id)) {
            Some(written) => *written = call,
            None => a.parts.push(call),
        }
    }
    emit(s, "session.execution.started", json!({ "sessionID": id }), tx);
}

/// Each running agent moves on to its next step.
pub fn advance(s: &mut App, tx: &mut Tx) {
    for (k, id) in started(s).into_iter().enumerate() {
        if !s.running.contains(&id) {
            continue;
        }
        let steps = AGENTS[k % AGENTS.len()].2;
        let since = s.session(&id).map_or(0, |i| s.now_ms - i.created);
        let (name, field, value) = steps[(since as f64 / 1000.0 / STEP) as usize % steps.len()];
        let mut input = json!({});
        input[field] = value.into();
        emit(s, "session.tool.called", json!({ "sessionID": id, "name": name, "input": input }), tx);
    }
}

/// The oldest running agent answers and finishes.
pub fn finish_oldest(s: &mut App, tx: &mut Tx) {
    let all = started(s);
    if let Some(k) = all.iter().position(|c| s.running.contains(c)) {
        let id = all[k].clone();
        let answer = Assistant {
            id: format!("msg_story_answer_{k:02}"),
            completed: Some(s.now_ms),
            parts: vec![Part::Text { text: ANSWERS[k % ANSWERS.len()].into() }],
            ..Default::default()
        };
        s.transcripts.entry(id.clone()).or_insert_with(|| Transcript { session: id.clone(), messages: Vec::new() }).messages.push(Message::Assistant(answer));
        emit(s, "session.execution.succeeded", json!({ "sessionID": id }), tx);
    }
}

/// The newest running agent stops on an error.
pub fn fail_newest(s: &mut App, tx: &mut Tx) {
    if let Some(id) = started(s).into_iter().rev().find(|c| s.running.contains(c)) {
        emit(s, "session.execution.failed", json!({ "sessionID": id, "error": { "message": "The provider is rate limiting this key" } }), tx);
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Subagent rows", KEYS).variants(VARIANTS),
]);
