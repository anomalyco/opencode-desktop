//! Checks a live run's post rate (`widgets-live.md`, Post rate) against the TS host's tests: a
//! burst of renders, a frame request under load, a 120 Hz game; and draw posts without a string
//! key. Renders and draws take a token of a bucket of 8 refilled at the rate; past it they wait,
//! coalescing (the render, a canvas's draw), and go out one per token, all at once when the run
//! ends. The first two run at 10 a second, as the TS tests do, so the waits are easy to see. The
//! client keeps only the latest of the renders it hasn't taken yet, so where renders come in a
//! row this sees the last of them. The repo has no automated tests (AGENTS.md), so this is a
//! program to run by hand: `cargo run --release -p opencode-widget --example live_rate_check`
//! (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::{Frame, Node, fixtures};
use serde_json::{Value, json};

fn start(source: &str, per_second: f64) -> Live {
    let program = Arc::new(LiveProgram::compile(&format!("{}\n{source}", fixtures::PRELUDE)).expect("compiles"));
    let setup = Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) };
    Live::start_rated(program, json!({}), setup, per_second)
}

/// What a run posted, each with when it was taken.
#[derive(Default)]
struct Seen(Vec<(Instant, Post)>);

impl Seen {
    /// Takes what `live` posted until `until` holds or `within` passes; whether it held.
    fn until(&mut self, live: &Live, within: Duration, until: impl Fn(&Seen, Option<&Ended>) -> bool) -> bool {
        let end = Instant::now() + within;
        loop {
            let ended = live.ended();
            let now = Instant::now();
            self.0.extend(live.take().into_iter().map(|p| (now, p)));
            if until(self, ended.as_ref()) {
                return true;
            }
            if now >= end {
                return false;
            }
            std::thread::sleep(Duration::from_micros(500));
        }
    }

