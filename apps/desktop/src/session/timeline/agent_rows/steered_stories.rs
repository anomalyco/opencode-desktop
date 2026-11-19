//! The storybook's Steered subagent story (Timeline › Steered subagent): the session starts an
//! agent, then sends it new instructions twice while it works, each a call of its own naming the
//! same session. Each card keeps its own call's share: the first two stop as Steered where the
//! next took over, and only the newest runs on and finishes. S steers it again, F finishes it,
//! R replays.

use opencode_kit::model::{Assistant, Message, Part, SessionInfo, ToolStatus, Transcript};
use rune::{Play, Tx};
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 's', label: "Steer it again", run: |s, tx| steer(s, tx) },
    Trigger { key: 'f', label: "Finish it", run: |s, tx| finish(s, tx) },
];

/// The agent every call names.
pub const CHILD: &str = "ses_story_steered";

/// Seconds between the agent's steps.
const STEP: f64 = 0.9;

/// Each call's task, what the session says before it, and the steps the agent takes for it.
type Call = (&'static str, &'static str, &'static [(&'static str, &'static str, &'static str)]);
const CALLS: &[Call] = &[
    ("Extract Rune desktop styles", "", &[("glob", "pattern", "apps/desktop/src/**/*.rs"), ("read", "filePath", "palette.rs"), ("grep", "pattern", "radius(")]),
    ("Extract Seer app styles", "Seer's styles matter more here, so I'll point it there instead.", &[("read", "filePath", "theme.rs"), ("read", "filePath", "space.rs"), ("grep", "pattern", "fill(")]),
    ("Stop styles extraction", "That's enough to go on; I'll ask it to wrap up.", &[("write", "filePath", "styles.md")]),
];

const ANSWER: &str = "Seer keeps a 4pt space scale and two tones per surface; notes in styles.md";

/// Starts the agent, steers it at 2.6 s and 4.9 s, and it finishes at 7.6 s.
pub fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    for t in [0.3, 2.6, 4.9] {
        tx.after(t, |s, tx| call(s, tx));
    }
    let mut t = 0.3 + STEP;
    while t < 7.5 {
        tx.after(t, |s, tx| advance(s, tx));
        t += STEP;
    }
    tx.after(7.6, |s, tx| finish(s, tx));
}

/// The open session's last answer, where its calls go.
fn last_answer<'a>(s: &'a mut App, parent: &str) -> Option<&'a mut Assistant> {
    s.transcripts.get_mut(parent)?.messages.iter_mut().rev().find_map(|m| match m {
        Message::Assistant(a) => Some(a),
        _ => None,
    })
}

/// The calls made so far.
fn calls(s: &App) -> usize {
    s.transcripts.get(&sid(s)).map_or(0, |t| {
        t.messages.iter().filter_map(|m| match m {
            Message::Assistant(a) => Some(a.parts.iter().filter(|p| matches!(p, Part::Tool { id, .. } if id.starts_with("call_story_steer_"))).count()),
            _ => None,
        }).sum()
    })
}

/// The session calls the agent: the first call starts it, each later one steers it.
pub fn call(s: &mut App, tx: &mut Tx) {
    let parent = sid(s);
    let k = calls(s);
    let (title, said, _) = CALLS[k.min(CALLS.len() - 1)];
    if k == 0 {
        let from = s.session(&parent).cloned().unwrap_or_default();
        s.sessions.push(SessionInfo {
            id: CHILD.into(),
            parent: Some(parent.clone()),
            project: from.project,
            directory: from.directory,
            title: CALLS[0].0.into(),
            agent: "explore".into(),
            created: *s.now_ms,
            updated: *s.now_ms,
            ..Default::default()
        });
    }
    let id = format!("call_story_steer_{k}");
    let mut input = json!({ "agent": "explore", "description": title, "prompt": title, "background": true });
    if k > 0 {
        input["sessionID"] = CHILD.into();
    }
    let part = Part::Tool { id: id.clone(), name: "subagent".into(), status: ToolStatus::Completed, title: title.into(), input, files: Vec::new(), metadata: json!({ "sessionID": CHILD, "status": "running" }) };
    if let Some(a) = last_answer(s, &parent) {
        if !said.is_empty() {
            a.parts.push(Part::Text { text: said.into() });
        }
        a.parts.push(part);
    }
    s.timeline_fx.agent_rows.called.insert(id, *s.now_ms);
    if k == 0 {
        emit(s, "session.execution.started", json!({ "sessionID": CHILD }), tx);
    } else {
        emit(s, "session.reasoning.started", json!({ "sessionID": CHILD }), tx);
    }
}

/// The agent takes the next step of the newest call's work.
fn advance(s: &mut App, tx: &mut Tx) {
    if !s.running.contains(CHILD) {
        return;
    }
    let k = calls(s).saturating_sub(1);
    let steps = CALLS[k.min(CALLS.len() - 1)].2;
    let since = s.timeline_fx.agent_rows.called.get(&format!("call_story_steer_{k}")).map_or(0, |at| *s.now_ms - at);
    let (name, field, value) = steps[(since as f64 / 1000.0 / STEP) as usize % steps.len()];
    let mut input = json!({});
    input[field] = value.into();
    emit(s, "session.tool.called", json!({ "sessionID": CHILD, "name": name, "input": input }), tx);
}

/// Sends it new instructions again, while it works.
fn steer(s: &mut App, tx: &mut Tx) {
    if s.running.contains(CHILD) {
        call(s, tx);
    }
}

/// The agent answers and finishes.
pub fn finish(s: &mut App, tx: &mut Tx) {
    if !s.running.contains(CHILD) {
        return;
    }
    let answer = Assistant { id: format!("msg_story_steered_{}", *s.now_ms), created: *s.now_ms - 400, completed: Some(*s.now_ms), parts: vec![Part::Text { text: ANSWER.into() }], ..Default::default() };
    s.transcripts.entry(CHILD.into()).or_insert_with(|| Transcript { session: CHILD.into(), messages: Vec::new() }).messages.push(Message::Assistant(answer));
    emit(s, "session.execution.succeeded", json!({ "sessionID": CHILD }), tx);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Steered subagent", KEYS),
]);
