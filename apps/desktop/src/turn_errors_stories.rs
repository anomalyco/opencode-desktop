//! The storybook's Errors, Retry and Notices stories (Timeline › …), on the fixture's session:
//! a prompt whose step fails with each kind of error Desktop tells apart, a step the server is
//! retrying with its countdown, and each notice. R replays; on Retry, N schedules the next
//! attempt and F fails it.

use rune::{Play, Tx};
use serde_json::{Value, json};

use crate::App;
use crate::stories::{Story, Trigger, emit, replay, sid};

pub const ERROR_VARIANTS: &[(&str, usize)] =
    &[("Error", 0), ("Provider JSON", 1), ("Free limit", 2), ("Go limit", 3), ("Budget", 4), ("Model budget", 5), ("No reset time", 6)];
pub const RETRY_VARIANTS: &[(&str, usize)] = &[("Rate limited", 0), ("Gemini", 1), ("Retrying now", 2)];
pub const NOTICE_VARIANTS: &[(&str, usize)] =
    &[("All", 0), ("Agent", 1), ("Model", 2), ("Variant", 3), ("Moved", 4), ("Instructions", 5), ("Restart", 6), ("Background", 7)];

const ERROR_KEYS: &[Trigger] = &[replay(error_open)];
const RETRY_KEYS: &[Trigger] = &[
    replay(retry_open),
    Trigger { key: 'n', label: "Schedule the next attempt", run: |s, tx| {
        tx.clear();
        let attempt = s.transcripts.get(&sid(s)).and_then(|t| t.retrying()).map_or(1, |r| r.attempt + 1);
        schedule(s, attempt, 6_000, RATE, tx);
    } },
    Trigger { key: 'f', label: "Fail it", run: |s, tx| {
        tx.clear();
        fail(s, json!({ "type": "provider", "message": RATE }), tx);
    } },
];
const NOTICE_KEYS: &[Trigger] = &[replay(notice_open)];

const STEP: &str = "msg_storyerrorstep";
const RATE: &str = "Rate limit exceeded: Too many requests, please slow down.";

fn prompt(s: &mut App, text: &str, tx: &mut Tx) {
    emit(s, "session.prompted", json!({ "messageID": "msg_storyerrorprompt", "text": text }), tx);
    emit(s, "session.step.started", json!({ "assistantMessageID": STEP }), tx);
}

fn fail(s: &mut App, error: Value, tx: &mut Tx) {
    emit(s, "session.step.failed", json!({ "assistantMessageID": STEP, "error": error }), tx);
}

fn error_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.3, |s, tx| {
        prompt(s, "Tidy up the reconnect backoff", tx);
        let resets = "2026-11-01T07:00:00.000Z";
        let body = format!(r#"{{"error":{{"type":"budget_exceeded","message":"Budget reached","metadata":{{"resetsAt":"{resets}"}}}}}}"#);
        let error = match s.variant {
            0 => json!({ "type": "provider", "message": "Bad Request: The model `claude-opus-9` does not exist or you do not have access to it." }),
            1 => json!({ "type": "provider", "message": r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"# }),
            2 => json!({ "type": "provider.free-limit", "message": "Free limit reached", "retryAfterMs": 4 * 3_600_000 }),
            3 => json!({ "type": "provider.go-limit", "message": "Go limit reached", "retryAfterMs": 3 * 86_400_000 }),
            4 => json!({ "type": "provider.budget-limit", "message": "Budget reached", "response": { "body": body } }),
            5 => json!({ "type": "provider.model-budget-limit", "message": "Budget reached", "response": { "body": body } }),
            _ => json!({ "type": "provider.free-limit", "message": "Free limit reached" }),
        };
        tx.after(0.4, move |s, tx| fail(s, error.clone(), tx));
    });
}

fn schedule(s: &mut App, attempt: u32, wait_ms: i64, message: &str, tx: &mut Tx) {
    let at = s.now_ms + wait_ms;
    emit(s, "session.retry.scheduled", json!({ "assistantMessageID": STEP, "attempt": attempt, "at": at, "error": { "type": "provider", "message": message } }), tx);
}

fn retry_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.3, |s, tx| {
        prompt(s, "Run the migration again", tx);
        let (wait, message) = match s.variant {
            1 => (12_000, "You exceeded your current quota for gemini-3-pro, please check your plan and billing details."),
            2 => (0, RATE),
            _ => (8_000, RATE),
        };
        schedule(s, 2, wait, message, tx);
    });
}

