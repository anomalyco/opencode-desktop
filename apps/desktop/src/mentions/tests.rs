use super::*;
use rune::Runtime;

#[test]
fn the_trigger_follows_desktops_patterns() {
    assert_eq!(trigger("@", false), Some((0, "")));
    assert_eq!(trigger("ask @exp", false), Some((4, "exp")));
    assert_eq!(trigger("mail@exp", false), None);
    assert_eq!(trigger("@fix the", false), None);
    assert_eq!(trigger("@fix the", true), Some((0, "fix the")));
    assert_eq!(trigger("@fix the ", true), Some((0, "fix the ")));
    assert_eq!(trigger("@fix  the", true), None);
    assert_eq!(trigger("@ fix", true), None);
    assert_eq!(trigger("a\n@x", false), Some((2, "x")));
}

#[test]
fn session_titles_match_word_starts() {
    assert!(matches_session_title("Fix flaky reconnect", "fla rec"));
    assert!(matches_session_title("Fix flaky reconnect", "FIX"));
    assert!(!matches_session_title("Fix flaky reconnect", "fix rec"));
    assert!(!matches_session_title("HOME", "home "));
    assert!(matches_session_title("HOME base", "home "));
}

#[test]
fn fuzzy_prefers_substrings() {
    assert!(fuzzy("exp", "@explore").unwrap() > fuzzy("epl", "@explore").unwrap());
    assert!(fuzzy("xyz", "@explore").is_none());
}

#[test]
fn the_wire_rewrites_sessions_and_counts_utf16() {
    let text = "  é @explore then @Fix it and @opencode  ";
    let a = text.find("@explore").unwrap();
    let s = text.find("@Fix it").unwrap();
    let k = text.find("@opencode").unwrap();
    let pills = vec![
        (s..s + "@Fix it".len(), "session:ses_abc123".to_owned()),
        (a..a + "@explore".len(), "agent:explore".to_owned()),
        (k..k + "@opencode".len(), "skill:opencode".to_owned()),
    ];
    let w = wire(text, &pills, "/repo", |id| if id == "opencode" { "OpenCode".into() } else { id.into() });
    assert_eq!(w.text, "é @explore then @session:ses_abc123 and @opencode");
    assert_eq!(w.display, "é @explore then @Fix it and @opencode");
    let m = |start, end, t: &str| Some(Mention { start, end, text: t.into() });
    assert_eq!(w.agents, vec![AgentInput { name: "explore".into(), mention: m(2, 10, "@explore") }]);
    let k16 = "é @explore then @session:ses_abc123 and ".encode_utf16().count();
    assert_eq!(w.skills, vec![SkillInput { id: "opencode".into(), name: "OpenCode".into(), mention: m(k16, k16 + 9, "@opencode") }]);
}

#[test]
fn file_mentions_go_out_as_file_urls() {
    let text = "read @src/a b.rs and @/etc/hosts";
    let a = text.find("@src").unwrap();
    let e = text.find("@/etc").unwrap();
    let pills = vec![
        (a..a + "@src/a b.rs".len(), "file:src/a b.rs".to_owned()),
        (e..text.len(), "file:/etc/hosts".to_owned()),
    ];
    let w = wire(text, &pills, "/repo/", |id| id.into());
    let m = |start, end, t: &str| Some(Mention { start, end, text: t.into() });
    assert_eq!(
        w.files,
        vec![
            FileInput { uri: "file:///repo/src/a%20b.rs".into(), name: "a b.rs".into(), mime: Some("text/plain".into()), mention: m(5, 16, "@src/a b.rs") },
            FileInput { uri: "file:///etc/hosts".into(), name: "hosts".into(), mime: Some("text/plain".into()), mention: m(21, 32, "@/etc/hosts") },
        ]
    );
    assert_eq!(w.text, text);
    assert_eq!(encode_path("C:\\x\\é#.rs"), "/C:/x/%C3%A9%23.rs");
    assert_eq!(filename("src/stream/"), "stream");
    assert_eq!(absolute("/repo", "C:/x"), "C:/x");
}

