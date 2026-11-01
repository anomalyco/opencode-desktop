//! A reply streams a plan in ```` ```ui ```` outlines (`genui`): each outline grows a line at a
//! time under the prose before it, its fence never drawn, and nothing above it moves.

use serde_json::json;

use super::{Run, SID};
use crate::genui::demo::{PLAN, chunks};

pub fn stream_outline(r: &mut Run) {
    let id = "msg_zzgenuigate";
    r.act("the run starts", |rt| rt.update(|s, _| {
        s.running.insert(SID.into());
    }));
    r.event("session.execution.started", json!({ "sessionID": SID, "assistantMessageID": id }));
    r.event("session.step.started", json!({ "sessionID": SID, "assistantMessageID": id, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }));
    r.event("session.text.started", json!({ "sessionID": SID, "assistantMessageID": id, "ordinal": 0 }));
    for c in chunks(PLAN) {
        r.event("session.text.delta", json!({ "sessionID": SID, "assistantMessageID": id, "ordinal": 0, "delta": c }));
        r.play(0.05);
    }
    r.event("session.text.ended", json!({ "sessionID": SID, "assistantMessageID": id, "ordinal": 0, "text": PLAN }));
    r.event("session.step.ended", json!({ "sessionID": SID, "assistantMessageID": id, "finish": "stop", "cost": 0.0, "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }));
    r.event("session.execution.succeeded", json!({ "sessionID": SID, "assistantMessageID": id }));
    r.play_out(1.0);
}
