use rune::Runtime;

use super::demo::{MIXED, PLAN, add_mixed, chunks};
use super::{Seg, find, inline, split};
use crate::App;
use crate::fixture_ids::MAIN;

const DT: f64 = 1.0 / 120.0;

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..240 {
        if !rt.frame(DT) {
            break;
        }
    }
}

fn fixture_texts() -> Vec<String> {
    let f: serde_json::Value = serde_json::from_str(crate::FIXTURE).expect("fixture");
    let mut out = Vec::new();
    let mut walk = |v: &serde_json::Value| {
        fn go(v: &serde_json::Value, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    if m.get("type").and_then(|t| t.as_str()) == Some("text")
                        && let Some(t) = m.get("text").and_then(|t| t.as_str())
                    {
                        out.push(t.to_owned());
                    }
                    m.values().for_each(|x| go(x, out));
                }
                serde_json::Value::Array(a) => a.iter().for_each(|x| go(x, out)),
                _ => {}
            }
        }
        go(v, &mut out);
    };
    walk(&f["messages"]);
    // The streamed reply, whole.
    out.extend(f["stream"]["events"].as_array().into_iter().flatten().filter_map(|e| e["data"]["text"].as_str().map(str::to_owned)));
    out
}

#[test]
fn replies_without_an_outline_take_todays_path() {
    let texts = fixture_texts();
    assert!(texts.len() >= 8, "fixture replies: {}", texts.len());
    for t in &texts {
        assert_eq!(split(t, false), None, "whole reply: {t:.60}");
        // Every prefix a stream could show, apart from a bare ``` held back for a delta until
        // its language says whether it's an outline.
        for (i, _) in t.char_indices() {
            let pre = &t[..i];
            if split(pre, true).is_some() {
                let tail = pre.rsplit('\n').next().unwrap_or(pre);
                assert_eq!(tail.trim_start(), "```", "only a bare fence is held: {pre:?}");
            }
        }
    }
}

#[test]
fn a_mixed_reply_cuts_into_markdown_and_outlines() {
    let segs = split(MIXED, false).expect("has outlines");
    let kinds: Vec<&str> = segs.iter().map(|s| if matches!(s, Seg::Md(_)) { "md" } else { "ui" }).collect();
    assert_eq!(kinds, ["md", "ui", "md", "ui", "md"]);
    for s in &segs {
        match s {
            Seg::Md(r) => assert!(!MIXED[r.clone()].contains("```ui"), "markdown run holds no outline"),
            Seg::Ui { at, nodes, .. } => {
                assert!(MIXED[at.clone()].starts_with("```ui\n") && MIXED[at.clone()].ends_with("```\n"));
                assert!(!nodes.is_empty());
            }
        }
    }
    // The code block and the diagram stay in the Markdown between the outlines.
    let Seg::Md(mid) = &segs[2] else { unreachable!() };
    assert!(MIXED[mid.clone()].contains("```ts") && MIXED[mid.clone()].contains("```mermaid"));
}

#[test]
fn the_demo_outlines_pass_the_checker() {
    for note in [MIXED, PLAN] {
        let issues = rune_parts::check(note, &rune_parts::BudgetProfile::CHAT);
        assert!(issues.is_empty(), "{issues:?}");
    }
}

#[test]
fn a_streaming_outline_only_grows_and_never_shows_its_fence() {
    let mut shown = String::new();
    let mut last: Vec<(usize, usize)> = Vec::new();
    for c in chunks(PLAN) {
        shown.push_str(&c);
        let Some(segs) = split(&shown, true) else {
            assert!(!shown.contains("```"), "an outline's start goes through genui: {shown:?}");
            continue;
        };
        let mut now = Vec::new();
        for s in &segs {
            match s {
                Seg::Md(r) => {
                    let md = &shown[r.clone()];
                    assert!(!md.contains("```"), "no fence in the Markdown around outlines: {md:?}");
                }
                Seg::Ui { line, nodes, .. } => now.push((*line, super::nodes_lines(nodes).len())),
            }
        }
        for (line, n) in &last {
            let grown = now.iter().find(|(l, _)| l == line).map(|x| x.1);
            assert!(grown.is_some_and(|g| g >= *n), "outline at line {line} kept its {n} lines: {now:?}");
        }
        last = now;
    }
    assert_eq!(last.len(), 2);
}

