use serde_json::json;

use super::*;

fn only(t: &Transcript) -> &Compaction {
    match t.messages.as_slice() {
        [Message::Compaction(c)] => c,
        other => panic!("one compaction, got {other:?}"),
    }
}

#[test]
fn parses_each_status() {
    let tokens = json!({"input": 100, "output": 20, "reasoning": 5, "cache": {"read": 7, "write": 3}});
    let running = json!({"id": "msg_r", "type": "compaction", "status": "running", "reason": "auto", "summary": "so far", "time": {"created": 5}});
    let done = json!({"id": "msg_d", "type": "compaction", "status": "completed", "reason": "manual", "summary": "## Goal", "tokens": tokens, "time": {"created": 6}});
    let native = json!({"id": "msg_n", "type": "compaction", "status": "completed", "reason": "auto", "summary": "", "native": {"messages": []}, "time": {"created": 7}});
    let failed = json!({"id": "msg_f", "type": "compaction", "status": "failed", "reason": "auto", "error": {"type": "aborted", "message": "Cancelled"}, "time": {"created": 8}});
    let parsed: Vec<Compaction> = [running, done, native, failed]
        .iter()
        .map(|m| match parse_message(m) {
            Some(Message::Compaction(c)) => c,
            other => panic!("a compaction, got {other:?}"),
        })
        .collect();
    let [r, d, n, f] = parsed.as_slice() else { unreachable!() };
    assert_eq!((r.status, r.manual, r.summary.as_str(), r.created, r.tokens.is_none()), (CompactionStatus::Running, false, "so far", 5, true));
    assert_eq!((d.status, d.manual, d.native), (CompactionStatus::Completed, true, false));
    let t = d.tokens.as_ref().unwrap();
    assert_eq!((t.input, t.output, t.reasoning, t.cache_read, t.cache_write), (100, 20, 5, 7, 3));
    assert_eq!((n.status, n.native), (CompactionStatus::Completed, true));
    assert_eq!((f.status, f.error.clone()), (CompactionStatus::Failed, Some(("aborted".into(), "Cancelled".into()))));
}

#[test]
fn follows_compaction_events() {
    let mut t = Transcript { session: "s".into(), messages: Vec::new() };
    let started = json!({"sessionID": "s", "reason": "auto", "eventID": "evt_abc"});
    assert!(t.apply("session.compaction.started", &started, 40));
    assert!(!t.apply("session.compaction.started", &started, 41), "a replayed start is ignored");
    assert_eq!((only(&t).id.as_str(), only(&t).status, only(&t).created), ("msg_abc", CompactionStatus::Running, 40));
    assert!(t.apply("session.compaction.delta", &json!({"sessionID": "s", "text": "## Go"}), 50));
    assert!(t.apply("session.compaction.delta", &json!({"sessionID": "s", "text": "al"}), 50));
    assert_eq!(only(&t).summary, "## Goal");
    let ended = json!({"sessionID": "s", "reason": "auto", "text": "## Goal\nShip it", "tokens": {"input": 9, "output": 1, "reasoning": 0, "cache": {"read": 0, "write": 0}}});
    assert!(t.apply("session.compaction.ended", &ended, 60));
    let c = only(&t);
    assert_eq!((c.status, c.summary.as_str(), c.tokens.as_ref().map(|t| t.input)), (CompactionStatus::Completed, "## Goal\nShip it", Some(9)));

    // A queued compaction starts under its inbox id, and fails in place.
    let mut t = Transcript { session: "s".into(), messages: Vec::new() };
    t.apply("session.compaction.started", &json!({"sessionID": "s", "reason": "manual", "inputID": "msg_q"}), 1);
    let failed = json!({"sessionID": "s", "reason": "manual", "error": {"type": "provider", "message": "Rate limited"}});
    assert!(t.apply("session.compaction.failed", &failed, 2));
    let c = only(&t);
    assert_eq!((c.id.as_str(), c.status, c.manual), ("msg_q", CompactionStatus::Failed, true));
    assert_eq!(c.error.as_ref().map(|e| e.1.as_str()), Some("Rate limited"));

    // An end with nothing running appends a settled one.
    let mut t = Transcript { session: "s".into(), messages: Vec::new() };
    assert!(t.apply("session.compaction.ended", &json!({"sessionID": "s", "reason": "auto", "text": "s", "eventID": "evt_z"}), 3));
    assert_eq!((only(&t).id.as_str(), only(&t).status), ("msg_z", CompactionStatus::Completed));
}

