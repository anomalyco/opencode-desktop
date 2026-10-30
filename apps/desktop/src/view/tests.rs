use opencode_kit::client::Incoming;
use opencode_kit::model::Message;
use rune::{Env, Key, KeyEvent, Runtime, TextEdit};

use crate::App;

#[test]
fn lazy_timeline_keeps_visible_rows_and_builds_only_the_window() {
    fn scene(s: &App, _: &mut rune::Cx<App>) -> rune::El {
        let crate::Route::Session(id) = &s.route else { unreachable!() };
        rune::col().w_full().h_full().child(super::timeline(s, super::pal(true), id, s.transcripts.get(id), 0.0))
    }
    fn visible(rt: &Runtime<App>) -> Vec<(String, Option<String>, [f32; 4])> {
        rt.elements().into_iter().filter(|e| e.visible[3] > 0.0 && !e.exiting)
            .map(|e| (e.path, e.text, e.rect)).collect()
    }
    let state = || {
        let mut s = crate::perf::fixture();
        let mut other = s.transcripts["bench"].clone();
        other.session = "other".into();
        s.transcripts.insert("other".into(), other);
        s.reduced_motion = true;
        s
    };
    super::EAGER_ROWS.set(true);
    let mut eager = Runtime::new(state(), scene, (800.0, 600.0), Env::new(true, 1.0));
    eager.frame(0.0);
    super::EAGER_ROWS.set(false);
    super::ROW_BUILDS.set(0);
    let mut lazy = Runtime::new(state(), scene, (800.0, 600.0), Env::new(true, 1.0));
    lazy.frame(0.0);
    assert!(super::ROW_BUILDS.get() < 100, "2402 rows must not build at open");
    // Compare at rest: initial content, a streamed word, a group expanded, a new prompt,
    // and a session switch. Each has the same keys, content and visible layout.
    for case in 0..5 {
        for (is_eager, rt) in [(true, &mut eager), (false, &mut lazy)] {
            super::EAGER_ROWS.set(is_eager);
            rt.update(|s, tx| {
                tx.settle();
                match case {
                    1 => { s.apply_event("session.text.delta", &serde_json::json!({"sessionID":"bench", "assistantMessageID":"stream", "ordinal":0, "delta":" more text"})); }
                    2 => { s.expanded.insert("c-a0799-0".into()); }
                    3 => { s.apply_event("session.prompted", &serde_json::json!({"sessionID":"bench", "messageID":"new", "text":"Next question"})); }
                    4 => s.route = crate::Route::Session("other".into()),
                    _ => {}
                }
            });
            for _ in 0..120 { if !rt.frame(1.0 / 120.0) { break; } }
        }
        let (a, b) = (visible(&eager), visible(&lazy));
        assert_eq!(a.len(), b.len(), "case {case}: visible elements");
        for ((ak, at, ar), (bk, bt, br)) in a.iter().zip(&b) {
            assert_eq!((ak, at), (bk, bt), "case {case}: identity and text");
            for (a, b) in ar.iter().zip(br) { assert!((a-b).abs() < 0.1, "case {case}, {ak}: {ar:?} != {br:?}"); }
        }
    }
    super::EAGER_ROWS.set(false);
}

#[test]
fn the_new_session_page_picks_its_project() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    app.draft_dir = "/Users/demo/code/lumen/src".into();
    app.route = crate::Route::Draft;
    let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tap = |rt: &mut Runtime<App>, key: &str| {
        let e = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap();
        let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    };
    assert_eq!(rt.state.draft_project().map(|p| p.name.as_str()), Some("lumen"));
    tap(&mut rt, "project");
    assert_eq!(rt.state.pickers.open, Some(crate::pickers::Kind::Project));
    let harbor = rt.state.projects.iter().find(|p| p.name == "harbor").unwrap().id.clone();
    tap(&mut rt, &format!("option-{harbor}"));
    assert_eq!(rt.state.draft_dir, "/Users/demo/code/harbor");
    assert_eq!(rt.state.pickers.open, None);
}