fn notice(s: &mut App, n: usize, ty: &str, data: Value, tx: &mut Tx) {
    let mut data = data;
    data["eventID"] = format!("evt_storynotice{n}").into();
    emit(s, ty, data, tx);
}

fn notice_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.3, |s, tx| {
        let v = s.variant;
        let model = |id: &str, variant: &str| json!({ "id": id, "providerID": "anthropic", "variant": variant });
        if v == 7 {
            return background(s, tx);
        }
        let all: [(&str, Value); 6] = [
            ("session.agent.selected", json!({ "agent": "plan", "previous": "build" })),
            ("session.model.selected", json!({ "model": model("claude-sonnet-4-5", "high"), "previous": model("claude-opus-4-5", "") })),
            ("session.model.selected", json!({ "model": model("claude-sonnet-4-5", "max"), "previous": model("claude-sonnet-4-5", "high") })),
            ("session.moved", json!({ "location": { "directory": "/Users/alex/code/worktrees/opencode/reconnect" } })),
            ("session.instructions.updated", json!({ "text": "…", "delta": { "AGENTS.md": {}, "packages/app/AGENTS.md": {} } })),
            ("session.synthetic", json!({ "text": "", "description": "Continuing after restart", "metadata": { "notice": "restart" } })),
        ];
        for (n, (ty, data)) in all.into_iter().enumerate() {
            if v == 0 || v == n + 1 {
                notice(s, n, ty, data, tx);
            }
        }
    });
}

const HEREDOC: &str = "\n  python3 - <<'PY'\nimport subprocess\nfor t,h,r,why in [('b5925','a63fc96ca','ses_e342da27','Re-reviewed moved tests'),\n                   ('b6600','9c1d2e0f','ses_77ab01c3','Handoff repaired')]:\n    print(t, h, r, why)\nPY";

/// Background work coming back: a shell that finished, one that failed, one with a command too
/// long for the line, a subagent, and two heredocs.
fn background(s: &mut App, tx: &mut Tx) {
    let shell = |state: &str, command: &str, extra: Value| {
        let mut metadata = json!({ "source": "shell", "state": state, "shellID": "sh_story" });
        if let (Some(m), Some(e)) = (metadata.as_object_mut(), extra.as_object()) {
            m.extend(e.clone());
        }
        json!({ "text": "", "description": command, "metadata": metadata })
    };
    let long = "bun guide/capture.ts --all-steps --write guide.json --check-against-compiler --verbose 2>&1 | tail -6";
    let all = [
        shell("completed", "bun guide/capture.ts 2>&1 | tail -6", json!({ "exit": 0 })),
        shell("completed", "cargo test -p session-view", json!({ "exit": 101 })),
        shell("completed", long, json!({ "exit": 0 })),
        json!({ "text": "", "description": "Find where notices are drawn", "metadata": { "source": "subagent", "state": "completed", "agent": "explore" } }),
        // A heredoc, and one whose shell call says what it does.
        shell("completed", HEREDOC, json!({ "exit": 0, "shellID": "sh_heredoc" })),
        shell("completed", HEREDOC, json!({ "exit": 0, "shellID": "sh_named" })),
    ];
    let call = json!({ "type": "tool", "id": "prt_story_named", "name": "shell", "state": {
        "status": "completed", "title": "Check which overnight tasks landed",
        "input": { "command": HEREDOC, "description": "Check which overnight tasks landed", "background": true },
        "metadata": { "shellID": "sh_named" },
    } });
    let step = opencode_kit::model::parse_messages(&json!({ "data": [
        { "id": "msg_story_named", "type": "assistant", "time": { "completed": 2 }, "content": [call] },
    ] }));
    let id = sid(s);
    if let Some(t) = s.transcripts.get_mut(&id) {
        t.messages.extend(step);
    }
    for (n, data) in all.into_iter().enumerate() {
        notice(s, 10 + n, "session.synthetic", data, tx);
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::session("Timeline/Errors", ERROR_KEYS).knobs(crate::stories::ROW_KNOBS).variants(ERROR_VARIANTS),
    Story::session("Timeline/Retry", RETRY_KEYS).knobs(crate::stories::ROW_KNOBS).variants(RETRY_VARIANTS),
    Story::session("Timeline/Notices", NOTICE_KEYS).knobs(crate::stories::ROW_KNOBS).variants(NOTICE_VARIANTS),
]);
