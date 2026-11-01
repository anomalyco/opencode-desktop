//! The permission dock's cell: a request takes the composer's place, Allow once brings the
//! next one in, Deny brings the composer back with its draft.

use serde_json::json;

use super::{Run, SID};

pub fn permission(r: &mut Run) {
    // Replies go to a channel, never a server; it stays open for the run.
    std::mem::forget(r.rt.state.fake_writes());
    let ask = |id: &str, action: &str, resources: &[&str]| json!({ "id": id, "sessionID": SID, "action": action, "resources": resources });
    r.event("permission.asked", ask("per_gate_1", "bash", &["bun test test/net/backoff.test.ts"]));
    r.event("permission.asked", ask("per_gate_2", "edit", &["src/net/backoff.ts", "src/net/backoff.test.ts", "src/net/reconnect.ts"]));
    r.play(0.8);
    r.act("allow once", |rt| Run::tap(rt, "perm-once"));
    r.play(0.8);
    r.act("deny", |rt| Run::tap(rt, "perm-deny"));
    r.play_out(0.8);
}