#[test]
fn only_mounted_image_rows_fetch_their_media() {
    use opencode_kit::model::{self, Assistant, Attachment, Message, Part, ToolStatus, Transcript};
    use std::sync::{Arc, Mutex};
    fn scene(s: &App, _: &mut rune::Cx<App>) -> rune::El {
        rune::col().w_full().h_full().child(super::timeline(s, super::pal(true), "lazy-media", s.transcripts.get("lazy-media"), 0.0))
    }
    let image = |c: char, name: &str| Attachment { mime: "image/png".into(), name: name.into(), media: Some(c.to_string().repeat(64)), ..Default::default() };
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = calls.clone();
    let mut app = App::empty();
    app.attach.set_fetch(Arc::new(move |hash: &str| {
        seen.lock().unwrap().push(hash.to_owned());
        Err("stub".into())
    }));
    // A prompt's image and a read tool's image far above, and a prompt's image at the bottom.
    let mut messages = vec![
        Message::User { id: "msg_lazy_first".into(), text: "look".into() },
        Message::Assistant(Assistant {
            id: "msg_lazy_read".into(),
            completed: Some(1),
            parts: vec![Part::Tool {
                id: "call_lazy_read".into(),
                name: "read".into(),
                status: ToolStatus::Completed,
                title: "a.png".into(),
                input: serde_json::Value::Null,
                files: vec![image('b', "a.png")],
                metadata: serde_json::Value::Null,
            }],
            ..Default::default()
        }),
    ];
    messages.extend((0..200).map(|i| Message::User { id: format!("msg_lazy_{i:03}"), text: format!("filler {i}") }));
    messages.push(Message::User { id: "msg_lazy_last".into(), text: "and this".into() });
    model::set_user_files("msg_lazy_first", vec![image('a', "first.png")]);
    model::set_user_files("msg_lazy_last", vec![image('c', "last.png")]);
    app.transcripts.insert("lazy-media".into(), Transcript { session: "lazy-media".into(), messages });
    app.route = crate::Route::Session("lazy-media".into());
    app.reduced_motion = true;
    let mut rt = Runtime::new(app, scene, (800.0, 600.0), Env::new(true, 1.0));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    model::set_user_files("msg_lazy_first", Vec::new());
    model::set_user_files("msg_lazy_last", Vec::new());
    assert!(rt.elements().iter().any(|e| e.path.contains("ui-msg_lazy_last")), "the bottom image row mounts");
    assert!(!rt.elements().iter().any(|e| e.path.contains("ti-call_lazy_read")), "the read tool's row is offscreen");
    for _ in 0..200 {
        if !calls.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // Room for any other fetch to show up.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(*calls.lock().unwrap(), ["c".repeat(64)], "only the mounted row fetched");
}

#[test]
fn a_link_in_a_message_opens() {
    fn view(_: &App, cx: &mut rune::Cx<App>) -> rune::El {
        let p = super::pal(cx.system_dark());
        rune::col().w_full().child(super::markdown_view(&p, "[openthislink](https://example.com/pr/1)", 0.0, false))
    }
    let mut rt = Runtime::new(App::empty(), view, (600.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let link = rt.elements().into_iter().find(|e| e.text.as_deref().is_some_and(|t| t.contains("openthislink"))).unwrap();
    let (x, y) = (link.rect[0] + 8.0, link.rect[1] + link.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.take_url_requests(), ["https://example.com/pr/1"]);
}

#[test]
fn a_tool_joining_the_group_slides_its_chevron() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, tx| {
        s.composer = TextEdit::new("Can you also check the reconnect jitter?");
        s.send(tx);
    });
    // The reply reads a file, then greps: "Used 1 Read", then "Used 2 Read, Grep".
    let chevron = |rt: &Runtime<App>| rt.elements().into_iter()
        .find(|e| !e.exiting && e.path.contains("c-call_reply_read-0001/") && e.key.as_deref() == Some("chevron"))
        .map(|e| e.rect[0]);
    let mut xs = Vec::new();
    for _ in 0..(4.5 * 120.0) as usize {
        rt.frame(1.0 / 120.0);
        xs.extend(chevron(&rt));
    }
    let (lo, hi) = xs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    assert!(hi - lo > 4.0, "the header changes width: {lo}..{hi}");
    let step = xs.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    // A spring, not a jump: no frame covers more than a tenth of the way.
    assert!(step < (hi - lo) / 10.0, "the chevron moves at most {step} pt in a frame");
}

fn settled(frames: usize) -> Runtime<App> {
    let mut rt = crate::fixture_runtime();
    for _ in 0..frames {
        rt.frame(1.0 / 60.0);
    }
    rt
}

fn session() -> Runtime<App> {
    settled(60)
}

/// A message row's bubble fill (drawn and clipped rects), its text's visible height, and
/// its glow's opacity, if it has one this frame.
struct Bubble {
    rect: [f32; 4],
    text: [f32; 4],
    visible_h: f32,
    text_h: f32,
    glow: Option<f32>,
}

fn bubble(rt: &Runtime<App>, id: &str) -> Option<Bubble> {
    let els = rt.elements();
    let row = format!("u-{id}");
    let inside = |e: &&rune::ElementInfo| !e.exiting && e.path.split('/').any(|p| p == row);
    let fill = els.iter().filter(inside).find(|e| e.key.as_deref() == Some("bubble"))?;
    let text = els.iter().filter(inside).find(|e| e.key.as_deref() == Some(&format!("text-{id}")))?;
    let glow = els.iter().filter(inside).find(|e| e.key.as_deref() == Some("glow")).map(|e| e.opacity);
    Some(Bubble { rect: fill.rect, text: text.rect, visible_h: fill.visible[3], text_h: text.visible[3], glow })
}

fn deliver(rt: &mut Runtime<App>, messages: Vec<Message>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let session = rt.state.transcripts.keys().find(|k| rt.state.route == crate::Route::Session((*k).clone())).unwrap().clone();
    tx.send(Incoming::Messages { session, messages }).unwrap();
    rt.state.inbox = Some(rx);
}

#[derive(Clone, Copy, Debug)]
enum Via {
    Enter,
    Click,
}

/// Sends through real input (focus, type, then Enter or a click on Send) and checks every
/// frame, `dts` giving each frame's length (dropped frames included).
fn send_and_trace(via: Via, dts: &[f64]) {
    let mut rt = session();
    let body = "Can you also check the reconnect jitter?";
    let tap = |rt: &mut Runtime<App>, key: &str| {
        let (x, y) = rt.find(key).unwrap();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    };
    tap(&mut rt, "prompt");
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Text(body.into()), mods: Default::default() });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let typed = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("prompt")).unwrap().rect;
    match via {
        Via::Enter => rt.key(KeyEvent { key: Key::Enter, mods: Default::default() }),
        Via::Click => tap(&mut rt, "submit"),
    }
    let id = "msg_zzzzsent0001";
    let mut texts = Vec::new();
    let working = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("working")).map(|e| e.rect[1]);
    let (mut ys, mut work, mut worst) = (Vec::new(), Vec::new(), 0.0f64);
    let mut t = 0.0;
    for (f, &dt) in dts.iter().enumerate() {
        let start = std::time::Instant::now();
        rt.frame(dt);
        worst = worst.max(start.elapsed().as_secs_f64());
        t += dt;
        if f == dts.len() / 2 {
            // The server's copy of the prompt replaces the local one under the same id.
            deliver(&mut rt, vec![Message::User { id: id.into(), text: body.into() }]);
        }
        assert!(rt.state.composer.text.is_empty(), "{via:?}: the composer clears");
        let b = bubble(&rt, id).unwrap_or_else(|| panic!("{via:?} frame {f}: the message has a bubble"));
        assert!(b.text_h <= 0.0 || b.visible_h > 0.0, "{via:?} frame {f}: text shows without its bubble: {:?} {} {}", b.rect, b.visible_h, b.text_h);
        assert!(b.glow.is_none(), "{via:?} frame {f}: this app's own send doesn't glow");
        let rows: Vec<String> = rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.key).filter(|k| k.starts_with("u-")).collect();
        assert_eq!(rows.iter().filter(|k| *k == &format!("u-{id}")).count(), 1, "frame {f}: one row, same key");
        assert!(rows.iter().collect::<std::collections::HashSet<_>>().len() == rows.len(), "frame {f}: duplicate row keys {rows:?}");
        ys.push((t, b.rect[1]));
        texts.push(b.text);
        work.extend(working(&rt).map(|y| (t, y)));
    }
    eprintln!("{via:?}: bubble y per frame {:?}", ys.iter().map(|p| (p.1 * 10.0).round() / 10.0).collect::<Vec<_>>());
    eprintln!("{via:?}: slowest frame {:.1} ms", worst * 1000.0);
    // It leaves from the composer: its words start where they were typed, at full size.
    // The first frame is a frame into the flight: a few points at 120 Hz.
    let (first, last) = (texts[0], texts[texts.len() - 1]);
    let near = if dts[0] < 0.01 { 5.0 } else { 15.0 };
    assert!((first[0] - typed[0]).abs() < near && (first[1] - typed[1]).abs() < near,
        "{via:?}: the first frame's words sit on the typed ones: {first:?} vs {typed:?}");
    for (f, r) in texts.iter().enumerate() {
        assert!((r[2] - last[2]).abs() < 0.5 && (r[3] - last[3]).abs() < 0.5, "{via:?} frame {f}: the words keep their size: {r:?} vs {last:?}");
    }
    // Then it flies up over several frames, not in one.
    let land = ys.iter().position(|y| (y.1 - ys[ys.len() - 1].1).abs() < 0.5).unwrap();
    assert!(ys[land].0 > 0.25 && ys[land].0 < 0.7, "{via:?}: lands in {:.2} s: {ys:?}", ys[land].0);
    let rise = ys[0].1 - ys[ys.len() - 1].1;
    assert!(rise > 40.0, "{via:?}: it rises into place: {ys:?}");
    // One smooth flight: never back, and never a tenth of the way in a 1/120 s frame (a
    // snap would be all of it in one frame).
    for w in ys.windows(2) {
        let speed = (w[0].1 - w[1].1) / (w[1].0 - w[0].0) as f32;
        assert!(w[1].1 <= w[0].1 + 0.01 && speed < rise * 12.0, "{via:?}: bubble y {ys:?}");
    }
    assert!(work.len() == ys.len(), "{via:?}: Working shows below it throughout");
    for w in work.windows(2) {
        assert!((w[1].1 - w[0].1).abs() / ((w[1].0 - w[0].0) as f32) < 700.0, "{via:?}: Working moves smoothly: {work:?}");
    }
}