pub(crate) use crate::fixture_runtime as runtime;
pub(crate) fn key(rt: &mut Runtime<App>, key: Key) {
    rt.key(KeyEvent { key, mods: Default::default() });
    rt.frame(1.0 / 60.0);
}
/// One keystroke a frame, as a person types.
pub(crate) fn typed(rt: &mut Runtime<App>, s: &str) {
    for c in s.chars() {
        key(rt, Key::Text(c.to_string()));
    }
}
pub(crate) fn tap(rt: &mut Runtime<App>, target: &str) {
    let (x, y) = rt.find(target).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}
fn keys(rt: &Runtime<App>) -> Vec<String> {
    rt.state.mention_items().into_iter().map(|i| i.key).collect()
}
pub(crate) fn shown(rt: &Runtime<App>, key: &str) -> Option<rune::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

#[test]
fn at_lists_skills_agents_and_sessions_and_filters() {
    let mut rt = runtime();
    tap(&mut rt, "prompt");
    typed(&mut rt, "ask @");
    let popover = shown(&rt, "mentions").expect("the popover opens on @");
    assert!((popover.opacity - 1.0).abs() < 1e-3, "no fade");
    let composer = shown(&rt, "composer").unwrap();
    assert!((popover.target[1] + popover.target[3] + 8.0 - composer.target[1]).abs() < 0.1, "8 points above");
    assert!((popover.target[2] - composer.target[2]).abs() < 0.1, "full width");
    let all = keys(&rt);
    assert_eq!(&all[..3], ["skill:opencode", "agent:general", "agent:explore"]);
    // The open session isn't offered; the five newest others are.
    assert_eq!(all.iter().filter(|k| k.starts_with("session:")).count(), 5);
    assert!(!all.contains(&"session:ses_fx0001aaaaaaaaaaaaaaaaaa".to_owned()));
    assert_eq!(rt.state.mention_active(&rt.state.mention_items()).as_deref(), Some("skill:opencode"));

    typed(&mut rt, "exp");
    let found = keys(&rt);
    assert_eq!(found[0], "agent:explore");
    assert!(found[1..].iter().all(|k| k.starts_with("session:")), "{found:?}");
    assert!(found.contains(&"session:ses_fx0008aaaaaaaaaaaaaaaaaa".to_owned()), "\"Explain …\" starts with exp");
    typed(&mut rt, "zzz");
    assert!(keys(&rt).is_empty());
    assert!(shown(&rt, "mentions").is_some(), "an empty list still shows its label");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.composer.text, "ask @expzzz", "Enter with nothing to pick neither picks nor sends");
    assert_eq!(rt.state.sends, 0);
    key(&mut rt, Key::Escape);
    assert!(shown(&rt, "mentions").is_none());
    assert!(rt.state.composer.focused);
}

#[test]
fn filtering_and_hover_land_in_one_frame() {
    let mut rt = runtime();
    tap(&mut rt, "prompt");
    let settled = |rt: &Runtime<App>, when: &str| {
        for e in rt.elements() {
            let Some(k) = e.key.as_deref() else { continue };
            if k == "mentions" || k.starts_with("mention-") {
                let moved = e.rect.iter().zip(e.target.iter()).any(|(a, b)| (a - b).abs() > 0.01);
                assert!(!moved && (e.opacity - 1.0).abs() < 1e-3, "{k} after {when}: rect {:?} target {:?} opacity {}", e.rect, e.target, e.opacity);
            }
        }
    };
    for c in "@exp".chars() {
        key(&mut rt, Key::Text(c.to_string()));
        settled(&rt, &format!("typing {c}"));
    }
    key(&mut rt, Key::Backspace);
    settled(&rt, "backspace");
    let items = keys(&rt);
    let (a, b) = (format!("mention-{}", items[0]), format!("mention-{}", items[1]));
    let (x, y) = rt.find(&a).unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    let (x, y) = rt.find(&b).unwrap();
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert!(shown(&rt, &a).unwrap().fill.a < 1e-3, "the old row clears at once");
    let lit = shown(&rt, &b).unwrap().fill.a;
    rt.frame(1.0 / 60.0);
    assert!(lit > 0.0 && (lit - shown(&rt, &b).unwrap().fill.a).abs() < 1e-3, "the new row lights at once");
}

