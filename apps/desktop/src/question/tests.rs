use crate::fixture_ids::{MAIN as SESSION};
use std::sync::mpsc::{Receiver, channel};

use opencode_kit::client::{Server, Write};
use rune::{Env, Key, KeyEvent, Mods, Runtime, TextEdit};
use serde_json::json;

use super::{Intent, Zone, intent};
use crate::{App, Source};

const FORM: &str = "frm_fx0001";

fn ev(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn cmd(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods { cmd: true, ..Mods::default() } }
}

fn txt(s: &str) -> KeyEvent {
    ev(Key::Text(s.into()))
}

#[test]
fn keys_map_as_desktops() {
    let o = Zone::Option;
    assert_eq!(intent(&ev(Key::Escape), Zone::Input, 4), Intent::Dismiss);
    assert_eq!(intent(&ev(Key::Down), o, 4), Intent::Move(1));
    assert_eq!(intent(&ev(Key::Up), o, 4), Intent::Move(-1));
    assert_eq!(intent(&ev(Key::Home), o, 4), Intent::First);
    assert_eq!(intent(&ev(Key::End), o, 4), Intent::Last);
    assert_eq!(intent(&ev(Key::Left), o, 4), Intent::Question(-1));
    assert_eq!(intent(&ev(Key::Right), o, 4), Intent::Question(1));
    assert_eq!(intent(&cmd(Key::Command('[')), o, 4), Intent::Question(-1));
    assert_eq!(intent(&cmd(Key::Enter), Zone::Input, 4), Intent::Advance);
    assert_eq!(intent(&txt("2"), o, 4), Intent::Number(1));
    assert_eq!(intent(&txt("4"), o, 4), Intent::Number(3));
    assert_eq!(intent(&txt("5"), o, 4), Intent::None, "past the answers");
    assert_eq!(intent(&txt("0"), o, 4), Intent::None);
    assert_eq!(intent(&txt(" "), o, 4), Intent::Activate);
    assert_eq!(intent(&ev(Key::Enter), o, 4), Intent::Commit);
    assert_eq!(intent(&KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } }, o, 4), Intent::None);
    assert_eq!(intent(&txt("a"), o, 4), Intent::None);
    assert_eq!(intent(&txt("a"), Zone::Custom, 4), Intent::Type("a".into()));
    assert_eq!(intent(&txt("1"), Zone::Custom, 4), Intent::Number(0), "digits still pick on the typed answer");
    assert_eq!(intent(&txt("2"), Zone::Input, 4), Intent::None, "typing in the field");
    assert_eq!(intent(&ev(Key::Down), Zone::Input, 4), Intent::None);
    assert_eq!(intent(&KeyEvent { key: Key::Down, mods: Mods { alt: true, ..Mods::default() } }, o, 4), Intent::None, "⌥↓ switches tabs");
    assert_eq!(intent(&KeyEvent { key: Key::Down, mods: Mods { shift: true, ..Mods::default() } }, o, 4), Intent::None);
    assert_eq!(intent(&cmd(Key::Command('t')), o, 4), Intent::None, "app shortcuts pass");
}

/// The fixture session with its question asked and a draft in the composer. Live sends go
/// to a channel, never a server; a fixture run polls its own playback instead.
fn asked(live: bool) -> (Runtime<App>, Option<Receiver<Write>>) {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    app.composer = TextEdit::new("half a thought");
    let q = app.fixture["question"].clone();
    app.apply_event("form.created", &q);
    let writes = live.then(|| {
        let (w, r) = channel();
        app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
        r
    });
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    if !live {
        rt.poll = Some(crate::poll);
    }
    rt.on_key = Some(crate::on_key);
    settle(&mut rt);
    (rt, writes)
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
    }
}

fn press(rt: &mut Runtime<App>, e: KeyEvent) {
    rt.key(e);
    rt.frame(1.0 / 120.0);
}

fn picks(rt: &Runtime<App>, key: &str) -> Vec<String> {
    rt.state.picks.get(&format!("{FORM}/{key}")).cloned().unwrap_or_default()
}

fn shows_composer(rt: &Runtime<App>) -> bool {
    rt.find("prompt").is_some() && rt.find("question").is_none()
}

#[test]
fn the_composer_becomes_the_question() {
    let (rt, _) = asked(true);
    assert!(rt.find("question").is_some() && rt.find("q-opt-0").is_some());
    assert!(rt.find("prompt").is_none() && rt.find("toolbar").is_none(), "one surface, not a card above the composer");
    assert!(rt.find(&format!("request-{FORM}")).is_none());
    let title = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(format!("q-title-{FORM}").as_str())).and_then(|e| e.label);
    assert_eq!(title.as_deref(), Some("1 of 2 questions"));
}