#[test]
fn a_sent_message_rises_whole_from_the_composer_and_keeps_its_key() {
    send_and_trace(Via::Enter, &[1.0 / 120.0; 90]);
    send_and_trace(Via::Click, &[1.0 / 120.0; 90]);
    // A dev build that drops frames: 60 Hz with two 50 ms stalls stays on the same path.
    let mut dts = vec![1.0 / 60.0; 45];
    dts[3] = 0.05;
    dts[10] = 0.05;
    send_and_trace(Via::Enter, &dts);
}

/// Read further up, nothing flies: the composer clears and the timeline cuts to its end, where
/// the message already stands and stays put.
#[test]
fn a_message_sent_while_reading_up_does_not_fly() {
    let mut rt = session();
    let (x, y) = rt.find("prompt").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.key(KeyEvent { key: Key::Text("Can you also check the reconnect jitter?".into()), mods: Default::default() });
    let scroll = rt.elements().into_iter().find(|e| e.key.as_deref().is_some_and(|k| k.starts_with("scroll-ses_"))).unwrap().rect;
    rt.wheel(scroll[0] + scroll[2] / 2.0, scroll[1] + scroll[3] / 2.0, 320.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let crate::Route::Session(sid) = rt.state.route.clone() else { panic!("no open session") };
    assert!(!rt.state.ends.at_end(&sid), "scrolled up off the end");
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
    let id = "msg_zzzzsent0001";
    rt.frame(1.0 / 120.0);
    let first = bubble(&rt, id).expect("the message joins the end").rect;
    for f in 1..90 {
        rt.frame(1.0 / 120.0);
        assert!(rt.state.composer.text.is_empty(), "frame {f}: the composer clears");
        let b = bubble(&rt, id).unwrap_or_else(|| panic!("frame {f}: the message stays at the end"));
        assert!(b.visible_h > 0.0, "frame {f}: it shows at once: {:?}", b.rect);
        assert!((b.rect[1] - first[1]).abs() < 0.5, "frame {f}: it doesn't fly: {:?} from {first:?}", b.rect);
    }
    assert!(rt.state.ends.at_end(&sid), "the timeline shows its end");
}

/// A run of several steps (thinking and a tool, a new assistant message that answers, then
/// another tool step): Working stays mounted at full opacity from the run's start to its end,
/// through the gaps between one step ending and the next starting.
#[test]
fn working_stays_up_between_steps_until_the_run_ends() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    let crate::Route::Session(sid) = app.route.clone() else { panic!("no open session") };
    let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let ev = |t: f64, ty: &str, step: &str, more: serde_json::Value| {
        let mut d = serde_json::json!({ "sessionID": sid, "assistantMessageID": step });
        d.as_object_mut().unwrap().extend(more.as_object().cloned().unwrap_or_default());
        (t, ty.to_owned(), d)
    };
    let none = serde_json::Value::Null;
    let tool = |id: &str| serde_json::json!({ "id": id, "name": "read", "input": { "filePath": "src/backoff.ts" } });
    let mut script = vec![
        ev(0.10, "session.execution.started", "msg_zzzz_step1", none.clone()),
        ev(0.10, "session.step.started", "msg_zzzz_step1", none.clone()),
        ev(0.20, "session.reasoning.started", "msg_zzzz_step1", serde_json::json!({ "ordinal": 0 })),
        ev(0.25, "session.reasoning.delta", "msg_zzzz_step1", serde_json::json!({ "ordinal": 0, "delta": "Read it first." })),
        ev(0.40, "session.reasoning.ended", "msg_zzzz_step1", serde_json::json!({ "ordinal": 0 })),
        ev(0.50, "session.tool.input.started", "msg_zzzz_step1", tool("call_zzzz_1")),
        ev(0.55, "session.tool.called", "msg_zzzz_step1", tool("call_zzzz_1")),
        ev(0.80, "session.tool.success", "msg_zzzz_step1", tool("call_zzzz_1")),
        ev(0.85, "session.step.ended", "msg_zzzz_step1", serde_json::json!({ "finish": "tool-calls" })),
        // The next step's message arrives a beat later.
        ev(0.95, "session.step.started", "msg_zzzz_step2", none.clone()),
        ev(1.00, "session.text.started", "msg_zzzz_step2", serde_json::json!({ "ordinal": 0 })),
    ];
    for (k, word) in "The backoff resets on every message, so ".split_inclusive(' ').enumerate() {
        script.push(ev(1.05 + k as f64 * 0.04, "session.text.delta", "msg_zzzz_step2", serde_json::json!({ "ordinal": 0, "delta": word })));
    }
    script.extend([
        ev(1.50, "session.text.ended", "msg_zzzz_step2", serde_json::json!({ "ordinal": 0 })),
        ev(1.55, "session.tool.input.started", "msg_zzzz_step2", tool("call_zzzz_2")),
        ev(1.60, "session.tool.called", "msg_zzzz_step2", tool("call_zzzz_2")),
        ev(1.90, "session.tool.success", "msg_zzzz_step2", tool("call_zzzz_2")),
        ev(1.95, "session.step.ended", "msg_zzzz_step2", serde_json::json!({ "finish": "tool-calls" })),
        ev(2.20, "session.step.started", "msg_zzzz_step3", none.clone()),
        ev(2.25, "session.text.started", "msg_zzzz_step3", serde_json::json!({ "ordinal": 0 })),
        ev(2.30, "session.text.delta", "msg_zzzz_step3", serde_json::json!({ "ordinal": 0, "delta": "Done." })),
        ev(2.40, "session.text.ended", "msg_zzzz_step3", serde_json::json!({ "ordinal": 0, "text": "Done." })),
        ev(2.50, "session.step.ended", "msg_zzzz_step3", serde_json::json!({ "finish": "stop" })),
        ev(2.50, "session.execution.succeeded", "msg_zzzz_step3", none.clone()),
    ]);
    let working = |rt: &Runtime<App>| rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("working")).map(|e| (e.exiting, e.opacity)).collect::<Vec<_>>();
    let (dt, mut t, mut next) = (1.0 / 120.0, 0.0, 0);
    while t < 3.0 {
        while next < script.len() && script[next].0 <= t {
            let (_, ty, d) = script[next].clone();
            rt.update(move |s, _| {
                s.apply_event(&ty, &d);
            });
            next += 1;
        }
        rt.frame(dt);
        t += dt;
        let w = working(&rt);
        if t > 0.1 && t < 2.5 {
            assert!(matches!(w[..], [(false, o)] if o > 0.999), "{t:.3}s: Working is one mounted row at opacity 1: {w:?}");
            // The shimmer's lit copy sits exactly on the label while its window sweeps.
            let els = rt.elements();
            let label = els.iter().find(|e| e.path.contains("/working/") && e.key.as_deref() == Some("label")).unwrap();
            let lit = els.iter().find(|e| e.path.contains("/lit/") && e.text.is_some()).unwrap();
            let (sheen, lit_box) = (els.iter().find(|e| e.key.as_deref() == Some("sheen")).unwrap(), els.iter().find(|e| e.key.as_deref() == Some("lit")).unwrap());
            assert!((label.rect[2] - crate::working::width("Working")).abs() < 2.0, "the label is {} wide", label.rect[2]);
            assert!((lit.rect[0] - label.rect[0]).abs() < 0.01 && (lit.rect[1] - label.rect[1]).abs() < 0.01, "{t:.3}s: lit {:?} on label {:?}", lit.rect, label.rect);
            assert_eq!(sheen.loop_phase, lit_box.loop_phase, "{t:.3}s: one sweep");
        }
    }
    assert!(working(&rt).iter().all(|&(_, o)| o < 0.001), "the finished run hides Working");
}

