//! A bare host that mounts [`crate::read_only`] and runs [`crate::poll`] gets the timeline's
//! motion with nothing else: streamed lines open over frames, a fold glides, a new row grows in.

use std::cell::RefCell;

use rune::{Env, Runtime, Tx};

use crate::story::{Host, arrive, poll, start_motion, stream, view};

type Feed = Box<dyn FnOnce(&mut Host)>;

thread_local! {
    static FEED: RefCell<Vec<Feed>> = RefCell::new(Vec::new());
}

/// As a live host takes its stream: what arrived lands in `poll`, not in a handler.
fn live_poll(s: &mut Host, now: f64, tx: &mut Tx) -> bool {
    let fed: Vec<Feed> = FEED.with_borrow_mut(std::mem::take);
    let heard = !fed.is_empty();
    for f in fed {
        f(s);
    }
    poll(s, now, tx) | heard
}

fn feed(f: impl FnOnce(&mut Host) + 'static) {
    FEED.with_borrow_mut(|q| q.push(Box::new(f)));
}

fn runtime() -> Runtime<Host> {
    FEED.with_borrow_mut(Vec::clear);
    let mut host = Host::default();
    start_motion(&mut host);
    let mut rt = Runtime::new(host, view, (1000.0, 1400.0), Env::new(true, 1.0));
    rt.poll = Some(live_poll);
    rt.frame(0.0);
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
    }
    rt
}

fn height(rt: &Runtime<Host>, key: &str) -> Option<f32> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map(|e| e.rect[3])
}

/// Heights of `key` over the next 60 frames at 120 Hz.
fn heights(rt: &mut Runtime<Host>, key: &str) -> Vec<f32> {
    (0..60).map(|_| {
        rt.frame(1.0 / 120.0);
        height(rt, key).unwrap_or(0.0)
    }).collect()
}

/// Whether `hs` passes through somewhere between `from` and its last value, rather than
/// landing in one frame.
fn moves(from: f32, hs: &[f32]) -> bool {
    let to = *hs.last().unwrap();
    let (lo, hi) = (from.min(to) + 1.0, from.max(to) - 1.0);
    (to - from).abs() > 8.0 && hs.iter().any(|h| *h > lo && *h < hi)
}

#[test]
fn a_streamed_line_opens_over_frames() {
    let mut rt = runtime();
    let key = "t-msg_m2-0";
    let before = height(&rt, key).expect("the streaming reply's row");
    feed(|s| stream(s, "\n\nA second paragraph arrives.\n\nAnd a third one under it."));
    let hs = heights(&mut rt, key);
    assert!(moves(before, &hs), "the reply's row should open over frames: {before} then {hs:?}");
}

#[test]
fn a_tool_group_folds_open_over_frames() {
    let mut rt = runtime();
    let key = "c-prt_m1";
    let before = height(&rt, key).expect("the tool group's row");
    rt.update(|s, tx| crate::disclosure::toggle_group::<Host>(key.to_owned())(s, tx));
    let hs = heights(&mut rt, key);
    assert!(moves(before, &hs), "the group should glide open: {before} then {hs:?}");
}

#[test]
fn a_new_row_grows_in() {
    let mut rt = runtime();
    feed(arrive);
    rt.frame(1.0 / 120.0);
    assert!(rt.state.arrivals.glow_at("msg_m3").is_some(), "a prompt from another client glows");
    let hs = heights(&mut rt, "u-msg_m3");
    assert!(moves(0.0, &hs), "the new prompt's row should grow in: {hs:?}");
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("glow")), "its bubble shows the glow");
}
