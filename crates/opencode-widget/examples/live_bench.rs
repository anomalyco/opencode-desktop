//! Runs a live widget template against a fake 60 Hz client and times its turns: from a frame delivered
//! to its next frame request. For widget templates that run frames on their own (not ones waiting on a
//! title screen). `cargo run --release -p opencode-widget --example live_bench --
//! <widget template.js> ['<data json>'] [frames] [--prelude-tree]`. With `--prelude-tree` the
//! prelude builds the views' trees (as with a server's older prelude) instead of this host.
#![allow(clippy::print_stdout, clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::{Frame, fixtures};

fn main() {
    let prelude_tree = std::env::args().any(|a| a == "--prelude-tree");
    let mut args = std::env::args().skip(1).filter(|a| a != "--prelude-tree");
    let source = std::fs::read_to_string(args.next().expect("a widget template source path")).expect("reads the source");
    let data = args.next().map_or(serde_json::json!({}), |d| serde_json::from_str(&d).expect("data is JSON"));
    let frames: u32 = args.next().map_or(900, |n| n.parse().expect("a frame count"));
    // Hidden from the native check (`LiveProgram::compile`), the same prelude builds the trees.
    let prelude = if prelude_tree { fixtures::PRELUDE.replace("function __widget_find(", "function __widget_find (") } else { fixtures::PRELUDE.to_owned() };
    let program = Arc::new(LiveProgram::compile(&format!("{prelude}\n{source}")).expect("compiles"));
    let setup = Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) };
    let live = Live::start(program, data, setup);
    let frame = Frame { id: None, t: 0.0, dt: 1.0 / 60.0, width: 1200.0, height: 800.0, scale: 2.0, pointer: None, dark: true, fullscreen: false, close: None };
    let (mut delivered, mut draws, mut turns) = (0u32, 0usize, Vec::new());
    let mut turn_start = Instant::now();
    while delivered < frames {
        // A widget template waiting for input (a title screen) asks for no frames.
        if turn_start.elapsed() > Duration::from_secs(5) {
            println!("asked for no frame for 5 s after {delivered} frames");
            break;
        }
        if let Some(end) = live.ended() {
            println!("ended after {delivered} frames: {end:?} (at {:?})", live.failed_at());
            break;
        }
        if !live.news() {
            std::thread::sleep(Duration::from_micros(200));
            continue;
        }
        let mut want = false;
        for post in live.take() {
            match post {
                Post::Frame => want = true,
                Post::Draw { commands, .. } => draws = commands.len(),
                Post::Capture(_) | Post::Render(_) => {}
                other => println!("post: {other:?}"),
            }
        }
        if want {
            if delivered > 0 {
                turns.push(turn_start.elapsed().as_secs_f64() * 1000.0);
            }
            std::thread::sleep(Duration::from_millis(16));
            turn_start = Instant::now();
            live.frame(&frame);
            delivered += 1;
        }
    }
    live.stop();
    turns.sort_by(f64::total_cmp);
    let at = |q: f64| turns.get(((turns.len() as f64 - 1.0) * q) as usize).copied().unwrap_or(0.0);
    println!("frames {delivered}, last draw {draws} commands, turn ms p50 {:.2} p95 {:.2} max {:.2}", at(0.5), at(0.95), at(1.0));
}