#[test]
fn a_message_from_elsewhere_glows_and_settles_and_a_big_one_glides_in() {
    let mut rt = session();
    let id = "msg_zzzzzelsewhere";
    let body = (0..14).map(|i| format!("Line {i} of a long message typed on another device.")).collect::<Vec<_>>().join("\n");
    deliver(&mut rt, vec![Message::User { id: id.into(), text: body }]);
    let (mut glows, mut bottoms) = (Vec::new(), Vec::new());
    for _ in 0..(1.2 * 120.0) as usize {
        rt.frame(1.0 / 120.0);
        let b = bubble(&rt, id).expect("the arrival shows");
        glows.push(b.glow.unwrap_or(0.0));
        bottoms.push(b.rect[1] + b.rect[3]);
    }
    let peak = glows.iter().cloned().fold(0.0, f32::max);
    assert!(peak > 0.9, "it glows: {glows:?}");
    let top = glows.iter().position(|&g| g == peak).unwrap();
    assert!(top <= 12, "the glow rises within 0.1 s: {glows:?}");
    assert!(glows[top..].windows(2).all(|w| w[1] <= w[0] + 1e-4), "then only fades: {glows:?}");
    assert!(glows[(0.9 * 120.0) as usize + 2..].iter().all(|&g| g == 0.0), "settled by 0.9 s: {glows:?}");

    // The pinned timeline follows the ~300 pt insert in one glide: its bubble rises without
    // stopping or stepping (speed up, then down, once) and lands within half a second.
    let speeds: Vec<f32> = bottoms.windows(2).map(|w| w[0] - w[1]).collect();
    let rise = bottoms[0] - bottoms[bottoms.len() - 1];
    assert!(rise > 200.0, "it rises from under the composer: {bottoms:?}");
    let fast = speeds.iter().cloned().fold(0.0, f32::max);
    let at = speeds.iter().position(|&v| v == fast).unwrap();
    assert!(speeds[..=at].windows(2).all(|w| w[1] >= w[0] - 0.05), "speeds {speeds:?}");
    assert!(speeds[at..].windows(2).all(|w| w[1] <= w[0] + 0.05), "speeds {speeds:?}");
    assert!(speeds.iter().all(|&v| v >= -0.01), "never back down: {speeds:?}");
    let land = bottoms.iter().position(|&y| (y - bottoms[bottoms.len() - 1]).abs() < 0.5).unwrap();
    assert!(land <= 60, "landed in {} s: {bottoms:?}", land as f32 / 120.0);
    assert!(!rt.state.glows.contains_key(id));
}