#[test]
fn an_end_the_page_already_holds_adds_nothing() {
    let tokens = json!({"input": 4, "output": 2439, "reasoning": 0, "cache": {"read": 169624, "write": 1053}});
    let settled = json!({"data": [{"id": "msg_s", "type": "compaction", "status": "completed", "reason": "auto", "summary": "## Goal", "tokens": tokens, "time": {"created": 1}}]});
    let mut t = Transcript { session: "s".into(), messages: Vec::new() };
    t.merge(parse_messages(&settled));
    assert!(!t.apply("session.compaction.started", &json!({"sessionID": "s", "reason": "auto", "eventID": "evt_s"}), 2));
    assert!(!t.apply("session.compaction.ended", &json!({"sessionID": "s", "reason": "auto", "text": "## Goal", "tokens": tokens, "eventID": "evt_e"}), 3));
    assert_eq!((only(&t).id.as_str(), only(&t).status), ("msg_s", CompactionStatus::Completed));

    // The same for a failure.
    let error = json!({"type": "provider", "message": "Rate limited"});
    let failed = json!({"data": [{"id": "msg_f", "type": "compaction", "status": "failed", "reason": "auto", "error": error, "time": {"created": 1}}]});
    let mut t = Transcript { session: "s".into(), messages: Vec::new() };
    t.merge(parse_messages(&failed));
    assert!(!t.apply("session.compaction.failed", &json!({"sessionID": "s", "reason": "auto", "error": error, "eventID": "evt_x"}), 3));
    assert_eq!(only(&t).id, "msg_f");
}

#[test]
fn a_page_read_while_it_ran_does_not_rewind_its_end() {
    let running = json!({"data": [{"id": "msg_abc", "type": "compaction", "status": "running", "reason": "auto", "summary": "", "time": {"created": 1}}]});
    let mut t = Transcript { session: "s".into(), messages: Vec::new() };
    t.apply("session.compaction.started", &json!({"sessionID": "s", "reason": "auto", "eventID": "evt_abc"}), 1);
    t.apply("session.compaction.ended", &json!({"sessionID": "s", "reason": "auto", "text": "## Goal"}), 2);
    t.merge(parse_messages(&running));
    assert_eq!((only(&t).status, only(&t).summary.as_str()), (CompactionStatus::Completed, "## Goal"));
}

#[test]
fn notes_a_queued_compaction() {
    let mut inbox = Inbox::default();
    let listed = json!({"sessionID": "s", "items": [{"id": "msg_c", "type": "compaction", "delivery": "steer", "payload": {}}]});
    assert_eq!(inbox.apply("session.inbox.listed", &listed), InboxChange::Changed);
    assert_eq!(inbox.compaction.as_deref(), Some("msg_c"));
    assert!(inbox.items.is_empty());
    assert_eq!(inbox.apply("session.inbox.delivered", &json!({"inboxID": "msg_c"})), InboxChange::Changed);
    assert_eq!(inbox.compaction, None);
    let enqueued = json!({"sessionID": "s", "inboxID": "msg_d", "item": {"type": "compaction", "delivery": "steer", "payload": {}}});
    assert_eq!(inbox.apply("session.inbox.enqueued", &enqueued), InboxChange::Changed);
    assert_eq!(inbox.apply("session.inbox.enqueued", &enqueued), InboxChange::None);
    assert_eq!(inbox.apply("session.inbox.cancelled", &json!({"inboxID": "msg_d"})), InboxChange::Changed);
    assert_eq!(inbox.compaction, None);
}

#[test]
fn reads_background_compaction_from_the_last_document_that_sets_experimental() {
    let entry = |on: Option<bool>| match on {
        Some(on) => json!({"type": "document", "info": {"experimental": {"background_compaction": on}}}),
        None => json!({"type": "document", "info": {}}),
    };
    assert!(background_compaction(&json!([entry(Some(false)), entry(Some(true)), entry(None)])));
    assert!(!background_compaction(&json!([entry(Some(true)), entry(Some(false))])));
    assert!(!background_compaction(&json!([{"type": "directory", "info": {}}])));
    assert!(background_compaction(&json!({"data": [entry(Some(true))]})));
}
