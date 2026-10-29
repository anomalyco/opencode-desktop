//! The composer's editor stops growing at Desktop's 180 pt and scrolls a long prompt inside.
use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};

use crate::App;

fn session() -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn el(rt: &Runtime<App>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

fn timeline(rt: &Runtime<App>) -> [f32; 4] {
    el(rt, "timeline-area").rect
}

/// The caret line sits wholly inside the editor's scroll viewport.
fn caret_shows(rt: &Runtime<App>) {
    let editor = el(rt, "editor-scroll").rect;
    let caret = el(rt, "prompt").caret.expect("a caret");
    let (top, bottom) = (editor[1], editor[1] + editor[3]);
    assert!(caret[1] >= top - 0.5 && caret[1] + caret[3] <= bottom + 0.5, "caret {caret:?} inside the viewport {editor:?}");
}

fn long_paste(rt: &mut Runtime<App>) {
    let (x, y) = rt.find("prompt").unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.paste(crate::stories::long_paste());
    settle(rt);
}

#[test]
fn a_long_paste_stops_at_the_cap_and_scrolls_inside() {
    let mut rt = session();
    let before = timeline(&rt);
    let short = el(&rt, "editor").rect[3];
    long_paste(&mut rt);
    let editor = el(&rt, "editor").rect;
    assert!((editor[3] - super::COMPOSER_MAX_H).abs() < 0.5, "the editor stops at the cap: {editor:?}");
    assert!(el(&rt, "prompt").rect[3] > 2000.0, "the text runs on under it");
    let after = timeline(&rt);
    assert!(before[3] - after[3] <= super::COMPOSER_MAX_H - short + 0.5, "the timeline keeps its room: {before:?} -> {after:?}");
    caret_shows(&rt);

    // Typing at the end keeps the caret in view.
    for word in ["\n", "and", " one", " more", "\n", "line"] {
        rt.key(KeyEvent { key: Key::Text(word.into()), mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        caret_shows(&rt);
    }
    settle(&mut rt);
    caret_shows(&rt);
    assert!((el(&rt, "editor").rect[3] - super::COMPOSER_MAX_H).abs() < 0.5, "still at the cap");
}

#[test]
fn the_wheel_over_a_long_prompt_scrolls_the_prompt_not_the_timeline() {
    let mut rt = session();
    long_paste(&mut rt);
    let editor = el(&rt, "editor").rect;
    let (x, y) = (editor[0] + editor[2] / 2.0, editor[1] + editor[3] / 2.0);
    let rows = |rt: &Runtime<App>| rt.elements().into_iter().filter(|e| !e.exiting && e.key.as_deref() == Some("bubble")).map(|e| e.rect[1]).collect::<Vec<_>>();
    let (text, list) = (el(&rt, "prompt").rect[1], rows(&rt));
    rt.wheel(x, y, 120.0);
    rt.frame(1.0 / 60.0);
    assert!(el(&rt, "prompt").rect[1] > text + 100.0, "the prompt scrolls back toward its start");
    assert_eq!(rows(&rt), list, "the timeline stays put");
}

#[test]
fn a_short_prompt_keeps_its_height() {
    let rt = session();
    let short = el(&rt, "editor").rect[3];
    assert!(short < super::COMPOSER_MAX_H, "an empty composer sits under the cap: {short}");
    assert_eq!(short, 16.0 + 40.0 + 8.0, "the editor's padding around a two-line field");
}