#[test]
fn a_run_of_prompts_sits_close_with_tight_trailing_corners() {
    use opencode_kit::model::Message;
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    let crate::Route::Session(id) = app.route.clone() else { panic!("no open session") };
    let t = app.transcripts.get_mut(&id).unwrap();
    t.messages.truncate(1);
    for (n, text) in ["Also the toast says reconnected twice.", "And keep the cap at 30s."].iter().enumerate() {
        t.messages.push(Message::User { id: format!("run{n}"), text: (*text).into() });
    }
    let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let bubble = |rt: &Runtime<App>, row: &str| rt.elements().into_iter()
        .find(|e| !e.exiting && e.path.contains(&format!("{row}/")) && e.key.as_deref() == Some("bubble"))
        .unwrap_or_else(|| panic!("no bubble in {row}"));
    let first = format!("u-{}", rt.state.transcripts[&id].messages[0].id());
    let (a, b, c) = (bubble(&rt, &first), bubble(&rt, "u-run0"), bubble(&rt, "u-run1"));
    assert!((b.rect[1] - (a.rect[1] + a.rect[3]) - 4.0).abs() < 0.5, "4 apart: {:?} {:?}", a.rect, b.rect);
    assert!((c.rect[1] - (b.rect[1] + b.rect[3]) - 4.0).abs() < 0.5, "4 apart: {:?} {:?}", b.rect, c.rect);
    assert!(a.rect[2] <= super::BUBBLE_MAX + 0.5, "64ch at most: {:?}", a.rect);
    // Only the run's last prompt has a copy line under it.
    let lines = rt.elements().into_iter().filter(|e| !e.exiting && e.path.contains("/u-") && e.key.as_deref() == Some("actions")).count();
    assert_eq!(lines, 1);
}

