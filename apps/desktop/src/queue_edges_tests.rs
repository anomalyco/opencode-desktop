//! The queue and what sits under it, the composer or the question dock, draw one outline:
//! no two ring edges run side by side within 2 px, and the queue's sides meet the composer's.

use rune::{Env, Runtime, TextEdit};
use serde_json::json;

use crate::App;

/// The elements that draw the panel's `border(0.5, p.ring)`.
const RINGED: &[&str] = &["queue-face", "composer"];
/// 2 px at 1×, 4 px at 2×.
const NEAR: f32 = 2.0;

/// One drawn edge of a ring: vertical (`x` constant) or horizontal, at `at`, spanning `from..to`.
#[derive(Debug)]
struct Edge {
    key: &'static str,
    vertical: bool,
    /// The top edge (for a horizontal edge).
    top: bool,
    at: f32,
    from: f32,
    to: f32,
}

/// The ring edges each ringed element draws this frame: its sides where a clip leaves them
/// visible, and no edge a clip cuts off.
fn edges(rt: &Runtime<App>) -> Vec<Edge> {
    let mut out = Vec::new();
    for e in rt.elements() {
        let Some(key) = RINGED.iter().copied().find(|k| e.key.as_deref() == Some(*k)) else { continue };
        if e.exiting || e.opacity < 0.01 {
            continue;
        }
        let [x, y, w, h] = e.rect;
        let [vx, vy, vw, vh] = e.visible;
        let (top, bottom) = (y.max(vy), (y + h).min(vy + vh));
        let (left, right) = (x.max(vx), (x + w).min(vx + vw));
        if bottom - top <= 0.01 || right - left <= 0.01 {
            continue;
        }
        let shown = |at: f32, lo: f32, hi: f32| at >= lo - 0.01 && at <= hi + 0.01;
        let sides = [(true, false, x, top, bottom, vx, vx + vw), (true, false, x + w, top, bottom, vx, vx + vw), (false, true, y, left, right, vy, vy + vh), (false, false, y + h, left, right, vy, vy + vh)];
        for (vertical, is_top, at, from, to, lo, hi) in sides {
            if shown(at, lo, hi) {
                out.push(Edge { key, vertical, top: is_top, at, from, to });
            }
        }
    }
    out
}

/// Fails on two edges of different rings running parallel within `NEAR` for more than a
/// ring's width, and on the queue's sides not reaching the composer's top edge. Returns
/// whether the queue's top edge is passing the composer's, as it does for a frame or two
/// when the queue opens or closes behind the composer.
fn one_outline(rt: &Runtime<App>, when: &str) -> bool {
    let edges = edges(rt);
    let mut passing = false;
    for (i, a) in edges.iter().enumerate() {
        for b in &edges[i + 1..] {
            if a.key == b.key || a.vertical != b.vertical || (a.at - b.at).abs() >= NEAR {
                continue;
            }
            let overlap = a.to.min(b.to) - a.from.max(b.from);
            if overlap <= 0.5 {
                continue;
            }
            let top = |e: &Edge, key: &str| e.key == key && e.top;
            if (top(a, "queue-face") && top(b, "composer")) || (top(b, "queue-face") && top(a, "composer")) {
                passing = true;
                continue;
            }
            panic!("{when}: {} and {} draw parallel edges {:.2} pt apart for {overlap:.2} pt: {a:?} {b:?}", a.key, b.key, (a.at - b.at).abs());
        }
    }
    let side = |key: &str| edges.iter().find(|e| e.key == key && e.vertical).map(|e| (e.from, e.to));
    if let (Some((from, to)), Some((top, _))) = (side("queue-face"), side("composer"))
        && to - from > 0.5
    {
        assert!((to - top).abs() < 0.25, "{when}: the queue's sides end at {to:.2}, the composer's start at {top:.2}");
    }
    passing
}

fn app(size: (f32, f32), scale: f32) -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::queue::scene(&mut app, 0, false);
    let mut rt = Runtime::new(app, crate::view::app, size, Env::new(true, scale));
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

