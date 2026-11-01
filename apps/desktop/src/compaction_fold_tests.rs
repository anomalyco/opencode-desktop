//! The quiet Compacted divider opening its summary (p162): it holds where it was pressed, at the
//! timeline's end too, sticks to the top while the summary scrolls under it, shuts from there
//! without leaving the screen, and its label lights up as one word.

use opencode_kit::model::{Assistant, Message, Part, ToolStatus};
use rune::{ElementInfo, Runtime};
use serde_json::json;

use crate::App;
use crate::compaction::Piece;
use crate::compaction_stories::{LONG, add_long_summary};
use crate::motion_gate::SID;

const DT: f64 = 1.0 / 60.0;

fn runtime() -> Runtime<App> {
    let mut app = App::fixture_session();
    add_long_summary(&mut app, SID);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..60 {
        rt.frame(DT);
    }
}

fn find(rt: &Runtime<App>, key: &str) -> ElementInfo {
    let row = Piece::Quiet.key(LONG);
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key) && e.path.contains(&row)).unwrap_or_else(|| panic!("the divider's {key}"))
}

/// The divider with its padding: what holds and sticks.
fn bar(rt: &Runtime<App>) -> ElementInfo {
    find(rt, "bar")
}

fn viewport(rt: &Runtime<App>) -> [f32; 4] {
    let key = format!("scroll-{SID}");
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str())).map(|e| e.rect).expect("the timeline")
}

fn open(rt: &Runtime<App>) -> bool {
    rt.state.expanded.contains(&format!("compaction-{LONG}"))
}

fn at_bottom(rt: &Runtime<App>) -> bool {
    rt.scroll_at_bottom(&format!("scroll-{SID}")).expect("the timeline scrolls")
}