fn sidebar() -> Runtime<App> {
    settled(90)
}

fn tab_key(rt: &Runtime<App>, i: usize) -> String {
    format!("t-{}", rt.state.tabs[i].id)
}

/// The top of a tab's painted pill (its hit area reaches into the gaps).
fn tab_y(rt: &Runtime<App>, key: &str) -> f32 {
    pill(rt, key).rect[1]
}

fn pill(rt: &Runtime<App>, key: &str) -> rune::ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.path.contains(&format!("{key}/")) && e.key.as_deref() == Some("pill")).unwrap_or_else(|| panic!("no {key}"))
}

#[test]
fn the_gap_between_tabs_belongs_to_a_tab() {
    let mut rt = sidebar();
    let (a, b) = (pill(&rt, &tab_key(&rt, 1)), pill(&rt, &tab_key(&rt, 2)));
    assert!(b.rect[1] - (a.rect[1] + a.rect[3]) > 3.5, "the pills keep their gap: {:?} {:?}", a.rect, b.rect);
    // Just under tab 1's pill, in the gap: a click there opens tab 1.
    let (x, y) = (a.rect[0] + a.rect[2] / 2.0, a.rect[1] + a.rect[3] + 1.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
    let id = rt.state.tabs[1].id.clone();
    assert_eq!(rt.state.route, crate::Route::Session(id));
}

fn ids(rt: &Runtime<App>) -> Vec<String> {
    rt.state.tabs.iter().map(|t| t.id.clone()).collect()
}

#[test]
fn dragging_a_tab_moves_it_saves_the_order_and_slides_every_row() {
    let mut rt = sidebar();
    let before = ids(&rt);
    let keys: Vec<String> = (0..before.len()).map(|i| tab_key(&rt, i)).collect();
    let rest: Vec<f32> = keys.iter().map(|k| tab_y(&rt, k)).collect();
    let pitch = super::TAB_PITCH;
    assert!((rest[1] - rest[0] - pitch).abs() < 0.5, "rows sit one pitch apart: {rest:?}");

    // Row 1 dragged two rows down, in steps, then dropped.
    let (x, y) = rt.find(&keys[1]).unwrap();
    crate::tabs::SAVED.with(|s| *s.borrow_mut() = None);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    let mut drawn: Vec<Vec<f32>> = Vec::new();
    let steps = 40;
    for i in 1..=steps {
        let dy = 2.0 * pitch * i as f32 / steps as f32;
        rt.pointer_move(x, y + dy);
        rt.frame(1.0 / 120.0);
        let dragged = tab_y(&rt, &keys[1]);
        if dy >= 4.0 {
            assert!((dragged - (rest[1] + dy)).abs() < 0.01, "the lifted row follows the pointer 1:1: {dragged} vs {}", rest[1] + dy);
        } else {
            assert_eq!(dragged, rest[1], "inside the 4 pt slop it's still a press");
        }
        drawn.push(keys.iter().map(|k| tab_y(&rt, k)).collect());
    }
    assert_eq!(rt.state.tab_drag.dragging(), Some(1));
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        drawn.push(keys.iter().map(|k| tab_y(&rt, k)).collect());
    }
    let held: Vec<f32> = keys.iter().map(|k| tab_y(&rt, k)).collect();
    assert!((held[2] - rest[1]).abs() < 0.5 && (held[3] - rest[2]).abs() < 0.5, "the passed rows stepped up: {held:?}");
    assert!((held[4] - rest[4]).abs() < 0.01, "rows past the target stay put");

    // Dropped 10 pt short of the slot it takes.
    rt.pointer_move(x, y + 2.0 * pitch - 10.0);
    rt.frame(1.0 / 120.0);
    drawn.push(keys.iter().map(|k| tab_y(&rt, k)).collect());
    rt.pointer_up(x, y + 2.0 * pitch - 10.0);
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
        drawn.push(keys.iter().map(|k| tab_y(&rt, k)).collect());
    }
    let mut want = before.clone();
    let moved = want.remove(1);
    want.insert(3, moved);
    assert_eq!(ids(&rt), want, "the drop moves it");
    assert_eq!(crate::tabs::SAVED.with(|s| s.borrow().clone()), Some(want), "and saves the new order");
    assert_eq!(rt.state.route, crate::Route::Session(before[0].clone()), "a drag doesn't open the tab");

    // Every row's drawn top, frame to frame: no jumps, no reversals once it starts toward
    // its place, and each lands where it rests.
    for (r, key) in keys.iter().enumerate() {
        let ys: Vec<f32> = drawn.iter().map(|f| f[r]).collect();
        let biggest = ys.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        // The dragged row moves with the hand (one 10 pt step); the rest spring.
        let limit = if r == 1 { 10.5 } else { 6.0 };
        assert!(biggest < limit, "{key} jumps {biggest} pt in a frame: {ys:?}");
        let settled = *ys.last().unwrap();
        let slot = tab_y(&rt, key);
        assert!((settled - slot).abs() < 0.01, "{key} lands in its slot");
    }
    // From where it was let go, it settles straight down into its slot.
    let dropped: Vec<f32> = drawn[drawn.len() - 91..].iter().map(|f| f[1]).collect();
    assert!((dropped[0] - (rest[3] - 10.0)).abs() < 0.01, "let go 10 pt short: {}", dropped[0]);
    assert!(dropped[1] > dropped[0] && dropped[1] < dropped[0] + 5.0, "it starts from where it was drawn: {dropped:?}");
    assert!(dropped.windows(2).all(|w| w[1] >= w[0] - 0.01), "one way, no bounce: {dropped:?}");
    assert!((tab_y(&rt, &keys[1]) - rest[3]).abs() < 0.5, "the dropped row ends in row 3's slot");
}

