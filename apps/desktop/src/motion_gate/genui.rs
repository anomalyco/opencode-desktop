//! A reply streams a plan in ```` ```ui ```` outlines (`genui`): each outline grows a line at a
//! time under the prose before it, its fence never drawn, and nothing above it moves. A reply
//! streams a flowchart and a sequence diagram (`stream-diagram`): each draws from its complete
//! lines, nodes growing in and edges drawing on, and closing its fence moves nothing.

use serde_json::json;

use super::{Run, SID};
use crate::genui::demo::{PLAN, chunks};
use crate::genui::diagrams::REPLY;

pub fn stream_outline(r: &mut Run) {
    stream_reply(r, "msg_zzgenuigate", PLAN);
}

pub fn stream_diagram(r: &mut Run) {
    stream_reply(r, "msg_zzdiagramgate", REPLY);
}

fn stream_reply(r: &mut Run, id: &str, text: &str) {
    r.act("the run starts", |rt| rt.update(|s, _| {
        s.running.insert(SID.into());
    }));
    r.event("session.execution.started", json!({ "sessionID": SID, "assistantMessageID": id }));
    r.event("session.step.started", json!({ "sessionID": SID, "assistantMessageID": id, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }));
    r.event("session.text.started", json!({ "sessionID": SID, "assistantMessageID": id, "ordinal": 0 }));
    for c in chunks(text) {
        r.event("session.text.delta", json!({ "sessionID": SID, "assistantMessageID": id, "ordinal": 0, "delta": c }));
        r.play(0.05);
    }
    r.event("session.text.ended", json!({ "sessionID": SID, "assistantMessageID": id, "ordinal": 0, "text": text }));
    r.event("session.step.ended", json!({ "sessionID": SID, "assistantMessageID": id, "finish": "stop", "cost": 0.0, "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }));
    r.event("session.execution.succeeded", json!({ "sessionID": SID, "assistantMessageID": id }));
    r.play_out(1.0);
}