#[test]
fn arrows_move_digits_pick_and_enter_submits() {
    let (mut rt, writes) = asked(true);
    let focus = |rt: &Runtime<App>| rt.state.question.focus;
    press(&mut rt, ev(Key::Down));
    assert_eq!(focus(&rt), 1);
    for _ in 0..5 {
        press(&mut rt, ev(Key::Down));
    }
    assert_eq!(focus(&rt), 3, "stops on the typed answer");
    press(&mut rt, ev(Key::Home));
    assert_eq!(focus(&rt), 0);
    press(&mut rt, ev(Key::End));
    press(&mut rt, ev(Key::Up));
    assert_eq!(focus(&rt), 2);
    assert!(picks(&rt, "q0").is_empty(), "moving only highlights");

    press(&mut rt, txt("2"));
    assert_eq!(picks(&rt, "q0"), ["Real timers"]);
    assert_eq!(focus(&rt), 1);
    press(&mut rt, ev(Key::Down));
    // Enter picks the highlighted answer and moves on.
    press(&mut rt, ev(Key::Enter));
    assert_eq!(picks(&rt, "q0"), ["Only the math"]);
    assert_eq!(rt.state.question.tab, 1);
    settle(&mut rt);
    let hint = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("q-hint") && !e.exiting).and_then(|e| e.text);
    assert_eq!(hint.as_deref(), Some("Pick any"));

    press(&mut rt, txt("1"));
    press(&mut rt, txt("3"));
    press(&mut rt, txt(" "));
    press(&mut rt, txt(" "));
    assert_eq!(picks(&rt, "q1"), ["Reset after success", "Jitter stays in range"]);
    press(&mut rt, ev(Key::Left));
    assert_eq!(rt.state.question.tab, 0);
    assert_eq!(focus(&rt), 2, "back on its pick");
    press(&mut rt, ev(Key::Right));
    assert_eq!(rt.state.question.tab, 1);
    assert!(writes.as_ref().unwrap().try_recv().is_err(), "nothing sent yet");

    press(&mut rt, ev(Key::Enter));
    let Ok(Write::Form { session, form, answer }) = writes.unwrap().try_recv() else { panic!("no reply") };
    assert_eq!((session.as_str(), form.as_str()), (SESSION, FORM));
    assert_eq!(answer, json!({ "q0": "Only the math", "q1": ["Reset after success", "Jitter stays in range"] }));
    settle(&mut rt);
    assert!(shows_composer(&rt));
    assert_eq!(rt.state.composer.text, "half a thought");
    assert!(rt.state.composer.focused, "the composer has the keyboard back");
}

#[test]
fn escape_dismisses() {
    let (mut rt, writes) = asked(true);
    press(&mut rt, ev(Key::Escape));
    let Ok(Write::CancelForm { session, form }) = writes.unwrap().try_recv() else { panic!("no cancel") };
    assert_eq!((session.as_str(), form.as_str()), (SESSION, FORM));
    settle(&mut rt);
    assert!(shows_composer(&rt));
    assert_eq!(rt.state.composer.text, "half a thought");
}

#[test]
fn a_typed_answer_goes_in_the_reply() {
    let (mut rt, writes) = asked(true);
    press(&mut rt, txt("4"));
    assert!(rt.state.typed.get(&format!("{FORM}/q0")).is_some_and(|t| t.focused), "4 opens the field");
    for c in ["B", "o", "t", "h"] {
        press(&mut rt, txt(c));
    }
    // Esc leaves the field without dismissing; a printable key goes back to typing.
    press(&mut rt, ev(Key::Escape));
    assert!(writes.as_ref().unwrap().try_recv().is_err());
    press(&mut rt, txt("!"));
    press(&mut rt, ev(Key::Enter));
    assert_eq!(rt.state.question.tab, 1);
    press(&mut rt, cmd(Key::Enter));
    let Ok(Write::Form { answer, .. }) = writes.unwrap().try_recv() else { panic!("no reply") };
    assert_eq!(answer, json!({ "q0": "Both!" }));
}

#[test]
fn a_click_picks_and_submit_replies() {
    let (mut rt, writes) = asked(true);
    let click = |rt: &mut Runtime<App>, key: &str| {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    };
    click(&mut rt, "q-opt-0");
    assert_eq!(picks(&rt, "q0"), ["Fake clock"]);
    click(&mut rt, "q-next");
    click(&mut rt, "q-opt-1");
    click(&mut rt, "q-next");
    let Ok(Write::Form { answer, .. }) = writes.unwrap().try_recv() else { panic!("no reply") };
    assert_eq!(answer, json!({ "q0": "Fake clock", "q1": ["Cap at 30 s"] }));
}

/// The surface's height each frame through a change made by `event` from another client.
fn morph(rt: &mut Runtime<App>, ty: &str, data: serde_json::Value) -> Vec<f32> {
    let at = rt.state.clock + 0.01;
    rt.state.playback.push_back((at, ty.into(), data));
    let height = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("composer")).unwrap().rect[3];
    let mut hs = vec![height(rt)];
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        hs.push(height(rt));
    }
    hs
}

