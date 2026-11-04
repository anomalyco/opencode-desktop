use rune::{ElementInfo, Env, Runtime};

use super::{INSET, MIN_LEN, thumb};
use crate::App;

const DT: f64 = 1.0 / 120.0;
const KEY: &str = "scroll-bench";

fn app(zoom: f32) -> Runtime<App> {
    let mut a = crate::perf::fixture();
    a.zoom = zoom;
    let mut rt = Runtime::new(a, crate::view::app, (1280.0, 820.0), Env::new(true, 2.0));
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    frames(&mut rt, 1.0);
    rt
}

/// The fixture with "Show scroll bars: Always", so the bar is drawn at rest.
fn always(zoom: f32) -> Runtime<App> {
    let mut rt = app(zoom);
    rt.state.bars.set_always(true);
    frames(&mut rt, 0.5);
    rt
}

fn drawn(rt: &Runtime<App>, key: &str) -> bool {
    rt.elements().into_iter().any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn frames(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        rt.frame(DT);
    }
}

fn find(rt: &Runtime<App>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

fn thumb_rect(rt: &Runtime<App>) -> [f32; 4] {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("bar-thumb") && e.path.contains("scroll-bench-bar")).expect("a thumb").rect
}

fn wheel(rt: &mut Runtime<App>, dy: f32) {
    let v = find(rt, KEY).rect;
    rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, dy);
}

fn offset(rt: &Runtime<App>) -> f32 {
    rt.state.bars.bars.borrow()[KEY].pos.get().unwrap().0
}

#[test]
fn the_thumb_is_the_viewports_share_of_the_track() {
    // Half the content shows: half the room.
    let (y, h) = thumb(404.0, 0.0, 404.0);
    assert_eq!((y, h), (INSET, 200.0));
    let (y, h) = thumb(404.0, 404.0, 404.0);
    assert_eq!((y, h), (INSET + 200.0, 200.0));
    // Very long content: never shorter than the minimum, and the end still reaches the end.
    let (y, h) = thumb(404.0, 1e6, 1e6);
    assert_eq!(h, MIN_LEN);
    assert_eq!(y + h, 404.0 - INSET);
}

#[test]
fn a_scroll_shows_the_bar_and_a_rest_fades_it() {
    let mut rt = app(1.0);
    assert!(!rt.state.bars.shown(KEY), "a session opens with its bar hidden");
    assert!(!drawn(&rt, "bar-face"), "at rest it draws nothing, so a resize runs no view");
    wheel(&mut rt, 600.0);
    rt.frame(DT);
    assert!(rt.state.bars.shown(KEY), "scrolling shows it");
    rt.frame(DT);
    assert!(drawn(&rt, "bar-face"));
    frames(&mut rt, 0.5);
    assert!(rt.state.bars.shown(KEY), "it stays while the scroll settles");
    frames(&mut rt, 1.5);
    assert!(!rt.state.bars.shown(KEY), "it fades after a rest");
    frames(&mut rt, 0.7);
    assert!(!drawn(&rt, "bar-face"), "and stops drawing once faded");
}

#[test]
fn always_keeps_it_showing() {
    let rt = always(1.0);
    assert!(rt.state.bars.shown(KEY));
    assert!((find(&rt, "bar-face").opacity - 1.0).abs() < 1e-3);
}

#[test]
fn a_streaming_reply_doesnt_show_it() {
    let mut rt = app(1.0);
    for i in 0..60 {
        rt.update(|s, _| {
            let t = s.transcripts.get_mut("bench").unwrap();
            if let Some(opencode_kit::model::Message::Assistant(a)) = t.messages.last_mut()
                && let Some(opencode_kit::model::Part::Text { text }) = a.parts.last_mut()
            {
                text.push_str(&format!(" word{i}"));
                if i % 6 == 0 { text.push_str("\n\n"); }
            }
        });
        frames(&mut rt, 0.05);
    }
    assert!(!rt.state.bars.shown(KEY), "following its end as text arrives isn't a scroll");
}

/// At the end, the thumb sits at the track's foot, inside the viewport, zoomed or not.
#[test]
fn the_thumb_sits_inside_the_viewport_at_any_zoom() {
    for zoom in [1.0, 1.6] {
        let rt = always(zoom);
        let (t, v) = (thumb_rect(&rt), find(&rt, KEY).rect);
        assert!(t[0] + t[2] <= v[0] + v[2] + 0.5 && t[0] > v[0] + v[2] - 20.0 * zoom, "zoom {zoom}: thumb {t:?} in {v:?}");
        let foot = v[1] + v[3] - INSET * zoom;
        assert!((t[1] + t[3] - foot).abs() < 1.0, "zoom {zoom}: thumb {t:?} ends at {foot}, viewport {v:?}");
        assert!(t[3] >= MIN_LEN * zoom - 0.5, "zoom {zoom}: {t:?}");
    }
}