#[test]
fn find_matches_an_outlines_words_where_they_are_written() {
    let hits = find(MIXED, "drops under");
    assert_eq!(hits.len(), 1);
    assert_eq!(MIXED[hits[0][0].clone()].to_lowercase(), "drops under");
    let offline = find(MIXED, "offline");
    assert!(offline.len() >= 4, "prose and outline: {offline:?}");
    let mut prev = 0;
    for m in &offline {
        assert!(m[0].start >= prev, "in reading order");
        prev = m[0].start;
    }
    // A label, a speaker or a tone isn't a part's words.
    let Some(Seg::Ui { src_at, nodes, .. }) = split(MIXED, false).and_then(|s| s.into_iter().nth(3)) else { panic!() };
    let pieces: Vec<&str> = inline::pieces(MIXED, src_at, &nodes).into_iter().map(|p| p.1).collect();
    assert_eq!(pieces, ["14 passed", "first retry is immediate", "slow end-to-end test running"]);
}

#[test]
fn every_theme_colours_parts() {
    for (i, info) in crate::themes::all().iter().enumerate() {
        crate::themes::set_active(&info.id);
        for dark in [false, true] {
            let p = crate::palette::pal(dark);
            let t = super::theme(p);
            assert_ne!(t.text, t.bg, "theme {i} dark={dark}");
            assert_ne!(t.done, t.risk, "theme {i} dark={dark}");
        }
    }
    crate::themes::set_active(&crate::themes::all()[0].id);
}

#[test]
fn a_mixed_reply_draws_its_outlines_beside_code_and_mermaid() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, _| add_mixed(s, MAIN));
    settle(&mut rt);
    let els = rt.elements();
    let ui: Vec<_> = els.iter().filter(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("ui-"))).collect();
    assert_eq!(ui.len(), 2, "both outlines drawn");
    assert!(ui.iter().all(|e| e.rect[3] > 40.0), "{:?}", ui.iter().map(|e| e.rect).collect::<Vec<_>>());
    let texts: Vec<String> = els.iter().filter(|e| !e.exiting).filter_map(|e| e.text.clone()).collect();
    assert!(texts.iter().any(|t| t == "Drops under 1 s stay quiet"));
    assert!(!texts.iter().any(|t| t.contains("item {") || t.contains("```")), "no outline source drawn");
    assert!(texts.iter().any(|t| t.contains("shouldShowOffline(attempt")), "the code block still draws");
}

#[test]
fn find_paints_a_match_inside_an_outline() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, _| add_mixed(s, MAIN));
    settle(&mut rt);
    let mut mods = rune::Mods::default();
    mods.cmd = true;
    rt.key(rune::KeyEvent { key: rune::Key::Command('f'), mods });
    rt.frame(DT);
    for ch in "drops under".chars() {
        rt.key(rune::KeyEvent { key: rune::Key::Text(ch.to_string()), mods: rune::Mods::default() });
        rt.frame(DT);
    }
    settle(&mut rt);
    assert_eq!(rt.state.find.count(), 1);
    let mark = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("find-active")).expect("active match painted");
    assert!(mark.rect[2] > 10.0, "{mark:?}");
}

#[test]
fn a_streamed_outline_grows_without_its_source_showing() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, _| {
        s.finish_stream();
        s.route = crate::Route::Session(MAIN.into());
    });
    settle(&mut rt);
    let id = "msg_zzgenuitest";
    rt.update(|s, _| {
        for (ty, data) in [
            ("session.execution.started", serde_json::json!({ "sessionID": MAIN, "assistantMessageID": id })),
            ("session.step.started", serde_json::json!({ "sessionID": MAIN, "assistantMessageID": id, "agent": "build", "model": { "id": "m", "providerID": "p" } })),
            ("session.text.started", serde_json::json!({ "sessionID": MAIN, "assistantMessageID": id, "ordinal": 0 })),
        ] {
            s.apply_event(ty, &data);
        }
    });
    let mut height = 0.0f32;
    for c in chunks(PLAN) {
        rt.update(|s, _| {
            s.apply_event("session.text.delta", &serde_json::json!({ "sessionID": MAIN, "assistantMessageID": id, "ordinal": 0, "delta": c }));
        });
        for _ in 0..6 {
            rt.frame(DT);
        }
        let els = rt.elements();
        let raw = els.iter().filter(|e| !e.exiting).filter_map(|e| e.text.as_deref()).find(|t| t.contains("```") || t.starts_with("item ") || t.starts_with("box "));
        assert!(raw.is_none(), "outline source drawn: {raw:?}");
        if let Some(plan) = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some("ui-3")) {
            assert!(plan.rect[3] >= height - 0.5, "the plan only grows: {} after {height}", plan.rect[3]);
            height = height.max(plan.rect[3]);
        }
    }
    assert!(height > 100.0, "the plan drew: {height}");
}