/// Springs from one height to the other: monotonic, no overshoot, no frame covering more
/// than a fifth of the way, settled at the end, within 2 % of the way after `lands` frames.
fn springs(hs: &[f32], what: &str, lands: std::ops::RangeInclusive<usize>) {
    let (a, b) = (hs[0], *hs.last().unwrap());
    assert!((a - b).abs() > 40.0, "{what}: {a} → {b}");
    let dir = (b - a).signum();
    for w in hs.windows(2) {
        assert!((w[1] - w[0]) * dir >= -0.01, "{what}: turns back at {w:?}");
        assert!((w[1] - w[0]).abs() < (b - a).abs() / 5.0, "{what}: jumps {w:?}");
    }
    assert!(hs.iter().all(|h| (h - a) * dir >= -0.01 && (h - b) * dir <= 0.01), "{what}: overshoots");
    let done = hs.iter().position(|h| (h - b).abs() < (b - a).abs() * 0.02).unwrap();
    assert!(lands.contains(&done), "{what}: lands after {done} frames at 120 Hz");
}

#[test]
fn an_answer_from_elsewhere_morphs_back_and_keeps_the_draft() {
    let (mut rt, _) = asked(false);
    assert!(!shows_composer(&rt));
    let hs = morph(&mut rt, "form.replied", json!({ "id": FORM, "sessionID": SESSION }));
    // Within 2 % of the way in about the morph's 0.4 s.
    springs(&hs, "closing", 30..=60);
    assert!(shows_composer(&rt));
    assert_eq!(rt.state.composer.text, "half a thought");
    let q = rt.state.fixture["question"].clone();
    let hs = morph(&mut rt, "form.created", q);
    springs(&hs, "opening", 30..=60);
    assert!(rt.find("q-opt-0").is_some());
}

/// The timeline is pinned to its end while a question is up, so through the storybook's
/// morph loop it rides the surface's top edge both ways, frame by frame, on every Question
/// dock preset: its last row ends the same distance above the surface throughout.
#[test]
fn the_timeline_follows_the_morph() {
    let g = crate::knobs::groups().iter().find(|g| g.name == "Question dock").unwrap();
    for p in g.presets.iter().map(|p| p.name).chain(["Desktop"]) {
        // The story's world takes the app's knobs.
        let mut rt = crate::stories::storybook();
        crate::knobs::apply_preset(&mut rt.state, p);
        rt.open_playground("Question dock/Morph", None);
        let gap = |rt: &Runtime<App>| {
            let top = rt.playground_element("composer").unwrap().rect[1];
            let last = rt.playground_element("turn-actions").expect("the last row").rect;
            top - (last[1] + last[3])
        };
        let shown = |rt: &Runtime<App>| crate::stories::world(rt).question_shown();
        let (rest, mut was, mut flips) = (gap(&rt), shown(&rt), 0);
        for frame in 0..120 * 7 {
            rt.frame(1.0 / 120.0);
            if shown(&rt) != was {
                was = !was;
                flips += 1;
            }
            let off = gap(&rt) - rest;
            let way = if was { "opening" } else { "closing" };
            assert!(off.abs() < 0.5, "{p}, {way}: the timeline leaves the surface by {off} pt at frame {frame}");
        }
        assert!(flips >= 4, "{p}: the dock opened and closed twice ({flips} changes)");
    }
}

/// Every Question dock preset but the bouncy one morphs without turning back or jumping,
/// and lands within its duration; the bouncy one overshoots a little and lands.
#[test]
fn question_dock_presets_morph_smoothly() {
    let g = crate::knobs::groups().iter().find(|g| g.name == "Question dock").unwrap();
    for p in g.presets.iter().map(|p| p.name).chain(["Desktop"]) {
        let (mut rt, _) = asked(false);
        crate::knobs::apply_preset(&mut rt.state, p);
        let frames = (rt.state.knobs.get(&crate::knobs::MORPH_DURATION) * 120.0) as usize;
        for (ty, data) in [("form.replied", json!({ "id": FORM, "sessionID": SESSION })), ("form.created", rt.state.fixture["question"].clone())] {
            let hs = morph(&mut rt, ty, data);
            if p == "Bounce" {
                let (a, b) = (hs[0], *hs.last().unwrap());
                let past = hs.iter().map(|h| (h - b) * (b - a).signum()).fold(0.0, f32::max);
                assert!(past > 0.5 && past < (b - a).abs() * 0.1, "{p} {ty}: overshoots {past}");
                assert!((hs[hs.len() - 2] - b).abs() < 0.5, "{p} {ty}: lands");
            } else {
                springs(&hs, &format!("{p} {ty}"), 10..=frames + 4);
            }
        }
    }
}
