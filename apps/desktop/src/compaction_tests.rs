use opencode_kit::model::{Assistant, Compaction, CompactionStatus, Inbox, Message, Part, ToolStatus, Tokens, Transcript};

use super::*;

const SID: &str = "ses_compaction_test";

fn shell(id: &str) -> Message {
    let call = Part::Tool { id: format!("call_{id}"), name: "bash".into(), status: ToolStatus::Completed, title: "ls".into(), input: serde_json::Value::Null, files: vec![], metadata: serde_json::Value::Null };
    Message::Assistant(Assistant { id: id.into(), completed: Some(1), parts: vec![call], ..Default::default() })
}

fn compaction(status: CompactionStatus, manual: bool) -> Compaction {
    Compaction { id: "msg_cmp".into(), status, manual, summary: "## Goal\nShip it".into(), created: 0, ..Default::default() }
}

fn app(messages: Vec<Message>) -> App {
    let mut s = App::empty();
    s.transcripts.insert(SID.into(), Transcript { session: SID.into(), messages });
    s.route = Route::Session(SID.into());
    s
}

#[test]
fn a_compaction_breaks_the_fold() {
    let c = compaction(CompactionStatus::Completed, false);
    let s = app(vec![shell("msg_a"), shell("msg_b"), Message::Compaction(c), shell("msg_c"), shell("msg_d")]);
    let keys = crate::view::row_keys(&s, &s.transcripts[SID]);
    let groups: Vec<usize> = keys.iter().enumerate().filter(|(_, k)| k.starts_with("c-")).map(|(i, _)| i).collect();
    let divider = keys.iter().position(|k| k == "cmp-msg_cmp-quiet").expect("the compaction's divider");
    assert_eq!(groups.len(), 2, "two folds, one each side: {keys:?}");
    assert!(groups[0] < divider && divider < groups[1], "the compaction sits between them: {keys:?}");
}

#[test]
fn only_a_failure_is_loud() {
    for manual in [false, true] {
        for status in [CompactionStatus::Running, CompactionStatus::Completed] {
            assert_eq!(pieces(&compaction(status, manual)), [Piece::Quiet], "{status:?}, manual {manual}");
        }
    }
    assert_eq!(pieces(&compaction(CompactionStatus::Failed, false)), [Piece::Start, Piece::End], "a failure stays loud, without its summary");
}

#[test]
fn a_failure_shows_its_card_unless_cancelled_or_interrupted() {
    let fail = |ty: &str| Compaction { error: Some((ty.into(), "The model refused".into())), ..compaction(CompactionStatus::Failed, true) };
    assert_eq!(pieces(&fail("unknown")), [Piece::Start, Piece::End, Piece::Error]);
    assert_eq!(pieces(&fail("aborted")), [Piece::Start, Piece::End]);
    assert_eq!(outcome(&fail("aborted")), "Session compaction cancelled");
    assert_eq!(outcome(&fail("compaction.interrupted")), "Session compaction interrupted");
    assert_eq!(outcome(&fail("unknown")), "Session compaction failed");
    let native = Compaction { native: true, ..compaction(CompactionStatus::Completed, true) };
    assert_eq!(outcome(&native), "Session compacted by provider");
}

#[test]
fn usage_counts_the_cache_in_and_reasoning_out() {
    let tokens = Tokens { input: 10_000, cache_read: 2_000, cache_write: 300, output: 1_000, reasoning: 100 };
    let done = Compaction { tokens: Some(tokens.clone()), ..compaction(CompactionStatus::Completed, true) };
    assert_eq!(usage(&done).as_deref(), Some("12.3K in · 1.1K out"));
    let running = Compaction { tokens: Some(tokens), ..compaction(CompactionStatus::Running, true) };
    assert_eq!(usage(&running), None);
    let empty = Compaction { tokens: Some(Tokens::default()), ..compaction(CompactionStatus::Completed, true) };
    assert_eq!(usage(&empty), None);
    assert_eq!([0, 999, 1_000, 1_049, 12_345, 999_960, 1_500_000].map(compact), ["0", "999", "1K", "1K", "12.3K", "1M", "1.5M"]);
}

#[test]
fn a_queued_compaction_waits_under_the_turn() {
    let mut s = app(vec![shell("msg_a")]);
    s.inboxes.insert(SID.into(), Inbox { items: vec![], compaction: Some("inb_cmp".into()) });
    let keys = crate::view::row_keys(&s, &s.transcripts[SID]);
    let group = keys.iter().position(|k| k.starts_with("c-")).unwrap();
    let queued = keys.iter().position(|k| k == "cmpq-inb_cmp").expect("the queued divider");
    assert!(group < queued, "{keys:?}");
}

#[test]
fn a_quiet_one_ending_inside_its_grace_fades_in_as_compacted() {
    let mut s = app(vec![Message::Compaction(compaction(CompactionStatus::Running, false))]);
    s.now_ms = 100;
    s.clock = 10.0;
    sync(&mut s);
    assert_eq!(s.compactions.seen["msg_cmp"].enter, Some(Enter::Grace));
    s.clock = 10.2;
    if let Some(Message::Compaction(c)) = s.transcripts.get_mut(SID).unwrap().messages.first_mut() {
        c.status = CompactionStatus::Completed;
    }
    assert!(sync(&mut s));
    assert_eq!(s.compactions.seen["msg_cmp"], Seen { at: 10.2, enter: Some(Enter::Now), running: false });
}

#[test]
fn dividers_back_to_back_stack_tight_and_keep_the_usual_gap_outside() {
    use opencode_kit::model::{Notice, NoticeKind};
    let notice = |id: &str, kind: NoticeKind| Message::Notice(Notice { id: id.into(), kind });
    let moved = |id: &str| notice(id, NoticeKind::Moved { directory: "/repo".into() });
    let prompt = Message::User { id: "msg_p".into(), text: "inbox zero plz".into() };
    let s = app(vec![shell("msg_a"), prompt, moved("msg_n1"), Message::Compaction(compaction(CompactionStatus::Completed, false)), moved("msg_n2")]);
    let runs: Vec<Run> = crate::view::divider_runs(&s, &s.transcripts[SID]).into_iter().map(|(_, r)| r).collect();
    let run = |above, below| Run { above, below };
    assert_eq!(runs, [
        run(Beside::Spaced, Beside::Divider),
        run(Beside::Divider, Beside::Divider),
        run(Beside::Divider, Beside::Spaced),
    ]);
    let gap = s.knobs.get(&DIVIDER_GAP);
    let (_, under) = space(&s, runs[0]);
    let (over, _) = space(&s, runs[1]);
    assert_eq!(under + WORDS + over, gap, "line to line within a stack");
    assert_eq!(space(&s, runs[2]), (gap / 2.0 - WORDS / 2.0, 0.0), "Working brings its own room");
}