#[test]
fn switching_tabs_lets_you_type() {
    let mut rt = sidebar();
    let before = ids(&rt);
    for i in [2, 1] {
        let (x, y) = rt.find(&tab_key(&rt, i)).unwrap();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.route, crate::Route::Session(before[i].clone()));
        rt.state.composer.text.clear();
        rt.key(KeyEvent { key: Key::Text("hi".into()), mods: Default::default() });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.composer.text, "hi", "tab {i} takes typing right away");
    }
}

#[test]
fn a_click_without_a_drag_still_opens_the_tab() {
    let mut rt = sidebar();
    let before = ids(&rt);
    let key = tab_key(&rt, 2);
    let (x, y) = rt.find(&key).unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 2.0);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x, y + 2.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.route, crate::Route::Session(before[2].clone()), "the click opens it");
    assert_eq!(ids(&rt), before, "and moves nothing");
    assert_eq!(rt.state.tab_drag.dragging(), None);
}

#[test]
fn escape_puts_a_dragged_tab_back() {
    let mut rt = sidebar();
    rt.on_key = Some(crate::on_key);
    let before = ids(&rt);
    let key = tab_key(&rt, 0);
    let rest = tab_y(&rt, &key);
    let (x, y) = rt.find(&key).unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x, y + 3.0 * super::TAB_PITCH);
    rt.frame(1.0 / 60.0);
    rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x, y + 3.0 * super::TAB_PITCH);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(ids(&rt), before);
    assert!((tab_y(&rt, &key) - rest).abs() < 0.01, "back in its place");
}

