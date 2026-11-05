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

/// A turn still running that has used `tools` so far, each step its own message (`split`) or
/// all in one.
fn live_turn(tools: &[(&str, &str)], split: bool) -> opencode_kit::model::Transcript {
    let tool = |(id, name): &(&str, &str)| serde_json::json!({
        "type": "tool", "id": id, "name": name, "state": {"status": "completed", "input": {"command": "ls", "filePath": "a.rs"}},
    });
    let mut data = vec![serde_json::json!({"id": "msg_u1", "type": "user", "time": {"created": 1_700_000_000_000_i64}, "text": "Make the card denser."})];
    let steps: Vec<Vec<serde_json::Value>> = if split { tools.iter().map(|t| vec![tool(t)]).collect() } else { vec![tools.iter().map(tool).collect()] };
    let n = steps.len();
    for (i, parts) in steps.into_iter().enumerate() {
        let created = 1_700_000_001_000_i64 + i as i64 * 1000;
        let time = if i + 1 < n { serde_json::json!({"created": created, "completed": created + 900}) } else { serde_json::json!({"created": created}) };
        data.push(serde_json::json!({"id": format!("msg_a{i}"), "type": "assistant", "time": time, "content": parts}));
    }
    let messages = opencode_kit::model::parse_messages(&serde_json::json!({ "data": data }));
    opencode_kit::model::Transcript { session: crate::fixture::SESSION_ID.into(), messages }
}

/// The widest right-edge fade on the glyphs drawn inside `rect` (points), and how many there are.
fn right_fade(rt: &Runtime<Host>, rect: [f32; 4], scale: f32) -> (usize, f32) {
    let [x, y, w, h] = rect.map(|v| v * scale);
    rt.instances.iter()
        .filter(|i| i.params[1] == rune::render::KIND_GLYPH)
        .filter(|i| i.rect[0] >= x - 1.0 && i.rect[0] < x + w && i.rect[1] >= y - h && i.rect[1] < y + 2.0 * h)
        .map(|i| rune::render::unpack_fade(i.fade).0[2])
        .fold((0, 0.0), |(n, most), f| (n + 1, f32::max(most, f)))
}

/// t1384: "Used 2 Read, Shell" stays whole on every frame while the count rolls from 1 to 2,
/// rolls again to 3 before it lands, and the label crossfades to each new name.
#[test]
fn a_group_label_that_fits_never_fades_while_its_count_rolls() {
    let mut faded = Vec::new();
    for (scale, zoom, split) in [(1.0, 1.0, false), (2.0, 1.3, false), (1.0, 1.0, true), (2.0, 1.0, true), (2.0, 1.3, true)] {
        FEED.with_borrow_mut(Vec::clear);
        let mut host = Host { zoom, busy: true, ..Host::default() };
        start_motion(&mut host);
        *host.talk.borrow_mut() = Some(live_turn(&[("prt_r", "read")], split));
        let mut rt = Runtime::new(host, view, (1000.0, 1400.0), Env::new(true, scale));
        rt.poll = Some(live_poll);
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
        }
        feed(move |s| *s.talk.borrow_mut() = Some(live_turn(&[("prt_r", "read"), ("prt_s", "bash")], split)));
        let mut names = std::collections::BTreeSet::new();
        for f in 0..120 {
            if f == 8 {
                // Mid-roll: another step lands before the 2 has rolled in.
                feed(move |s| *s.talk.borrow_mut() = Some(live_turn(&[("prt_r", "read"), ("prt_s", "bash"), ("prt_g", "grep")], split)));
            }
            rt.frame(1.0 / 120.0);
            let Some(t) = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("title-") && k.contains(','))) else { continue };
            names.insert(t.text.clone().unwrap_or_default());
            let (glyphs, fade) = right_fade(&rt, t.rect, scale);
            if glyphs > 0 && fade > 0.0 {
                faded.push(format!("{scale}x at {zoom} zoom, split {split}, frame {f}: {:?} drawn {:?} laid out {:?}", t.text, t.rect, t.target));
            }
        }
        assert!(names.len() == 2, "{scale}x at {zoom} zoom: the label took both new names: {names:?}");
    }
    assert!(faded.is_empty(), "the label faded while it fit: {faded:#?}");
}