#[test]
fn copying_a_whole_outline_copies_it_as_plain_lines() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, _| add_mixed(s, MAIN));
    settle(&mut rt);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let copy = |rt: &mut Runtime<App>| {
        let mods = rune::Mods { cmd: true, ..rune::Mods::default() };
        rt.key(rune::KeyEvent { key: rune::Key::Command('c'), mods });
        rt.frame(DT);
        clipboard.text().map(|t| t.to_string())
    };
    let Some(Seg::Ui { nodes, .. }) = split(MIXED, false).and_then(|s| s.into_iter().nth(1)) else { panic!() };
    rt.update(|_, tx| tx.select_all_text("ui-3"));
    rt.frame(DT);
    let whole = copy(&mut rt).expect("copied");
    assert_eq!(whole, rune_parts::to_plain(&nodes));
    assert!(whole.contains("Today\n") && whole.contains("  Drops under 1 s stay quiet\n"), "{whole}");
}

#[test]
fn a_drag_from_prose_into_an_outline_copies_both_as_drawn() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, _| add_mixed(s, MAIN));
    settle(&mut rt);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let is_prose = |e: &&rune::ElementInfo| e.text.as_deref().is_some_and(|t| t.starts_with("It flashes because"));
    for _ in 0..60 {
        let els = rt.elements();
        let prose = els.iter().find(is_prose).expect("prose drawn");
        if prose.visible[3] >= prose.rect[3] - 0.5 {
            break;
        }
        let (x, y) = (prose.rect[0] + 40.0, 300.0);
        drop(els);
        rt.wheel(x, y, 60.0);
        settle(&mut rt);
    }
    let els = rt.elements();
    let prose = els.iter().find(is_prose).expect("prose drawn");
    let quiet = els.iter().find(|e| e.text.as_deref() == Some("Drops under 1 s stay quiet")).expect("outline drawn");
    assert!(prose.visible[3] > 0.0 && quiet.visible[3] > 0.0, "both on screen: {:?} {:?} / {:?} {:?}", prose.rect, prose.visible, quiet.rect, quiet.visible);
    let from = (prose.rect[0] + 1.0, prose.rect[1] + 6.0);
    let to = (quiet.rect[0] + quiet.rect[2] - 1.0, quiet.rect[1] + quiet.rect[3] / 2.0);
    let mid = (prose.rect[0] + 60.0, from.1);
    drop(els);
    let mut drag = |rt: &mut Runtime<App>, to: (f32, f32)| {
        rt.pointer_move(from.0, from.1);
        rt.pointer_down(from.0, from.1);
        rt.frame(DT);
        rt.pointer_move(to.0, to.1);
        rt.frame(DT);
        rt.pointer_up(to.0, to.1);
        rt.frame(DT);
        let mods = rune::Mods { cmd: true, ..rune::Mods::default() };
        rt.key(rune::KeyEvent { key: rune::Key::Command('c'), mods });
        rt.frame(DT);
        let t = clipboard.text().map(|t| t.to_string()).expect("copied");
        rt.update(|_, tx| tx.clear_text_selection());
        rt.frame(DT);
        t
    };
    let words = drag(&mut rt, mid);
    assert!(words.starts_with("It fl") && !words.contains('\n'), "{words}");
    let both = drag(&mut rt, to);
    assert!(both.starts_with("It flashes because") && both.contains("Waiting for a second failure fixes it:"), "{both}");
    assert!(both.contains("Reconnecting") && both.ends_with("Drops under 1 s stay quiet"), "{both}");
}
