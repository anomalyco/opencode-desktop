//! The storybook's Subagent rows story (Timeline › Subagent rows): agents start one after
//! another and work through their steps; the first finishes, the newest fails, and the rest
//! finish later. S starts another, F finishes the oldest, E fails the newest, R replays.

use opencode_kit::model::{Assistant, Message, Part, SessionInfo, ToolStatus, Transcript};
use rune::Tx;
use rune_knobs::{Action, Group, Knob};
use serde_json::json;

use crate::App;
use crate::stories::{emit, replay, sid};

/// How many agents the story starts.
pub const COUNT: Knob = Knob::choice("Agents", &["1", "3", "6"], 1);
const COUNTS: [usize; 3] = [1, 3, 6];

pub const KNOBS: &[Group] = &[Group::new("Subagent rows", "stage", &[COUNT])];
pub const KEYS: &[Action<App>] = &[
    Action { cap: "R", label: "Replay", run: replay },
    Action { cap: "S", label: "Start another", run: spawn },
    Action { cap: "F", label: "Finish the oldest", run: finish_oldest },
    Action { cap: "E", label: "Fail the newest", run: fail_newest },
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

/// A snapshot scene: `count` agents (the Agents knob's index), light or dark.
pub fn scene(s: &mut App, count: usize, dark: bool) {
    s.knobs.set(&COUNT, count as f32);
    crate::stories::scene(s, "Subagent rows");
    s.settings.scheme = if dark { crate::settings::Scheme::Dark } else { crate::settings::Scheme::Light };
}

/// The agents start a beat apart and step along. At 4.5 s the oldest finishes and the newest
/// fails, so one of each shows from 5.4 s until the rest finish at 9 s.
pub fn open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    let n = COUNTS[(s.knobs.get(&COUNT) as usize).min(2)];
    for k in 0..n {
        s.play.after(0.3 + k as f64 * 0.25, spawn, tx);
    }
    let mut t = 0.3 + STEP;
    while t < 20.0 {
        s.play.after(t, advance, tx);
        t += STEP;
    }
    s.play.after(4.5, finish_oldest, tx);
    if n > 1 {
        s.play.after(5.4, fail_newest, tx);
    }
    for k in 0..n.saturating_sub(2) {
        s.play.after(9.0 + k as f64 * 0.6, finish_oldest, tx);
    }
}

/// The open session calls another agent in the background: its card joins the timeline, its
/// session starts running.
pub fn spawn(s: &mut App, tx: &mut Tx) {
    let parent = sid(s);
    let k = started(s).len();
    let (agent, title, _) = AGENTS[k % AGENTS.len()];
    let id = child(k);
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
        id: format!("call_story_agent_{k:02}"),
        name: "subagent".into(),
        status: ToolStatus::Completed,
        title: title.into(),
        input: json!({ "agent": agent, "description": title, "background": true }),
        files: Vec::new(),
        metadata: json!({ "sessionID": id, "status": "running" }),
    };
    if let Some(Message::Assistant(a)) = s.transcripts.get_mut(&parent).and_then(|t| t.messages.iter_mut().rev().find(|m| matches!(m, Message::Assistant(_)))) {
        a.parts.push(call);
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
