use crate::fixture_ids::{THIRD as SESSION};
use super::*;
use opencode_kit::mention;
use crate::mentions::tests::{key, runtime, shown, tap, typed};
use crate::Route;
use rune::{Key, KeyEvent, Mods};


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
        let k = ink(&p);
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
    assert_eq!(text, "@explore look at @opencode and Fix flaky or ses_gone");
    let got: Vec<(&str, MentionKind)> = marks.iter().map(|(r, k)| (&text[r.clone()], *k)).collect();
    assert_eq!(
        got,
        [
            ("@explore", MentionKind::Agent),
            ("@opencode", MentionKind::Skill),
            ("Fix flaky", MentionKind::Session),
            ("ses_gone", MentionKind::Session),
        ]
    );
}

const UNKNOWN: &str = "ses_fx999900aaaaaaaaaaaaaaaaaa";

/// p279: a prompt naming a session reads as one badge, its title or short id, even with the
/// local echo's offsets (taken from the text as typed, `the @SEER session`) still recorded;
/// pressing it opens the session. At zoom 1.6, as Kit reads.
#[test]
fn a_session_mention_is_one_badge_that_opens_it_at_zoom_1_6() {
    for (n, target) in [(0, crate::fixture_ids::SIXTH), (1, UNKNOWN)] {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        app.route = Route::Session(SESSION.into());
        app.zoom = 1.6;
        let id = format!("msg_zzzzmention{n:02}");
        let body = format!("the @session:{target} session");
        app.transcripts.get_mut(SESSION).unwrap().messages.push(opencode_kit::model::Message::User { id: id.clone(), text: body });
        mention::set_user_mentions(&id, vec![UserMention { range: 4..9, kind: MentionKind::Session }]);
        let label = app.mention_title(target).unwrap_or_else(|| "ses_fx999900".to_owned());
        assert_eq!(n == 0, label != "ses_fx999900", "SIXTH is known, the other isn't");
        let mut rt = rune::Runtime::new(app, crate::view::app, (1600.0, 1400.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        rt.poll = Some(crate::poll);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let t = shown(&rt, &format!("text-{id}")).expect("the bubble shows");
        assert_eq!(t.text.as_deref(), Some(format!("the {label} session").as_str()));
        let r = t.rect;
        let mut glyphs: Vec<[f32; 4]> = rt
            .instances
            .iter()
            .filter(|i| i.params[1] == rune::render::KIND_GLYPH && i.rect[0] >= r[0] - 1.0 && i.rect[0] <= r[0] + r[2] && i.rect[1] >= r[1] - 4.0 && i.rect[1] <= r[1] + r[3])
            .map(|i| i.rect)
            .collect();
        glyphs.sort_by(|a, b| a[0].total_cmp(&b[0]));
        // "the", the badge's icon, then its first letter.
        let g = glyphs[4];
        let (x, y) = (g[0] + g[2] / 2.0, g[1] + g[3] / 2.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.route, Route::Session(target.to_owned()), "pressing {label} opens it");
        assert!(rt.state.session(target).is_some());
    }
}

#[test]
fn a_sent_session_mention_echoes_as_its_token() {
    let mut rt = runtime();
    rt.state.route = Route::Session(SESSION.into());
    rt.frame(1.0 / 60.0);
    // Its own message id: the mention store is shared with tests running beside it.
    rt.state.sends = 800;
    tap(&mut rt, "prompt");
    let sixth = crate::fixture_ids::SIXTH;
    let title = rt.state.mention_title(sixth).expect("a fixture session");
    let typed_text = format!("see @{title} and ");
    rt.state.composer = rune::TextEdit::new("");
    rt.state.composer.insert("see ");
    rt.state.composer.insert_pill(format!("session:{sixth}"), &format!("@{title}"));
    rt.state.composer.insert(" and ");
    rt.state.composer.focused = true;
    assert_eq!(rt.state.composer.text, typed_text);
    typed(&mut rt, "@exp");
    key(&mut rt, Key::Enter);
    key(&mut rt, Key::Enter);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let echo = rt.state.transcripts[SESSION].messages.iter().rev().find_map(|m| match m {
        opencode_kit::model::Message::User { id, text } if id.starts_with("msg_zzzzsent") => Some((id.clone(), text.clone())),
        _ => None,
    });
    let (id, text) = echo.unwrap();
    assert_eq!(text, format!("see @session:{sixth} and @explore"));
    let marks: Vec<_> = mention::user_mentions(&id).iter().map(|m| (text[m.range.clone()].to_owned(), m.kind)).collect();
    assert_eq!(marks, [("@explore".to_owned(), MentionKind::Agent)]);
    assert_eq!(shown(&rt, &format!("text-{id}")).unwrap().text.as_deref(), Some(format!("see {title} and @explore").as_str()));
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
    rt.state.transcripts.get_mut(SESSION).unwrap().messages.push(opencode_kit::model::Message::Note { id: "n-relayout".into(), text: String::new(), failed: false });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let plain = shown(&rt, &format!("text-{id}")).unwrap().rect[2];
    assert!(badged > plain + 8.0, "badged {badged} vs plain {plain}");
}

/// At the largest zoom a badge wider than the bubble wraps inside it, as Desktop wraps a long
/// mention, rather than running past its edge.
#[test]
fn a_badge_wider_than_the_bubble_wraps_inside_it_at_the_largest_zoom() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    app.route = Route::Session(SESSION.into());
    app.zoom = crate::shortcuts::ZOOM_MAX;
    // Its own message id: the mention store is shared with tests running beside it.
    app.sends = 700;
    // Big enough at 3× to keep the new bubble on screen, beside the sidebar.
    let mut rt = rune::Runtime::new(app, crate::view::app, (2400.0, 2400.0), rune::Env::new(true, 1.0));
    rt.on_key = Some(crate::on_key);
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    tap(&mut rt, "prompt");
    let label = "@Migrate config loader to the new schema";
    rt.state.composer = rune::TextEdit::new("");
    rt.state.composer.insert(&format!("as in {label} did"));
    key(&mut rt, Key::Enter);
    let id = "msg_zzzzsent0701";
    mention::set_user_mentions(id, vec![mention::UserMention { range: 6..6 + label.len(), kind: MentionKind::Agent }]);
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
    let r = shown(&rt, &format!("text-{id}")).expect("the bubble shows").rect;
    let glyphs: Vec<[f32; 4]> = rt
        .instances
        .iter()
        // The bubble's rows, from its left edge to the window's (the timeline is all there is).
        .filter(|i| i.params[1] == rune::render::KIND_GLYPH && i.rect[1] >= r[1] - 1.0 && i.rect[1] + i.rect[3] <= r[1] + r[3] + 1.0 && i.rect[0] >= r[0] - 1.0)
        .map(|i| i.rect)
        .collect();
    assert!(glyphs.len() > label.len() / 2, "the bubble's glyphs are drawn: {} in {r:?}", glyphs.len());
    let rows = glyphs.iter().map(|g| g[1].round() as i32).collect::<std::collections::BTreeSet<_>>();
    assert!(rows.len() >= 3, "the badge wraps: {rows:?}");
    let ends: Vec<(i32, f32)> = rows.iter().map(|&y| (y, glyphs.iter().filter(|g| g[1].round() as i32 == y).map(|g| g[0] + g[2]).fold(0.0, f32::max))).collect();
    for g in &glyphs {
        assert!(g[0] >= r[0] - 1.0 && g[0] + g[2] <= r[0] + r[2] + 1.0, "glyph {g:?} inside the bubble's text {r:?}; row ends {ends:?}");
    }
}
