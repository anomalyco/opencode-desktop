//! The composer's editor stops growing at Desktop's 180 pt and scrolls a long prompt inside.
use rune::{ElementInfo, Key, KeyEvent, Mods, Runtime};

use crate::App;

fn session() -> Runtime<App> {
    let mut rt = crate::fixture_runtime();
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
    assert_eq!(short, 16.0 + 20.0 + 8.0, "the editor's padding around one line");
}

/// Gaps and width in layout points: the zoomed shell scales every rect, so differences between rects divide by zoom.
fn dock_gaps(width: f32, zoom: f32) -> (f32, f32, f32, f32) {
    let mut app = App::fixture_session();
    app.zoom = zoom;
    let mut rt = rune::Runtime::new(app, super::app, (width, 760.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);
    let (dock, composer) = (el(&rt, "dock").rect, el(&rt, "composer").rect);
    let side = composer[0] - dock[0];
    let right = dock[0] + dock[2] - (composer[0] + composer[2]);
    let bottom = dock[1] + dock[3] - (composer[1] + composer[3]);
    let z = zoom;
    (side / z, right / z, bottom / z, composer[2] / z)
}

#[test]
fn the_composer_sits_as_far_from_the_side_as_from_the_bottom() {
    for zoom in [1.0, 1.6] {
        let (left, right, bottom, _) = dock_gaps(560.0 * zoom, zoom);
        eprintln!("narrow zoom {zoom}: left {left} right {right} bottom {bottom}");
        assert!((left - bottom).abs() < 0.5 && (right - bottom).abs() < 0.5, "zoom {zoom}: sides {left}/{right}, bottom {bottom}");
        assert!((bottom - super::DOCK_SIDE).abs() < 0.5, "zoom {zoom}: bottom {bottom}");
    }
}

#[test]
fn at_full_width_the_composer_stands_twelve_wider_than_the_reply_text() {
    for zoom in [1.0, 1.6] {
        let (left, right, bottom, w) = dock_gaps(1400.0 * zoom, zoom);
        eprintln!("wide zoom {zoom}: left {left} right {right} bottom {bottom} w {w}");
        assert!((w - (super::CONTENT_W + 24.0)).abs() < 0.5, "zoom {zoom}: composer {w}");
        assert!((left - right).abs() < 0.5, "zoom {zoom}: centred");
    }
}
