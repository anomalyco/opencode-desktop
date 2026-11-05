//! Compaction messages: the server summarises the conversation, and the summary replaces the
//! history before it in later requests. Parsed from `type: "compaction"` messages and kept
//! current by `session.compaction.*` events, as OpenCode's message updater does.

use serde_json::Value;

use super::{Message, Tokens, i, s};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompactionStatus {
    #[default]
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct Compaction {
    pub id: String,
    pub status: CompactionStatus,
    /// Asked for by a person (`/compact`) rather than started automatically.
    pub manual: bool,
    pub summary: String,
    /// Usage of the compaction request itself, once it ends.
    pub tokens: Option<Tokens>,
    /// The provider compacted it (`native.messages`) instead of a summary.
    pub native: bool,
    /// The failure's `type` and `message`.
    pub error: Option<(String, String)>,
    pub created: i64,
}

impl Compaction {
    pub(super) fn parse(m: &Value) -> Compaction {
        Compaction {
            id: s(&m["id"]),
            status: match m["status"].as_str() {
                Some("completed") => CompactionStatus::Completed,
                Some("failed") => CompactionStatus::Failed,
                _ => CompactionStatus::Running,
            },
            manual: m["reason"] == "manual",
            summary: s(&m["summary"]),
            tokens: m["tokens"].is_object().then(|| Tokens::parse(&m["tokens"])),
            native: !m["native"]["messages"].is_null(),
            error: m["error"].is_object().then(|| (s(&m["error"]["type"]), s(&m["error"]["message"]))),
            created: i(&m["time"]["created"]),
        }
    }

    /// Ends it as a `session.compaction.ended` or `.failed` payload says.
    fn end(&mut self, ty: &str, d: &Value) {
        self.manual = d["reason"] == "manual";
        self.tokens = d["tokens"].is_object().then(|| Tokens::parse(&d["tokens"]));
        if ty == "session.compaction.failed" {
            self.status = CompactionStatus::Failed;
            self.error = Some((s(&d["error"]["type"]), s(&d["error"]["message"])));
        } else {
            self.status = CompactionStatus::Completed;
            self.summary = s(&d["text"]);
            self.native = !d["native"]["messages"].is_null();
        }
    }
}

/// Applies one `session.compaction.*` event; returns whether anything changed. A start appends a
/// running compaction, a delta streams its summary, and an end or failure settles the running one
/// (or appends a settled one when none is running).
pub(super) fn apply(messages: &mut Vec<Message>, ty: &str, d: &Value, now: i64) -> bool {
    let running = messages.iter_mut().rev().find_map(|m| if let Message::Compaction(c) = m { Some(c) } else { None });
    let running = running.filter(|c| c.status == CompactionStatus::Running);
    let id = match ty {
        "session.compaction.started" => d["inputID"].as_str().map_or_else(|| event_message_id(d, now), str::to_owned),
        "session.compaction.delta" => {
            let Some(c) = running else { return false };
            c.summary.push_str(d["text"].as_str().unwrap_or(""));
            return true;
        }
        _ => {
            if let Some(c) = running {
                c.end(ty, d);
                return true;
            }
            if settled_already(messages, ty, d) {
                return false;
            }
            let input = d["inputID"].as_str().filter(|_| ty == "session.compaction.failed");
            input.map_or_else(|| event_message_id(d, now), str::to_owned)
        }
    };
    if messages.iter().any(|m| m.id() == id) {
        return false;
    }
    let mut c = Compaction { id, manual: d["reason"] == "manual", created: now, ..Default::default() };
    if ty != "session.compaction.started" {
        c.end(ty, d);
    }
    messages.push(Message::Compaction(c));
    true
}

/// Whether the last compaction already ended as this end or failure says: a read of the latest
/// page landed before the event, holding it settled under its start's id. Without this the end
/// adds the same compaction again under its own id.
fn settled_already(messages: &[Message], ty: &str, d: &Value) -> bool {
    let Some(last) = messages.iter().rev().find_map(|m| if let Message::Compaction(c) = m { Some(c) } else { None }) else { return false };
    let mut ended = Compaction::default();
    ended.end(ty, d);
    (last.status, &last.summary, last.native, &last.error, &last.tokens) == (ended.status, &ended.summary, ended.native, &ended.error, &ended.tokens)
}

/// The id the server gives a message an event created: the event's id (`eventID`, which the
/// client stamps on) with `msg_` for `evt_`.
fn event_message_id(d: &Value, now: i64) -> String {
    d["eventID"].as_str().map_or_else(|| format!("msg_compaction_{now}"), |e| format!("msg_{}", e.strip_prefix("evt_").unwrap_or(e)))
}

/// Whether `/api/config`'s documents turn on `experimental.background_compaction`: the last
/// document that sets `experimental` decides, as the server reads them.
pub fn background_compaction(v: &Value) -> bool {
    let list = v.as_array().or_else(|| v["data"].as_array());
    let last = list.into_iter().flatten().filter(|e| e["type"] == "document" && e["info"]["experimental"].is_object()).last();
    last.is_some_and(|e| e["info"]["experimental"]["background_compaction"] == true)
}
