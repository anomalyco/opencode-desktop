//! Replies with ```` ```ui ```` outlines in them, for the fixture and the storybook.
//!
//! `--scene genui` opens the fixture session with a finished reply that mixes prose, a before
//! and after outline, code and Mermaid. `--scene genui-stream` streams a plan into it. The
//! story (Timeline › Generative UI) streams either one again on R; F finishes it.

use opencode_kit::model::{Assistant, Message, Part};
use rune::Play;
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, replay, sid};

/// Kit's question, above both replies.
pub const ASK: &str = "Why does the Offline banner flash on hotel Wi-Fi, and what's the fix?";

/// A finished reply: prose, a before and after, code, a diagram and the checks.
pub const MIXED: &str = r#"It flashes because the sidebar shows **Offline** on the first failed retry, and on a hotel network the first retry fails almost every time. Waiting for a second failure fixes it:

```ui
row
  screen Today
    title Reconnecting
    item {risk} Offline · after 1 failed retry
    item {risk} Banner flashes on every short drop
  arrow
  screen After
    title Reconnecting
    item {done} Offline · after 2 failed retries
    item {done} Drops under 1 s stay quiet
```

The rule lives in one place, `shouldShowOffline` in `sidebar/status.ts`:

```ts
export function shouldShowOffline(attempt: number, downMs: number) {
  return attempt >= 2 && downMs >= 1000
}
```

```mermaid
flowchart LR
  R1[Retry at once] -->|fails| R2[Back off 1 s]
  R2 -->|fails| Off[Offline]
  R1 -->|ok| On[Online]
```

```ui
box Checks
  kv Unit tests: {done} 14 passed · bun test net/
  kv Fake clock: {done} first retry is immediate
  kv Real timers: {working} slow end-to-end test running
```

I'll open the PR once the slow test passes; nothing changes on the server.
"#;

/// A reply that streams: a plan as steps, then the change people will see.
pub const PLAN: &str = r#"Here's the reconnect work in the order I'll land it:

```ui
box Plan
  item {done} Retry at once after a drop · backoff.ts
  item {done} Back off from 1 s, capped at 30 s, ±20% jitter
  item {working} Offline only after the second failure · sidebar
  item {proposal} Reconnect button after 3 hours offline
  item Queued prompts keep their order · inbox.ts
```

What changes for people is the sidebar row:

```ui
row
  box Before
    text Says **Offline** the moment the socket drops, then flips back.
  arrow
  box After
    text Says *Reconnecting…* and goes Offline only after two failed tries.
```

I'll start with the banner, since it's the part people see.
"#;

const ASK_ID: &str = "msg_zzgenui0";
const MIXED_ID: &str = "msg_zzgenui1";
const PLAN_ID: &str = "msg_zzgenui2";

/// The ask and the finished mixed reply, at the end of session `id`.
pub fn add_mixed(app: &mut App, id: &str) {
    let Some(t) = app.transcripts.get_mut(id) else { return };
    t.messages.push(Message::User { id: ASK_ID.into(), text: ASK.into() });
    let parts = vec![Part::Text { text: MIXED.into() }];
    t.messages.push(Message::Assistant(Assistant { id: MIXED_ID.into(), completed: Some(1), parts, ..Default::default() }));
}

/// Streams `text` as reply `reply` into the open session, a few words at a time, `step` seconds
/// apart, as a model's deltas arrive.
pub(crate) fn stream(s: &mut App, reply: &str, text: &str, step: f64) {
    let id = sid(s);
    s.running.insert(id.clone());
    let now = s.clock;
    s.schedule(now, "session.execution.started", json!({ "sessionID": id, "assistantMessageID": reply }));
    s.schedule(now, "session.step.started", json!({ "sessionID": id, "assistantMessageID": reply, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }));
    s.schedule(now, "session.text.started", json!({ "sessionID": id, "assistantMessageID": reply, "ordinal": 0 }));
    let mut at = s.clock + 0.3;
    for chunk in chunks(text) {
        s.schedule(at, "session.text.delta", json!({ "sessionID": id, "assistantMessageID": reply, "ordinal": 0, "delta": chunk }));
        at += step;
    }
    s.schedule(at, "session.text.ended", json!({ "sessionID": id, "assistantMessageID": reply, "ordinal": 0, "text": text }));
    s.schedule(at + 0.1, "session.step.ended", json!({ "sessionID": id, "assistantMessageID": reply, "finish": "stop", "cost": 0.04, "tokens": { "input": 6, "output": 320, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }));
    s.schedule(at + 0.1, "session.execution.succeeded", json!({ "sessionID": id, "assistantMessageID": reply }));
    s.wake_at_playback();
}

/// A model's deltas: two or three words at a time, a newline ending its own delta, so fence
/// lines arrive in pieces the way they do from a real stream.
pub fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut words = 0;
    for piece in text.split_inclusive([' ', '\n']) {
        cur.push_str(piece);
        words += 1;
        if piece.ends_with('\n') || words == 3 {
            out.push(std::mem::take(&mut cur));
            words = 0;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn prompt(s: &mut App) {
    let id = sid(s);
    if let Some(t) = s.transcripts.get_mut(&id) {
        t.messages.push(Message::User { id: ASK_ID.into(), text: ASK.into() });
    }
}

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    prompt(s);
    match s.variant {
        1 => stream(s, MIXED_ID, MIXED, 0.06),
        _ => stream(s, PLAN_ID, PLAN, 0.09),
    }
}

pub(crate) fn finish(s: &mut App, _: &mut Play<App>) {
    while let Some((_, ty, data)) = s.playback.pop_front() {
        s.apply_event(&ty, &data);
    }
}

pub const KEYS: &[Trigger] = &[replay(open), Trigger { key: 'f', label: "Finish the stream", run: finish }];
pub const VARIANTS: &[(&str, usize)] = &[("Plan", 0), ("Mixed reply", 1)];

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Generative UI", KEYS).knobs(crate::stories::ROW_KNOBS).variants(VARIANTS),
]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("genui", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = s.route.clone() {
            add_mixed(s, &id);
        }
    }),
    ("genui-stream", |s| {
        crate::open_scene(s);
        s.finish_stream();
        prompt(s);
        stream(s, PLAN_ID, PLAN, 0.09);
    }),
]);