/// A click on the divider's label, where Kit pressed.
fn press(rt: &mut Runtime<App>) {
    let b = bar(rt);
    let (x, y) = (b.rect[0] + b.rect[2] / 2.0, b.visible[1] + b.visible[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// Wheels the timeline `dy` points down its content.
fn scroll_down(rt: &mut Runtime<App>, dy: f32) {
    let v = viewport(rt);
    rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, -dy);
    settle(rt);
}

#[test]
fn opening_at_the_bottom_holds_the_divider_and_pushes_the_end_down() {
    let mut rt = runtime();
    assert!(at_bottom(&rt), "the timeline starts at its end");
    let y = bar(&rt).rect[1];
    assert!(y > viewport(&rt)[1] && y < viewport(&rt)[1] + viewport(&rt)[3], "the divider shows near the end: {y}");
    press(&mut rt);
    assert!(open(&rt), "the press opens the summary");
    let after = bar(&rt).rect[1];
    assert!((after - y).abs() < 1.0, "the divider stays under the pointer: {y} -> {after}");
    assert!(!at_bottom(&rt), "the summary pushes the end down rather than growing up");
}

/// The other disclosures on the timeline hold the same way: a thought's "Thought for Ns" opened
/// at the timeline's end stays under the pointer.
#[test]
fn a_thought_opened_at_the_bottom_holds_its_trigger() {
    let mut app = App::fixture_session();
    let t = app.transcripts.get_mut(SID).expect("session");
    t.messages.push(Message::User { id: "msg_zzthought0".into(), text: "Why did it reconnect?".into() });
    let body = "The socket closed while the laptop slept, and the backoff restarted from its base. ".repeat(24);
    let parts = vec![
        Part::Reasoning { text: body, started: 0, completed: Some(9_000) },
        Part::Tool { id: "call_zzthought".into(), name: "shell".into(), title: "tail log".into(), status: ToolStatus::Completed, input: json!({}), files: vec![], metadata: json!(null) },
        Part::Text { text: "The laptop slept mid-stream.".into() },
    ];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzthought1".into(), completed: Some(1), parts, ..Default::default() }));
    app.expanded.insert("c-msg_zzthought1-0".into());
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    settle(&mut rt);
    let trigger = |rt: &Runtime<App>| {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("trigger") && e.path.contains("/r-msg_zzthought1-")).expect("the thought's trigger")
    };
    assert!(at_bottom(&rt), "the timeline starts at its end");
    let b = trigger(&rt);
    let (x, y) = (b.rect[0] + 40.0, b.rect[1] + b.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(rt.state.expanded.iter().any(|k| k.starts_with("r-msg_zzthought1-")), "the press opens the thought");
    let after = trigger(&rt).rect[1];
    assert!((after - b.rect[1]).abs() < 1.0, "the thought's trigger stays under the pointer: {} -> {after}", b.rect[1]);
}

#[test]
fn opening_mid_page_holds_the_divider() {
    let mut rt = runtime();
    scroll_down(&mut rt, -100.0);
    assert!(!at_bottom(&rt), "scrolled up off the timeline's end");
    let y = bar(&rt).rect[1];
    press(&mut rt);
    assert!(open(&rt));
    let after = bar(&rt).rect[1];
    assert!((after - y).abs() < 1.0, "the divider stays under the pointer: {y} -> {after}");
}

#[test]
fn scrolled_into_the_open_summary_the_divider_sticks_and_shuts_from_there() {
    let mut rt = runtime();
    press(&mut rt);
    scroll_down(&mut rt, 800.0);
    let v = viewport(&rt);
    let b = bar(&rt);
    assert!(b.stuck > 0.0, "scrolled into the summary, the divider sticks to the top");
    assert!((b.rect[1] - v[1]).abs() < 1.0, "stuck at the timeline's top: {} vs {}", b.rect[1], v[1]);
    press(&mut rt);
    assert!(!open(&rt), "a press on the stuck divider shuts the summary");
    let b = bar(&rt);
    let v = viewport(&rt);
    assert!(b.rect[1] >= v[1] - 1.0 && b.rect[1] + b.rect[3] <= v[1] + v[3] + 1.0, "the divider stays on screen: {:?} in {v:?}", b.rect);
    assert!(b.visible[3] > b.rect[3] - 1.0, "and shows whole: {:?}", b.visible);
}

/// The brightest glyph drawn inside element `e`, as premultiplied RGB summed.
fn ink(rt: &Runtime<App>, e: &ElementInfo) -> f32 {
    let [x, y, w, h] = e.rect;
    rt.instances
        .iter()
        .filter(|q| {
            let (cx, cy) = (q.rect[0] + q.rect[2] / 2.0, q.rect[1] + q.rect[3] / 2.0);
            q.rect[2] < w * 0.9 && cx > x && cx < x + w && cy > y && cy < y + h
        })
        .map(|q| (q.fill[0] + q.fill[1] + q.fill[2]) * q.fill[3])
        .fold(0.0, f32::max)
}

/// "Compact" and its ending's ink.
fn word(rt: &Runtime<App>) -> (f32, f32) {
    (ink(rt, &find(rt, "stem")), ink(rt, &find(rt, "done")))
}

#[test]
fn compact_and_its_ending_light_up_together() {
    let mut rt = runtime();
    let rest = word(&rt);
    assert!(rest.0 > 0.0 && (rest.0 - rest.1).abs() < 0.02, "at rest the word is one colour: {rest:?}");
    let b = bar(&rt);
    rt.pointer_move(b.rect[0] + b.rect[2] / 2.0, b.rect[1] + b.rect[3] / 2.0);
    settle(&mut rt);
    let hovered = word(&rt);
    assert!(hovered.0 > rest.0 + 0.02, "hovered, \"Compact\" brightens: {rest:?} -> {hovered:?}");
    assert!((hovered.0 - hovered.1).abs() < 0.02, "hovered, the word is one colour: {hovered:?}");
    press(&mut rt);
    rt.pointer_move(1.0, 1.0);
    settle(&mut rt);
    let opened = word(&rt);
    assert!(opened.0 > rest.0 + 0.02, "open, \"Compact\" stays bright: {rest:?} -> {opened:?}");
    assert!((opened.0 - opened.1).abs() < 0.02, "open, the word is one colour: {opened:?}");
}
