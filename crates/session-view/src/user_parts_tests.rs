use super::*;

const ID: &str = "ses_fx999900aaaaaaaaaaaaaaaaaa";

fn body() -> String {
    format!("the @session:{ID} session")
}

/// The echo's offsets came from `the @SEER session`: five bytes at 4, which on the server's
/// copy cover only `@sess` (p279).
fn stale_echo() -> Vec<UserMention> {
    vec![UserMention { range: 4..9, kind: MentionKind::Session }]
}

#[test]
fn a_stale_echo_range_no_longer_cuts_a_session_mention() {
    let body = body();
    for stale in [stale_echo(), vec![UserMention { range: 4..9, kind: MentionKind::Agent }]] {
        let (text, marks) = bubble_marks(&body, &stale, |id| (id == ID).then(|| "SEER".to_owned()));
        assert_eq!(text, "the SEER session");
        assert_eq!(marks, [BubbleMark { range: 4..8, kind: MentionKind::Session, session: Some(ID.to_owned()), title: Some("SEER".to_owned()) }]);
    }
}

#[test]
fn an_unknown_session_reads_as_its_short_id() {
    let (text, marks) = bubble_marks(&body(), &stale_echo(), |_| None);
    assert_eq!(text, "the ses_fx999900 session");
    assert_eq!(&text[marks[0].range.clone()], "ses_fx999900");
    assert_eq!((marks[0].session.as_deref(), marks[0].title.as_deref()), (Some(ID), None));
    assert_eq!(session_tip(ID, None), format!("Open session {ID}"));
    assert_eq!(session_tip(ID, Some("SEER")), format!("Open session “SEER”\n{ID}"));
}

#[test]
fn an_echo_without_tokens_keeps_its_session_badges_and_others_stay() {
    let typed = "the @SEER session with @explore";
    let echo = [UserMention { range: 4..9, kind: MentionKind::Session }, UserMention { range: 23..31, kind: MentionKind::Agent }];
    let (text, marks) = mention_bubble(typed, &echo, |_| None);
    assert_eq!(text, typed);
    assert_eq!(marks, [(4..9, MentionKind::Session), (23..31, MentionKind::Agent)]);
    // On the server's copy the agent's own offsets come with it; the stale session one goes.
    let raw = format!("the @session:{ID} session with @explore");
    let at = raw.find("@explore").unwrap();
    let server = [UserMention { range: 4..9, kind: MentionKind::Session }, UserMention { range: at..at + 8, kind: MentionKind::Agent }];
    let (text, marks) = mention_bubble(&raw, &server, |_| Some("SEER".into()));
    assert_eq!(text, "the SEER session with @explore");
    assert_eq!(marks, [(4..8, MentionKind::Session), (22..30, MentionKind::Agent)]);
}

#[test]
fn short_ids_cut_after_twelve_characters() {
    assert_eq!(short_session_id(ID), "ses_fx999900");
    assert_eq!(short_session_id("ses_ab"), "ses_ab");
}
