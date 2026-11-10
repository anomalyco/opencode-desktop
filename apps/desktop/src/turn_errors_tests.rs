use opencode_kit::model::{Assistant, Message, NoticeKind, Retry, StepError, Transcript, parse_messages};
use rune::LocalTime;
use serde_json::json;

use super::*;

const SID: &str = "ses_turn_errors";

fn app(messages: Vec<Message>) -> App {
    let mut s = App::empty();
    s.transcripts.insert(SID.into(), Transcript { session: SID.into(), messages });
    s.route = Route::here(SID.into());
    s
}

fn prompt() -> Message {
    Message::User { id: "msg_a".into(), text: "Go".into() }
}

fn failed(kind: &str, message: &str) -> Assistant {
    let failure = StepError { kind: kind.into(), message: message.into(), ..Default::default() };
    Assistant { id: "msg_b".into(), completed: Some(10), error: Some(message.into()), failure: Some(failure), ..Default::default() }
}

#[test]
fn the_last_step_reports_its_error_and_a_running_one_its_retry() {
    let s = app(vec![prompt(), Message::Assistant(failed("provider", "Bad Request: nope"))]);
    assert!(crate::view::row_keys(&s, &s.transcripts[SID]).contains(&"e-msg_b".to_owned()));
    let retrying = Assistant { id: "msg_b".into(), retry: Some(Retry { attempt: 2, at: 5_000, message: "Rate limited".into() }), ..Default::default() };
    let s = app(vec![prompt(), Message::Assistant(retrying)]);
    let keys = crate::view::row_keys(&s, &s.transcripts[SID]);
    assert!(keys.contains(&"r-msg_b".to_owned()) && !keys.contains(&"e-msg_b".to_owned()), "{keys:?}");
}

#[test]
fn an_abort_is_not_an_error() {
    let s = app(vec![prompt(), Message::Assistant(failed("session.aborted", "Aborted"))]);
    assert!(!crate::view::row_keys(&s, &s.transcripts[SID]).iter().any(|k| k.starts_with("e-")));
}