/// `secs` of 120 Hz frames, each checked. The queue's top edge passes the composer's in at
/// most 3 frames (25 ms).
fn play(rt: &mut Runtime<App>, secs: f32, what: &str) {
    let mut passing = 0;
    for f in 0..(secs * 120.0) as usize {
        rt.frame(1.0 / 120.0);
        passing += one_outline(rt, &format!("{what}, frame {f}")) as usize;
    }
    assert!(passing <= 3, "{what}: the queue's top edge runs within {NEAR} pt of the composer's for {passing} frames");
}

fn event(rt: &mut Runtime<App>, ty: &str, data: serde_json::Value) {
    let at = rt.state.clock + 0.001;
    rt.state.playback.push_back((at, ty.into(), data));
}

fn sid(rt: &Runtime<App>) -> String {
    match &rt.state.route {
        crate::Route::Session(id) => id.clone(),
        _ => panic!("no open session"),
    }
}

fn queue_one(rt: &mut Runtime<App>, text: &str) {
    rt.update(|s, tx| {
        s.composer = TextEdit::new(text);
        s.submit(true, tx);
    });
}

fn deliver_oldest(rt: &mut Runtime<App>) {
    let id = sid(rt);
    let item = rt.state.inbox_for(&id).and_then(|i| i.queued().next()).map(|i| i.id.clone()).expect("a queued prompt");
    event(rt, "session.inbox.delivered", json!({ "sessionID": id, "inboxID": item }));
}

#[test]
fn a_settled_queue_shares_one_outline_with_the_composer_or_the_question() {
    // A question in the composer's place closes the queue, as Desktop's does.
    for (n, ask) in [(1, false), (3, false), (6, false)] {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::queue::scene(&mut app, n, ask);
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 2.0));
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.question_shown(), ask);
        assert!(!one_outline(&rt, &format!("{n} queued, question {ask}")), "{n} queued, question {ask}: settled, the queue's top edge is clear of the composer's");
        let sides = edges(&rt).into_iter().filter(|e| e.vertical).count();
        assert_eq!(sides, 4, "{n} queued, question {ask}: the queue's and the composer's sides show");
    }
}

#[test]
fn the_queue_and_the_dock_under_it_draw_one_outline_every_frame() {
    for (size, scale) in [((1280.0, 820.0), 1.0), ((1280.0, 820.0), 2.0), ((720.0, 820.0), 2.0)] {
        let at = format!("{size:?} at {scale}×");
        let mut rt = app(size, scale);
        let question = rt.state.fixture["question"].clone();
        let replied = json!({ "id": question["id"], "sessionID": question["sessionID"] });

        queue_one(&mut rt, "Then write up what changed in the PR description");
        play(&mut rt, 1.0, &format!("{at}: the queue opens over the composer"));
        assert!(rt.find("queue-face").is_some_and(|(_, y)| y > 0.0), "{at}: the queue shows");

        event(&mut rt, "form.created", question.clone());
        play(&mut rt, 1.0, &format!("{at}: the question opens under the queue"));
        assert!(rt.state.question_shown(), "{at}: the question shows");

        event(&mut rt, "form.replied", replied.clone());
        play(&mut rt, 1.0, &format!("{at}: the question closes under the queue"));

        for text in ["And keep the retry budget as it is", "Bump the version and tag it", "Then run the release"] {
            queue_one(&mut rt, text);
            play(&mut rt, 0.3, &format!("{at}: several queued"));
        }
        event(&mut rt, "form.created", question.clone());
        play(&mut rt, 1.0, &format!("{at}: the question opens under four queued"));

        for _ in 0..4 {
            deliver_oldest(&mut rt);
            play(&mut rt, 0.6, &format!("{at}: the queue empties over the question"));
        }
        event(&mut rt, "form.replied", replied);
        play(&mut rt, 1.0, &format!("{at}: the question closes"));
    }
}