#[test]
fn typing_reuses_memoized_timeline_rows() {
    let mut app = crate::perf::fixture();
    app.composer.focused = true;
    let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) {
            break;
        }
    }
    super::ROW_BUILDS.set(0);
    rt.key(KeyEvent { key: Key::Text("a".into()), mods: Default::default() });
    rt.frame(1.0 / 60.0);
    // Only `turn-actions` rebuilds; every mounted Markdown reply, collapsed tool group and
    // plain prompt bubble hits `rune::memo`.
    assert!(super::ROW_BUILDS.get() <= 1, "expected <= 1 row build on typing, got {}", super::ROW_BUILDS.get());
}

#[test]
fn a_long_header_title_shrinks_before_usage_at_large_zoom() {
    for w in [1040.0, 860.0] {
        let mut app = App::fixture_session();
        app.zoom = 1.6;
        if let crate::Route::Session(id) = app.route.clone()
            && let Some(info) = app.sessions.iter_mut().find(|s| s.id == id)
        {
            info.title = "Refactor reconnect backoff, bounded session cache, and virtual timeline rows".into();
        }
        let mut rt = Runtime::new(app, super::app, (w, 680.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let rect = |key: &str| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap().rect;
        let (crumbs, title, usage) = (rect("crumbs"), rect("title"), rect("usage"));
        assert!(crumbs[0] + crumbs[2] <= usage[0] + 0.5, "w={w}: crumbs {:?} must stay left of usage {:?}", crumbs, usage);
        if w >= 1040.0 {
            assert!(title[2] > 80.0, "w={w}: title keeps readable room before fading: {:?}", title);
        }
    }
}