/// Dragging the thumb to the track's top scrolls to the start; zoomed, it tracks the pointer.
#[test]
fn dragging_the_thumb_scrolls_zoomed() {
    let mut rt = always(1.6);
    let t = thumb_rect(&rt);
    let (x, y) = (t[0] + t[2] / 2.0, t[1] + t[3] / 2.0);
    rt.pointer_move(x, y);
    frames(&mut rt, 0.2);
    assert!(rt.state.bars.shown(KEY), "at the edge it shows");
    let max = rt.state.bars.bars.borrow()[KEY].pos.get().unwrap().1;
    rt.pointer_down(x, y);
    rt.frame(DT);
    // Halfway up the track's travel, in window points.
    let v = find(&rt, KEY).rect;
    let travel = v[3] - 2.0 * INSET * 1.6 - t[3];
    for k in 1..=10 {
        rt.pointer_move(x, y - travel / 2.0 * k as f32 / 10.0);
        rt.frame(DT);
    }
    frames(&mut rt, 0.2);
    let at = offset(&rt) / max;
    assert!((at - 0.5).abs() < 0.05, "half the travel is half the content: {at}");
    let t2 = thumb_rect(&rt);
    assert!(((t2[1] + t2[3] / 2.0) - (y - travel / 2.0)).abs() < 4.0, "the thumb stays under the pointer: {t2:?}");
    rt.pointer_move(x, v[1] - 200.0);
    rt.frame(DT);
    frames(&mut rt, 0.3);
    assert!(offset(&rt) < 1.0, "past the top is the start: {}", offset(&rt));
    rt.pointer_up(x, v[1] - 200.0);
    frames(&mut rt, 0.1);
}

/// A click in the track above the thumb pages up.
#[test]
fn a_click_in_the_track_pages() {
    let mut rt = always(1.0);
    let t = thumb_rect(&rt);
    let before = offset(&rt);
    let (x, y) = (t[0] + t[2] / 2.0, t[1] - 100.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(&mut rt, 0.3);
    let v = find(&rt, KEY).rect;
    let moved = before - offset(&rt);
    assert!((moved - v[3] * 0.9).abs() < 2.0, "one page up: {moved} of {}", v[3]);
}

/// Scrolling up through rows that measure as they mount, the thumb never jumps.
#[test]
fn the_thumb_glides_while_rows_measure() {
    let mut rt = always(1.0);
    let mut was = thumb_rect(&rt)[1];
    let mut worst: f32 = 0.0;
    for _ in 0..40 {
        wheel(&mut rt, 120.0);
        rt.frame(DT);
        let y = thumb_rect(&rt)[1];
        worst = worst.max((y - was).abs());
        was = y;
    }
    assert!(worst < 6.0, "the thumb moved {worst} pt in one frame");
}

/// Settings scrolls with the same bar, at its own right edge.
#[test]
fn settings_has_the_bar() {
    let mut rt = app(1.0);
    rt.update(|s, _| s.open_settings());
    frames(&mut rt, 0.5);
    let v = find(&rt, "settings").rect;
    rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, -400.0);
    rt.frame(DT);
    assert!(rt.state.bars.shown("settings"), "scrolling settings shows its bar");
    rt.frame(DT);
    let t = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("bar-thumb") && e.path.contains("settings-bar")).expect("a thumb").rect;
    assert!(t[0] + t[2] <= v[0] + v[2] + 0.5 && t[0] > v[0] + v[2] - 20.0, "thumb {t:?} at the edge of {v:?}");
}

/// Coming back to a session that was left scrolled up doesn't flash its bar.
#[test]
fn reopening_a_viewport_doesnt_show_it() {
    let mut rt = app(1.0);
    wheel(&mut rt, 600.0);
    frames(&mut rt, 2.0);
    assert!(!rt.state.bars.shown(KEY));
    rt.update(|s, _| s.open_settings());
    frames(&mut rt, 0.5);
    rt.update(|s, _| s.close_settings());
    for _ in 0..60 {
        rt.frame(DT);
        assert!(!rt.state.bars.shown(KEY), "the timeline finding its place isn't a scroll");
    }
}

#[test]
fn jumping_to_latest_doesnt_show_it() {
    let mut rt = app(1.0);
    wheel(&mut rt, 600.0);
    frames(&mut rt, 2.5);
    assert!(!rt.state.bars.shown(KEY));
    rt.update(|s, tx| {
        s.bars.follow(KEY);
        tx.scroll_to_bottom(KEY);
    });
    for _ in 0..120 {
        rt.frame(DT);
        assert!(!rt.state.bars.shown(KEY), "the jump isn't Kit scrolling");
    }
    wheel(&mut rt, 300.0);
    rt.frame(DT);
    assert!(rt.state.bars.shown(KEY), "scrolling up again shows it");
}