#[test]
fn limits_are_told_apart_with_their_reset() {
    let mut free = failed("provider.free-limit", "x");
    free.failure.as_mut().unwrap().retry_after_ms = Some(3_600_000);
    assert_eq!(limit(&free), Some((Limit::Free, Some(3_600_010))));
    free.failure.as_mut().unwrap().retry_after_ms = Some(8 * 86_400_000);
    assert_eq!(limit(&free), Some((Limit::Free, None)), "a wait past a week is dropped");
    assert_eq!(limit(&failed("provider.go-limit", "x")), Some((Limit::Go, None)));
    let mut budget = failed("provider.model-budget-limit", "x");
    budget.failure.as_mut().unwrap().body = Some(r#"{"error":{"metadata":{"resetsAt": "2026-11-01T00:00:00.000Z"}}}"#.into());
    assert_eq!(limit(&budget), Some((Limit::ModelBudget, parse_iso("2026-11-01T00:00:00Z"))));
    assert_eq!(limit(&failed("provider", "x")), None);
}

#[test]
fn times_read_as_desktop_formats_them() {
    assert_eq!(parse_iso("1970-01-02T00:00:00Z"), Some(86_400_000));
    assert_eq!(parse_iso("1970-01-01T01:00:00+01:00"), Some(0));
    let t = LocalTime::at_offset(parse_iso("2026-11-01T07:05:00Z").unwrap(), -7 * 3600);
    assert_eq!(short_time(t), "12:05 AM");
    assert_eq!(date_time(t), "Nov 1, 2026, 12:05 AM");
    assert_eq!(budget_time(t), "Nov 1, 12:05 AM GMT-7");
    let t = LocalTime::at_offset(parse_iso("2026-11-01T07:35:00Z").unwrap(), 19_800);
    assert_eq!(budget_time(t), "Nov 1, 1:05 PM GMT+5:30");
}

#[test]
fn limit_copy_matches_desktop() {
    assert_eq!(limit_copy(Limit::Go, None, 0).0, "You've reached your OpenCode Go usage limit.");
    let (words, action) = limit_copy(Limit::Budget, None, 0);
    assert_eq!(words, "Workspace budget reached · paid models paused until it resets · free models still work");
    assert_eq!(action, Some("Use a free model"));
    assert_eq!(limit_copy(Limit::ModelBudget, None, 0).1, Some("Switch model"));
    assert!(limit_copy(Limit::Free, Some(3_600_000), 0).0.starts_with("You've used today's free messages. They reset at "));
}

#[test]
fn provider_envelopes_unwrap() {
    assert_eq!(unwrap_error(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#), "overloaded_error: Overloaded");
    assert_eq!(unwrap_error(r#"Error: 529 {"error":{"message":"Busy"}}"#), "Busy");
    assert_eq!(unwrap_error("Plain words"), "Plain words");
}

#[test]
fn retry_counts_down_in_whole_seconds() {
    let r = Retry { attempt: 3, at: 10_000, message: "Rate limited".into() };
    assert_eq!(retry_copy(&r, 4_600).1, "Attempt 3 - retrying in 5s");
    assert_eq!(retry_copy(&r, 10_000).1, "Attempt 3 - retrying");
    let hot = Retry { message: "You exceeded your current quota for gemini-3".into(), ..r };
    assert_eq!(retry_copy(&hot, 0).0, "gemini is way too hot right now");
}

#[test]
fn notices_parse_from_messages_and_events() {
    let list = parse_messages(&json!({ "data": [
        { "id": "msg_1", "type": "agent-switched", "agent": "plan", "previous": "build", "time": { "created": 1 } },
        { "id": "msg_2", "type": "model-switched", "model": { "id": "b", "providerID": "p", "variant": "high" }, "previous": { "id": "b", "providerID": "p" }, "time": { "created": 2 } },
        { "id": "msg_3", "type": "system", "text": "…", "description": "Instructions updated: AGENTS.md, a/AGENTS.md", "time": { "created": 3 } },
        { "id": "msg_4", "type": "synthetic", "text": "", "metadata": { "notice": "restart" }, "time": { "created": 4 } },
        { "id": "msg_5", "type": "synthetic", "text": "", "description": "Shell done", "metadata": { "source": "shell" }, "time": { "created": 5 } },
    ] }));
    let kinds: Vec<NoticeKind> = list.into_iter().filter_map(|m| if let Message::Notice(n) = m { Some(n.kind) } else { None }).collect();
    assert_eq!(kinds.len(), 5, "a background shell's notice with a description is one: {kinds:?}");
    assert_eq!(kinds[2], NoticeKind::Instructions { sources: vec!["AGENTS.md".into(), "a/AGENTS.md".into()] });
    assert_eq!(kinds[3], NoticeKind::Restart);

    let mut t = Transcript { session: SID.into(), messages: vec![prompt()] };
    assert!(t.apply("session.moved", &json!({ "sessionID": SID, "eventID": "evt_9", "location": { "directory": "/x" } }), 0));
    assert!(!t.apply("session.moved", &json!({ "sessionID": SID, "eventID": "evt_9", "location": { "directory": "/x" } }), 0), "once");
    assert!(matches!(&t.messages[1], Message::Notice(n) if n.id == "msg_9"));
    assert!(t.busy(), "a notice doesn't end the turn it lands in");
}

#[test]
fn retry_events_set_and_clear_it() {
    let mut t = Transcript { session: SID.into(), messages: vec![prompt()] };
    t.apply("session.step.started", &json!({ "sessionID": SID, "assistantMessageID": "msg_b" }), 0);
    t.apply("session.retry.scheduled", &json!({ "sessionID": SID, "assistantMessageID": "msg_b", "attempt": 2, "at": 900, "error": { "type": "x", "message": "Rate limited" } }), 0);
    assert_eq!(t.retrying().map(|r| r.attempt), Some(2));
    t.apply("session.step.started", &json!({ "sessionID": SID, "assistantMessageID": "msg_b" }), 1);
    assert_eq!(t.retrying(), None);
}

#[test]
fn the_fixture_has_each_row() {
    use crate::fixture_ids::*;
    let mut s = App::empty();
    s.load_fixture(crate::FIXTURE);
    let keys = |s: &mut App, id: &str| {
        s.route = Route::here(id.into());
        crate::view::row_keys(s, &s.transcripts[id])
    };
    for id in [FREE_LIMIT, GO_LIMIT, BUDGET, MODEL_BUDGET, ERRORED] {
        assert!(keys(&mut s, id).iter().any(|k| k.starts_with("e-")), "{id}");
    }
    assert!(keys(&mut s, RETRYING).iter().any(|k| k.starts_with("r-")));
    let notices = s.transcripts[NOTICES].messages.iter().filter(|m| matches!(m, Message::Notice(_))).count();
    assert_eq!(notices, 6);
    assert_eq!(keys(&mut s, NOTICES).iter().filter(|k| k.starts_with("n-")).count(), 6);
}
