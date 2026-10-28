use super::*;
use crate::mentions::tests::{key, runtime, shown, tap, typed};
use crate::Route;
use rune::{Key, KeyEvent, Mods};

const SESSION: &str = "ses_fx0003aaaaaaaaaaaaaaaaaa";

fn shift(rt: &mut rune::Runtime<App>, k: Key) {
    rt.key(KeyEvent { key: k, mods: Mods { shift: true, ..Default::default() } });
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_badge_is_one_unit_for_the_caret_selection_and_backspace() {
    let mut rt = runtime();
    tap(&mut rt, "prompt");
    typed(&mut rt, "a @op");
    key(&mut rt, Key::Enter);
    typed(&mut rt, "b");
    let c = &rt.state.composer;
    assert_eq!(c.text, "a @opencode b", "one space after the badge, none doubled");
    assert_eq!(c.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(), [(2..11, "skill:opencode".to_owned())]);
    key(&mut rt, Key::Left);
    key(&mut rt, Key::Left);
    assert_eq!(rt.state.composer.focus, 11);
    key(&mut rt, Key::Left);
    assert_eq!(rt.state.composer.focus, 2, "the caret steps over the badge in one move");
    key(&mut rt, Key::Right);
    assert_eq!(rt.state.composer.focus, 11);
    // A selection reaching into the badge covers all of it.
    key(&mut rt, Key::Left);
    key(&mut rt, Key::Left);
    shift(&mut rt, Key::Right);
    shift(&mut rt, Key::Right);
    assert_eq!(rt.state.composer.selected(), " @opencode");
    key(&mut rt, Key::Right);
    assert_eq!(rt.state.composer.focus, 11);
    key(&mut rt, Key::Backspace);
    assert_eq!(rt.state.composer.text, "a  b", "Backspace deletes the badge whole");
    assert_eq!(rt.state.composer.pills().count(), 0);
    typed(&mut rt, "x");
    assert_eq!(rt.state.composer.text, "a x b", "typing after it works");
}

#[test]
fn every_kind_gets_a_chip_in_its_colour_and_no_icon() {
    for dark in [false, true] {
        let p = *crate::palette::pal(dark);
        let k = ink(dark);
        let style = pill_style(&p);
        let agent = style("agent:explore");
        let file = style("file:src/a.rs");
        assert_eq!(format!("{agent:?}"), format!("{:?}", PillStyle::new().bg(k.agent.alpha(tint(dark))).fg(k.agent)));
        assert_eq!(format!("{file:?}"), format!("{:?}", PillStyle::new().bg(k.file.alpha(tint(dark))).fg(k.file)));
        assert_eq!(format!("{:?}", style("file:src/")), format!("{:?}", PillStyle::new().bg(k.file.alpha(tint(dark))).fg(k.file)));
        assert_eq!(format!("{:?}", style("skill:opencode")), format!("{:?}", PillStyle::new().bg(p.text.alpha(neutral(dark))).fg(p.text)));
        assert_eq!(format!("{:?}", style("session:ses_x")), format!("{:?}", PillStyle::new().bg(p.text.alpha(neutral(dark))).fg(p.text)));
        assert_eq!(format!("{:?}", style("quote:1")), format!("{:?}", PillStyle::new()), "other atoms keep their own look");
    }
}

#[test]
fn a_bubble_badges_its_mentions_and_names_sessions() {
    let body = "@explore look at @opencode and @session:ses_abc1 or @session:ses_gone";
    let at = |s: &str| body.find(s).unwrap();
    let mentions = [
        UserMention { range: at("@explore")..at("@explore") + 8, kind: MentionKind::Agent },
        UserMention { range: at("@opencode")..at("@opencode") + 9, kind: MentionKind::Skill },
        UserMention { range: body.len() - 2..body.len() + 4, kind: MentionKind::File },
    ];
    let (text, marks) = bubble(body, &mentions, |id| (id == "ses_abc1").then(|| "Fix flaky".to_owned()));
    assert_eq!(text, "@explore look at @opencode and @Fix flaky or @session:ses_gone");
    let got: Vec<(&str, MentionKind)> = marks.iter().map(|(r, k)| (&text[r.clone()], *k)).collect();
    assert_eq!(
        got,
        [
            ("@explore", MentionKind::Agent),
            ("@opencode", MentionKind::Skill),
            ("@Fix flaky", MentionKind::Session),
            ("@session:ses_gone", MentionKind::Session),
        ]
    );
}

#[test]
fn the_echo_counts_from_the_trimmed_text() {
    let pills = [(3..11, "agent:general".to_owned()), (12..14, "note:x".to_owned())];
    assert_eq!(echo("  @general ok  ", &[(2..10, "agent:general".to_owned())]), [UserMention { range: 0..8, kind: MentionKind::Agent }]);
    assert_eq!(echo("hi @general ok", &pills), [UserMention { range: 3..11, kind: MentionKind::Agent }]);
}

#[test]
fn a_sent_prompt_shows_badges_in_its_bubble_and_sends_the_same_payload() {
    let mut rt = runtime();
    rt.state.route = Route::Session(SESSION.into());
    rt.frame(1.0 / 60.0);
    tap(&mut rt, "prompt");
    typed(&mut rt, "what other @op");
    key(&mut rt, Key::Enter);
    typed(&mut rt, "findings from @exp");
    key(&mut rt, Key::Enter);
    typed(&mut rt, "did you have?");
    let w = rt.state.mention_wire();
    assert_eq!(w.text, "what other @opencode findings from @explore did you have?");
    assert_eq!(w.skills[0].mention.as_ref().map(|m| (m.start, m.end)), Some((11, 20)));
    assert_eq!(w.agents[0].mention.as_ref().map(|m| (m.start, m.end)), Some((35, 43)));
    key(&mut rt, Key::Enter);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let id = "msg_zzzzsent0001";
    let marks: Vec<(Range<usize>, MentionKind)> = mention::user_mentions(id).iter().map(|m| (m.range.clone(), m.kind)).collect();
    assert_eq!(marks, [(11..20, MentionKind::Skill), (35..43, MentionKind::Agent)]);
    let text = shown(&rt, &format!("text-{id}")).expect("the bubble shows");
    assert_eq!(text.text.as_deref(), Some("what other @opencode findings from @explore did you have?"));
    // The chips' padding takes room: the same words without badges are narrower.
    let badged = text.rect[2];
    mention::set_user_mentions(id, Vec::new());
    rt.state.transcripts.get_mut(SESSION).unwrap().messages.push(opencode_kit::model::Message::Note { id: "n-relayout".into(), text: String::new() });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let plain = shown(&rt, &format!("text-{id}")).unwrap().rect[2];
    assert!(badged > plain + 8.0, "badged {badged} vs plain {plain}");
}
