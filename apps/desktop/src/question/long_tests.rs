//! A long question keeps to part of the pane, its header and footer stay on screen, and the
//! keys bring the highlighted answer into sight without moving the list when it's already there.

use rune::{ElementInfo, Key, KeyEvent, Mods, Runtime};

use crate::App;
use crate::fixture_ids::MAIN as SESSION;

/// The long question asked on the fixture session, zoomed in, in a `size` window.
fn asked(size: (f32, f32)) -> Runtime<App> {
    let mut app = App::fixture_session();
    app.zoom = 1.6;
    let q = super::form(&app);
    app.apply_event("form.created", &q);
    std::mem::forget(app.fake_writes());
    let mut rt = app.into_runtime_sized(size);
    rt.poll = None;
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
    }
}

fn press(rt: &mut Runtime<App>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    settle(rt);
}

fn el(rt: &Runtime<App>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[1] >= outer[1] - 0.5 && inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
}

/// At the Air's window and a short one, the dock stays within its share of the pane, the
/// timeline still shows above it, and the footer is whole.
#[test]
fn a_long_question_leaves_the_timeline_room() {
    for size in [(1470.0, 920.0), (1280.0, 640.0)] {
        let rt = asked(size);
        let panel = el(&rt, "panel").rect;
        let composer = el(&rt, "composer").rect;
        let cap = (panel[3] * 0.6).max(240.0 * 1.6);
        assert!(composer[3] <= cap + 1.0, "{size:?}: a {} pt dock in a {} pt pane", composer[3], panel[3]);
        let timeline = el(&rt, &format!("scroll-{SESSION}"));
        assert!(timeline.visible[3] >= panel[3] * 0.2, "{size:?}: {} pt of timeline shows", timeline.visible[3]);
        for key in ["q-next", "q-dismiss", "q-top"] {
            let e = el(&rt, key);
            assert!(inside(e.rect, e.visible) && inside(e.rect, panel), "{size:?}: {key} whole on screen: {:?} in {panel:?}", e.rect);
        }
        let last = el(&rt, "q-opt-8");
        assert!(!inside(last.rect, last.visible), "{size:?}: the last answers scroll out of sight");
    }
}

/// ↓ walks down every answer to "Type your own answer" and ↑ back: the highlighted one is
/// always whole in the viewport, the list moves only when it has to, and the header and footer
/// stay put.
#[test]
fn the_keys_keep_the_highlight_in_sight() {
    let mut rt = asked((1280.0, 640.0));
    let view = el(&rt, "q-scroll-0").rect;
    let footer = el(&rt, "q-next").rect;
    let key = |rt: &Runtime<App>| {
        let focus = rt.state.question.focus;
        if focus == super::ANGLES.len() { "q-custom".to_owned() } else { format!("q-opt-{focus}") }
    };
    let mut moved = 0;
    for (step, k) in std::iter::repeat_n(Key::Down, super::ANGLES.len()).chain(std::iter::repeat_n(Key::Up, super::ANGLES.len())).enumerate() {
        let list = el(&rt, "q-options-0").rect[1];
        let before = rt.state.question.focus;
        let next = if k == Key::Down { before + 1 } else { before - 1 };
        let target = if next == super::ANGLES.len() { "q-custom".to_owned() } else { format!("q-opt-{next}") };
        let shown = inside(el(&rt, &target).rect, view);
        press(&mut rt, k);
        let row = el(&rt, &key(&rt)).rect;
        assert!(inside(row, view), "step {step}: the highlight {row:?} is whole in {view:?}");
        let now = el(&rt, "q-options-0").rect[1];
        // Back on the first answer the list goes to its top, showing the question again.
        if shown && next > 0 {
            assert!((now - list).abs() < 0.5, "step {step}: the list moved {} pt with the highlight already in sight", now - list);
        } else if (now - list).abs() > 0.5 {
            moved += 1;
        }
    }
    assert!(moved > 0, "the list scrolled to reach the far answers");
    assert_eq!(el(&rt, "q-next").rect, footer, "the footer stays put");
    assert_eq!(el(&rt, "q-scroll-0").rect, view, "and so does the viewport");
}

/// End goes to "Type your own answer" at the bottom, Home back to the first at the top.
#[test]
fn home_and_end_reach_the_ends() {
    let mut rt = asked((1280.0, 640.0));
    let view = el(&rt, "q-scroll-0").rect;
    press(&mut rt, Key::End);
    assert!(inside(el(&rt, "q-custom").rect, view), "End reveals the typed answer");
    press(&mut rt, Key::Home);
    let (first, question) = (el(&rt, "q-opt-0").rect, el(&rt, "q-text").rect);
    assert!(inside(first, view), "Home reveals the first answer");
    let snug = first[1] + first[3] >= view[1] + view[3] - super::FADE * 1.6 - 1.0;
    assert!(inside(question, view) || snug, "and as much of the question as fits: {question:?} over {first:?} in {view:?}");
}