#[test]
fn row_text_stays_inside_its_row() {
    let mut rt = runtime();
    let long = "Find and complete requested reviews and items across every account, with receipts and notes";
    rt.state.mentions.skills = ["opencode", "opentui", "opencode-drive", "opencode-session-control", "loop-me", "psychopomp", "clean-copy", "fieldwork-operate", "computer-use", "to-spec", "ramp-complete-tasks", "ramp-complete-expenses", "prototype", "fieldwork-prove", "ramp-incorporate-with-ramp", "animation-principles", "google-workspace"]
        .iter()
        .map(|id| Skill { id: (*id).into(), name: (*id).into(), description: format!("{id}: {long}\nsecond line") })
        .collect();
    tap(&mut rt, "prompt");
    let check = |rt: &Runtime<App>, when: &str| {
        let els = rt.elements();
        for (i, e) in els.iter().enumerate() {
            if e.role != "text" {
                continue;
            }
            let mut p = e.parent;
            while let Some(j) = p {
                if els[j].key.as_deref().is_some_and(|k| k.starts_with("mention-")) {
                    let r = els[j].rect;
                    assert!(e.rect[1] >= r[1] - 0.5 && e.rect[1] + e.rect[3] <= r[1] + r[3] + 0.5, "{when}: {:?} at {:?} outside its row {:?} {:?}", e.text, e.rect, els[j].key, r);
                    break;
                }
                p = els[j].parent;
            }
            let _ = i;
        }
    };
    for c in "@ope".chars() {
        key(&mut rt, Key::Text(c.to_string()));
        check(&rt, &format!("typing {c}"));
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        check(&rt, &format!("after {c}"));
    }
    for _ in 0..12 {
        key(&mut rt, Key::Down);
    }
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    check(&rt, "scrolled");
    key(&mut rt, Key::Backspace);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    check(&rt, "backspace");
}

#[test]
fn multi_word_queries_list_sessions_only() {
    let mut rt = runtime();
    tap(&mut rt, "prompt");
    typed(&mut rt, "@prof");
    assert_eq!(keys(&rt), ["session:ses_fx0005aaaaaaaaaaaaaaaaaa"]);
    typed(&mut rt, " scr");
    assert_eq!(keys(&rt), ["session:ses_fx0005aaaaaaaaaaaaaaaaaa"]);
    typed(&mut rt, "x");
    assert!(keys(&rt).is_empty());
    // Closed, a space ends the mention.
    key(&mut rt, Key::Escape);
    typed(&mut rt, " ");
    assert!(!rt.state.mentions.open);
}

#[test]
fn keys_move_wrap_and_pick_into_a_pill() {
    let mut rt = runtime();
    tap(&mut rt, "prompt");
    typed(&mut rt, "hi @");
    let all = keys(&rt);
    key(&mut rt, Key::Up);
    assert_eq!(rt.state.mention_active(&rt.state.mention_items()).as_deref(), Some(all.last().unwrap().as_str()), "up wraps");
    key(&mut rt, Key::Down);
    rt.key(KeyEvent { key: Key::Text("n".into()), mods: rune::Mods { ctrl: true, ..Default::default() } });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.mention_active(&rt.state.mention_items()).as_deref(), Some("agent:general"));
    key(&mut rt, Key::Tab);
    assert_eq!(rt.state.composer.text, "hi @general ");
    assert!(!rt.state.mentions.open);
    assert_eq!(rt.state.composer.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(), [(3..11, "agent:general".to_owned())]);
    assert_eq!(rt.state.composer.focus, 12);
    assert!(rt.state.pickers.toolbar_focus.is_none(), "Tab picked instead of moving to the toolbar");

    // Backspace takes the space, then the whole pill, and never reopens the popover on it.
    key(&mut rt, Key::Backspace);
    assert!(!rt.state.mentions.open);
    key(&mut rt, Key::Backspace);
    assert_eq!(rt.state.composer.text, "hi ");

    typed(&mut rt, "@fix foo");
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.composer.text, "hi @Fix footnote numbering in exported PDFs ");
    assert_eq!(rt.state.composer.pills().next().map(|(_, id)| id.to_owned()).as_deref(), Some("session:ses_fx0010aaaaaaaaaaaaaaaaaa"));
}