    /// The texts of the text views rendered (a column's first text), in order.
    fn texts(&self) -> Vec<String> {
        self.0
            .iter()
            .filter_map(|(_, p)| match p {
                Post::Render(Node::Text { text, .. }) => Some(text.clone()),
                Post::Render(Node::Column { children, .. }) => children.iter().find_map(|c| match c {
                    Node::Text { text, .. } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect()
    }

    fn draws(&self) -> Vec<(String, Value)> {
        self.0
            .iter()
            .filter_map(|(_, p)| match p {
                Post::Draw { id, commands } => Some((id.clone(), commands.iter().map(opencode_widget::Command::to_value).collect())),
                _ => None,
            })
            .collect()
    }

    fn count(&self, f: impl Fn(&Post) -> bool) -> usize {
        self.0.iter().filter(|(_, p)| f(p)).count()
    }
}

fn frame(t: f64) -> Frame {
    Frame { t, width: 100.0, height: 100.0, scale: 2.0, ..Default::default() }
}

/// A draw of `alpha` as the client gets it (an `f32`).
fn alpha(a: f64) -> Value {
    json!([["alpha", f64::from(a as f32)]])
}

/// 30 renders at 10 a second: the 8 of the burst go out at once (this sees the last, `7`), the
/// other 22 wait and replace each other; `29` goes out alone a tenth of a second later. Ending
/// straight after the burst, by an answer or by returning, sends `29` first.
fn burst() {
    let live = start("async function main() { for (let i = 0; i < 30; i++) widget.render(ui.text(String(i))); await widget.sleep(1000); widget.submit(1) }", 10.0);
    let mut seen = Seen::default();
    assert!(seen.until(&live, Duration::from_secs(2), |s, _| s.texts().last().is_some_and(|t| t == "7")), "the burst goes out: {:?}", seen.texts());
    let burst = seen.0.last().unwrap().0;
    assert!(seen.until(&live, Duration::from_secs(2), |s, _| s.texts().last().is_some_and(|t| t == "29")), "29 goes out: {:?}", seen.texts());
    let after = seen.0.last().unwrap().0.duration_since(burst);
    assert!(seen.texts().iter().all(|t| t.parse::<usize>().unwrap() <= 7 || t == "29"), "renders 8 to 28 never go out: {:?}", seen.texts());
    assert!((Duration::from_millis(85)..Duration::from_millis(170)).contains(&after), "29 goes out a token (100 ms) after the burst: {after:?}");
    assert!(seen.until(&live, Duration::from_secs(3), |s, _| s.count(|p| matches!(p, Post::Submit(_))) == 1), "it submits");
    assert_eq!(seen.texts().last().map(String::as_str), Some("29"), "nothing more");
    assert!(matches!(live.ended(), None | Some(Ended::Returned)), "no error: {:?}", live.ended());
    println!("ok   30 renders at 10/s: the burst of 8 at once, then render 29 alone {} ms later, then the submit", after.as_millis());

    for last in ["widget.submit(1)", "return"] {
        let live = start(&format!("async function main() {{ for (let i = 0; i < 30; i++) widget.render(ui.text(String(i))); {last} }}"), 10.0);
        let mut seen = Seen::default();
        let answered = |s: &Seen| s.count(|p| matches!(p, Post::Submit(_))) == 1;
        assert!(seen.until(&live, Duration::from_secs(2), |s, e| if last == "return" { e.is_some() } else { answered(s) }), "it ends");
        let texts = seen.texts();
        assert!(texts.iter().all(|t| t.parse::<usize>().unwrap() <= 7 || t == "29"), "{last}: {texts:?}");
        let tail: Vec<String> = seen.0.iter().rev().take(2).rev().map(|(_, p)| match p {
            Post::Submit(_) => "submit".to_owned(),
            Post::Render(Node::Text { text, .. }) => text.clone(),
            other => format!("{other:?}"),
        }).collect();
        let want = if last == "return" { vec!["29"] } else { vec!["29", "submit"] };
        assert_eq!(tail[tail.len() - want.len()..], want[..], "{last}: 29 goes out before the end");
        if last == "return" {
            assert_eq!(live.ended(), Some(Ended::Returned));
        }
        println!("ok   ending by {last} straight after the burst sends render 29 first: {texts:?}{}", if last == "return" { "" } else { ", submit" });
    }
}

/// A render, then 20 draws on canvas `a` and 20 on `b`, alternating, at 10 a second: 7 draws go out
/// at once (the render took a token), then the latest `b` and the latest `a`, a tenth of a second
/// apart. The frame request made meanwhile goes out at once, and the frame's render waits its turn.
fn draws() {
    let live = start(
        r#"async function main() {
          widget.render(ui.column([ui.canvas({ key: "a", height: 1 }), ui.canvas({ key: "b", height: 1 })]))
          for (let i = 0; i < 20; i++) { widget.draw("a", [["alpha", i / 100]]); widget.draw("b", [["alpha", i / 50]]) }
          const frame = await widget.frame()
          widget.render(ui.text("frame " + frame.t))
        }"#,
        10.0,
    );
    let mut seen = Seen::default();
    assert!(seen.until(&live, Duration::from_secs(2), |s, _| s.count(|p| *p == Post::Frame) == 1), "it asks for a frame");
    let draws = seen.draws().len();
    assert_eq!(draws, 7, "the frame request goes out at once, while draws wait");
    // The waiting draws go out a token apart while the frame request waits for its frame.
    seen.until(&live, Duration::from_millis(250), |_, _| false);
    let want = [("0/:b".to_owned(), alpha(19.0 / 50.0)), ("0/:a".to_owned(), alpha(19.0 / 100.0))];
    assert_eq!(seen.draws()[7..], want[..], "then the latest draw of b, then of a");
    let at: Vec<Instant> = seen.0.iter().filter(|(_, p)| matches!(p, Post::Draw { .. })).map(|(t, _)| *t).collect();
    let apart = at[8].duration_since(at[7]);
    assert!((Duration::from_millis(85)..Duration::from_millis(170)).contains(&apart), "a token (100 ms) apart: {apart:?}");
    live.frame(&frame(1.0));
    assert!(seen.until(&live, Duration::from_secs(2), |_, e| e.is_some()), "it returns");
    assert_eq!(seen.draws().len(), 9, "no more draws");
    assert_eq!((seen.texts().last().map(String::as_str), live.ended()), (Some("frame 1"), Some(Ended::Returned)), "the frame's render goes out before the end");
    println!("ok   draws at 10/s: 7 at once, the frame request at once, then b's latest and a's {} ms apart, then the frame's render", apart.as_millis());
}

/// A game at 120 Hz posting a render and two draws a frame (360 a second) over 120 frames, at the
/// real rate (240 a second): every frame request goes out, at most a second's tokens and the burst
/// of counted posts go out while it runs (and the last frame's at the end), and it shows its last
/// frame.
fn game() {
    let live = start(
        r#"async function main() {
          for (let n = 0; n < 120; n++) {
            const frame = await widget.frame()
            widget.render(ui.column([ui.text("n " + n), ui.canvas({ key: "a", height: 1 }), ui.canvas({ key: "b", height: 1 })]))
            widget.draw("a", [["alpha", n / 1000]])
            widget.draw("b", [["alpha", n / 200]])
          }
        }"#,
        opencode_widget::live::POSTS_PER_SECOND,
    );
    let counted = |p: &Post| matches!(p, Post::Render(_) | Post::Draw { .. });
    let mut seen = Seen::default();
    let began = Instant::now();
    for n in 0..120 {
        assert!(seen.until(&live, Duration::from_secs(2), |s, _| s.count(|p| *p == Post::Frame) == n + 1), "frame request {n} goes out");
        live.frame(&frame(n as f64 / 120.0));
        let next = began + Duration::from_secs_f64((n + 1) as f64 / 120.0);
        seen.until(&live, next.saturating_duration_since(Instant::now()), |_, _| false);
    }
    let running = began.elapsed();
    let during = seen.count(counted);
    assert!(seen.until(&live, Duration::from_secs(2), |_, e| e.is_some()), "it returns");
    let most = 8 + (running.as_secs_f64() * 240.0).ceil() as usize;
    assert!(during > 200 && during <= most, "{during} counted posts in {running:?}: over 200, at most {most}");
    assert!(seen.count(counted) <= most + 3, "the last frame's posts at the end, at most 3");
    assert_eq!(seen.count(|p| *p == Post::Frame), 120, "every frame request goes out");
    assert_eq!(seen.texts().last().map(String::as_str), Some("n 119"), "the last view is the last frame's");
    let last = |id: &str| seen.draws().into_iter().rev().find(|(i, _)| i == id).map(|(_, c)| c);
    assert_eq!((last("0/:a"), last("0/:b")), (Some(alpha(119.0 / 1000.0)), Some(alpha(119.0 / 200.0))), "the drawings are the last frame's");
    assert_eq!(live.ended(), Some(Ended::Returned));
    println!(
        "ok   a 120 Hz game: 120 frame requests, {during} counted posts in {:.2} s (at most {most}), {} with the end, last frame shown",
        running.as_secs_f64(),
        seen.count(counted)
    );
}

/// A draw post whose key is missing or not a string is an invalid post: the run ends with the
/// program error `A draw post needs a string key`.
fn keys() {
    for post in [r#"__host.post({ type: "draw", id: "0", commands: [] })"#, r#"__host.post({ type: "draw", key: 1, commands: [] })"#] {
        let live = start(&format!("async function main() {{ widget.render(ui.canvas({{ key: \"c\", height: 1 }})); try {{ {post} }} catch (e) {{ widget.render(ui.text(\"caught\")) }} }}"), 240.0);
        let mut seen = Seen::default();
        assert!(seen.until(&live, Duration::from_secs(2), |_, e| e.is_some()), "it ends");
        assert_eq!(live.ended(), Some(Ended::Failed("A draw post needs a string key".into())), "{post}");
        assert!(seen.draws().is_empty() && seen.texts().is_empty(), "{post}: nothing drawn, nothing caught");
        println!("ok   {post} ends the run: A draw post needs a string key");
    }
}

fn main() {
    burst();
    draws();
    game();
    keys();
}
