use rune::{ElementInfo, Env, Runtime, ScrollBars};

use crate::App;

const DT: f64 = 1.0 / 120.0;
const KEY: &str = "scroll-bench";

fn app(zoom: f32, bars: ScrollBars) -> Runtime<App> {
    let mut a = crate::perf::fixture();
    a.zoom = zoom;
    let mut rt = Runtime::new(a, counted, (1280.0, 820.0), Env::new(true, 2.0));
    rt.set_system_scroll_bars(bars);
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    frames(&mut rt, 1.0);
    rt
}

thread_local! {
    static VIEWS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn counted(s: &App, cx: &mut rune::Cx<App>) -> rune::El {
    VIEWS.set(VIEWS.get() + 1);
    crate::view::app(s, cx)
}

fn frames(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        rt.frame(DT);
    }
}

fn find(rt: &Runtime<App>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

/// The thumb of the bar on the viewport keyed `area`.
fn thumb(rt: &Runtime<App>, area: &str) -> ElementInfo {
    let area = format!("{area}-scroll-area");
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("thumb") && e.path.contains(&area)).expect("a thumb")
}

fn shown(rt: &Runtime<App>) -> bool {
    thumb(rt, KEY).opacity > 0.01
}

fn wheel(rt: &mut Runtime<App>, dy: f32) {
    let v = find(rt, KEY).rect;
    rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, dy);
}

#[test]
fn a_scroll_shows_the_bar_and_a_rest_fades_it() {
    let mut rt = app(1.0, ScrollBars::WhenScrolling);
    assert!(!shown(&rt), "a session opens with its bar hidden");
    for _ in 0..10 {
        wheel(&mut rt, 60.0);
        rt.frame(DT);
    }
    frames(&mut rt, 0.2);
    assert!(shown(&rt), "scrolling shows it");
    frames(&mut rt, 2.0);
    assert!(!shown(&rt), "it fades after a rest");
}

/// How many times 20 resizes run the view.
fn resize_views(rt: &mut Runtime<App>) -> u64 {
    let before = VIEWS.get();
    for i in 0..20 {
        rt.set_viewport((1280.0 - 4.0 * i as f32, 820.0), 2.0);
        rt.frame(DT);
    }
    frames(rt, 0.2);
    VIEWS.get() - before
}

/// "Show scroll bars: Always" keeps it showing at rest, and a resize still lays the last tree
/// out again without running the view (the old bar ran it on every resize frame).
#[test]
fn always_keeps_it_showing_and_resizes_run_no_view() {
    let mut rt = app(1.0, ScrollBars::Always);
    assert!((thumb(&rt, KEY).opacity - 1.0).abs() < 1e-3);
    let always = resize_views(&mut rt);
    let hidden = resize_views(&mut app(1.0, ScrollBars::WhenScrolling));
    assert_eq!((always, hidden), (0, 0), "views run by 20 resizes, with Always and with the bar hidden");
}

#[test]
fn a_streaming_reply_doesnt_show_it() {
    let mut rt = app(1.0, ScrollBars::WhenScrolling);
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
        for _ in 0..6 {
            rt.frame(DT);
            assert!(!shown(&rt), "following its end as text arrives isn't a scroll ({i})");
        }
    }
}

/// At the end, the thumb sits at the track's foot, inside the viewport, zoomed or not.
#[test]
fn the_thumb_sits_inside_the_viewport_at_any_zoom() {
    for zoom in [1.0, 1.6] {
        let rt = app(zoom, ScrollBars::Always);
        let (t, v) = (thumb(&rt, KEY).rect, find(&rt, KEY).rect);
        assert!(t[0] + t[2] <= v[0] + v[2] + 0.5 && t[0] > v[0] + v[2] - 20.0 * zoom, "zoom {zoom}: thumb {t:?} in {v:?}");
        assert!((t[1] + t[3] - (v[1] + v[3])).abs() < 4.0 * zoom, "zoom {zoom}: thumb {t:?} ends at the foot of {v:?}");
    }
}

/// Settings scrolls with the same bar, at its own right edge.
#[test]
fn settings_has_the_bar() {
    let mut rt = app(1.0, ScrollBars::Always);
    rt.update(|s, _| s.open_settings());
    frames(&mut rt, 0.5);
    let v = find(&rt, "settings").rect;
    let t = thumb(&rt, "settings");
    assert!(t.opacity > 0.99, "settings overflows, so Always shows its bar");
    assert!(t.rect[0] + t.rect[2] <= v[0] + v[2] + 0.5 && t.rect[0] > v[0] + v[2] - 20.0, "thumb {:?} at the edge of {v:?}", t.rect);
}

/// Coming back to a session that was left scrolled up doesn't flash its bar.
#[test]
fn reopening_a_viewport_doesnt_show_it() {
    let mut rt = app(1.0, ScrollBars::WhenScrolling);
    wheel(&mut rt, 600.0);
    frames(&mut rt, 2.5);
    assert!(!shown(&rt));
    rt.update(|s, _| s.open_settings());
    frames(&mut rt, 0.5);
    rt.update(|s, _| s.close_settings());
    for _ in 0..60 {
        rt.frame(DT);
        assert!(!shown(&rt), "the timeline finding its place isn't a scroll");
    }
}

#[test]
fn jumping_to_latest_doesnt_show_it() {
    let mut rt = app(1.0, ScrollBars::WhenScrolling);
    wheel(&mut rt, 600.0);
    frames(&mut rt, 2.5);
    assert!(!shown(&rt));
    rt.update(|_, tx| tx.scroll_to_bottom(KEY));
    for _ in 0..120 {
        rt.frame(DT);
        assert!(!shown(&rt), "the jump isn't Kit scrolling");
    }
}