#[test]
fn files_follow_sessions_and_pick_into_a_pill() {
    let mut rt = runtime();
    tap(&mut rt, "prompt");
    typed(&mut rt, "@");
    assert!(keys(&rt).iter().all(|k| !k.starts_with("file:")), "no file search without a query");
    typed(&mut rt, "rec");
    let found = keys(&rt);
    let first_file = found.iter().position(|k| k.starts_with("file:")).expect("files listed");
    assert!(found[..first_file].iter().all(|k| !k.starts_with("file:")));
    assert!(found[first_file..].iter().all(|k| k.starts_with("file:")), "{found:?}");
    assert_eq!(found[first_file], "file:src/stream/reconnect.rs");
    typed(&mut rt, "onnect.rs");
    assert_eq!(keys(&rt), ["file:src/stream/reconnect.rs"]);
    let row = shown(&rt, "mention-file:src/stream/reconnect.rs").expect("the row shows");
    assert!(row.target[3] > 0.0);
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.composer.text, "@src/stream/reconnect.rs ");
    let w = rt.state.mention_wire();
    assert_eq!(
        w.files,
        vec![FileInput {
            uri: "file:///Users/demo/code/lumen/src/stream/reconnect.rs".into(),
            name: "reconnect.rs".into(),
            mime: Some("text/plain".into()),
            mention: Some(Mention { start: 0, end: 24, text: "@src/stream/reconnect.rs".into() }),
        }]
    );
}

#[test]
fn sending_carries_the_mentions() {
    let mut rt = runtime();
    rt.state.route = Route::Session(crate::fixture_ids::THIRD.into());
    rt.frame(1.0 / 60.0);
    tap(&mut rt, "prompt");
    typed(&mut rt, "@exp");
    key(&mut rt, Key::Enter);
    typed(&mut rt, "look at @opencode");
    key(&mut rt, Key::Enter);
    typed(&mut rt, "and @fix flaky");
    key(&mut rt, Key::Enter);
    let w = rt.state.mention_wire();
    assert_eq!(w.text, "@explore look at @opencode and @session:ses_fx0001aaaaaaaaaaaaaaaaaa");
    assert_eq!(w.display, "@explore look at @opencode and @Fix flaky reconnect in the event stream");
    assert_eq!(w.agents, vec![AgentInput { name: "explore".into(), mention: Some(Mention { start: 0, end: 8, text: "@explore".into() }) }]);
    assert_eq!(w.skills, vec![SkillInput { id: "opencode".into(), name: "OpenCode".into(), mention: Some(Mention { start: 17, end: 26, text: "@opencode".into() }) }]);
    key(&mut rt, Key::Enter);
    assert_eq!(rt.state.sends, 1);
    assert!(rt.state.composer.text.is_empty());
    let t = &rt.state.transcripts[crate::fixture_ids::THIRD];
    assert!(matches!(t.messages.last(), Some(opencode_kit::model::Message::User { text, .. }) if text.ends_with("@Fix flaky reconnect in the event stream")), "the echo shows what was typed");
}

#[test]
fn popovers_follow_the_text_size() {
    let row = |rt: &Runtime<App>| shown(rt, "mention-skill:opencode").unwrap();
    let mut rt = runtime();
    tap(&mut rt, "prompt");
    typed(&mut rt, "ask @");
    let small = row(&rt);
    assert!((small.scale - 1.0).abs() < 1e-3);
    let mut rt = runtime();
    rt.state.zoom = 1.6;
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    tap(&mut rt, "prompt");
    typed(&mut rt, "ask @");
    rt.frame(1.0 / 60.0);
    let (big, popover, composer) = (row(&rt), shown(&rt, "mentions").unwrap(), shown(&rt, "composer").unwrap());
    assert!((big.scale - 1.6).abs() < 1e-3, "rows scale with zoom: {}", big.scale);
    assert!((big.rect[3] / small.rect[3] - 1.6).abs() < 0.01, "row height {} vs {}", big.rect[3], small.rect[3]);
    assert!((popover.rect[2] - composer.rect[2]).abs() < 0.5, "still the composer's width: {:?} vs {:?}", popover.rect, composer.rect);
    assert!((popover.rect[1] + popover.rect[3] + 8.0 * 1.6 - composer.rect[1]).abs() < 0.5, "still 8 page points above");
}
