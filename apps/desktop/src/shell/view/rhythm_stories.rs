//! The storybook's Rhythm story (`Timeline/Rhythm`): one reply with every neighbour the
//! timeline's vertical rhythm spaces (`session_view::timeline::rhythm`): paragraphs around a code
//! block, background work coming back, a thought, subagent capsules, and the next prompt.

use opencode_kit::model::parse_messages;
use rune::Play;
use serde_json::{Value, json};

use crate::App;
use crate::stories::{Story, Trigger, frame, replay, sid, stage};

const KEYS: &[Trigger] = &[replay(open)];

fn user(id: &str, text: &str) -> Value {
    json!({ "id": id, "type": "user", "text": text })
}

fn reply(id: &str, content: Value) -> Value {
    json!({ "id": id, "type": "assistant", "time": { "completed": 2 }, "content": content })
}

fn finished(id: &str, agent: &str, task: &str) -> Value {
    json!({ "id": id, "type": "synthetic", "description": task, "metadata": { "source": "subagent", "state": "completed", "agent": agent } })
}

fn thought(text: &str) -> Value {
    json!({ "type": "reasoning", "text": text, "time": { "start": 1, "end": 2 } })
}

fn prose(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn capsule(id: &str, agent: &str, task: &str) -> Value {
    json!({ "type": "tool", "id": id, "name": "subagent", "state": {
        "status": "completed", "title": task,
        "input": { "agent": agent, "description": task, "background": true },
        "metadata": { "background": true, "status": "running" },
    } })
}

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    let page = vec![
        user("msg_rh_01", "Is the task runtime fast yet?"),
        reply("msg_rh_02", json!([
            prose("**The honest part: none of this is measured.** The task runtime doesn't exist yet, so \"fast\" is a design argument for now.\n\nRust refuses this program:\n\n```rust\nlet buf = vec![0u8; 1024];\ntokio::spawn(async { read_into(&buf).await });\n```\n\nWhy do you think the compiler says no? What could go wrong if it allowed it?"),
        ])),
        finished("msg_rh_03", "blood-reviewer", "Review strengthened tests"),
        finished("msg_rh_04", "blood-builder", "Slop batch 2: memory references"),
        reply("msg_rh_05", json!([
            thought("Both came back clean."),
            prose("Two small cleanups are cleared to land, including the broken doc links that kept showing up as background failures."),
        ])),
        finished("msg_rh_06", "blood-builder", "Bound closure contract size"),
        reply("msg_rh_07", json!([capsule("prt_rh_1", "blood-reviewer", "Confirm contract bound holds")])),
        finished("msg_rh_08", "blood-reviewer", "Review layout port trigger"),
        reply("msg_rh_09", json!([
            thought("The layout port needs another pass."),
            capsule("prt_rh_2", "blood-builder", "Register layout comparison"),
            prose("The closure checker's bound now holds across three fuzz seeds and is back with Opus for a check.\n\nThe `tokio::spawn` question is still waiting for you."),
        ])),
        user("msg_rh_10", "Frames, because the heap is the slow part."),
        reply("msg_rh_11", json!([prose("Right: a frame is one allocation for the whole task, sized when it's spawned.")])),
    ];
    let messages = parse_messages(&json!({ "data": page }));
    if let Some(t) = s.transcripts.get_mut(&id) {
        t.messages = messages;
    }
}

fn view(s: &App, p: &crate::palette::Pal) -> rune::El {
    frame(p, "story-session").h_full().child(crate::view::story_session(s, p))
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Timeline/Rhythm", scene: |s| stage(s, view), knobs: &[], keys: KEYS, variants: &[] },
]);
