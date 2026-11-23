//! Checks views at the depth limit (`widgets.md`, Keys and depth): a view of 100 levels, with a
//! `select`'s options at level 100 (value depth 202 in a live render), steps and renders; 101
//! levels fail in phase `view` with the spec's message. Then it measures the smallest stack such a
//! step needs, by bisection in child processes (an overflow kills only the child). The repo has no
//! automated tests (AGENTS.md), so this is a program to run by hand:
//! `cargo run [--release] -p opencode-widget --example depth_check` (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::runner::Error;
use opencode_widget::{Node, Program, StepInput, fixtures, tree};
use serde_json::json;

/// A view `levels` deep: columns around a `select` whose options are the deepest values.
fn view(levels: usize) -> String {
    format!(
        "function deep() {{ let node = ui.select([{{ value: \"a\", label: \"A\" }}], {{ onChange: (s) => s }}); for (let i = 1; i < {levels}; i++) node = ui.column([node]); return node }}"
    )
}

fn step(levels: usize) -> Result<Node, Error> {
    let program = Program::compile(&format!("{}\n{}\nfunction view() {{ return deep() }}", fixtures::PRELUDE, view(levels))).expect("compiles");
    program.step(&StepInput::Init { data: json!(null) }).map(|o| o.view)
}

/// How a live run rendering a view `levels` deep goes: its view, or how it ended.
fn live(levels: usize) -> Result<Node, Option<Ended>> {
    let source = fixtures::served("live", &format!("{}\nasync function main() {{ widget.render(deep()); await widget.sleep(500) }}", view(levels)));
    let program = Arc::new(LiveProgram::compile(&source).expect("compiles"));
    let live = Live::start(program, json!({}), Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) });
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        for p in live.take() {
            if let Post::Render(node) = p {
                return Ok(node);
            }
        }
        if let Some(e) = live.ended() {
            return Err(Some(e));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Err(None)
}

/// The levels of `node`: 1 for a leaf, one more per column.
fn levels(node: &Node) -> usize {
    1 + node.children().iter().map(levels).max().unwrap_or(0)
}

/// Child mode: one step of the deepest view on a thread of `bytes` of stack; exits 0 when it ran.
fn child(bytes: usize) -> ! {
    let ran = std::thread::Builder::new().stack_size(bytes).spawn(|| step(tree::LEVELS).is_ok_and(|n| levels(&n) == tree::LEVELS)).unwrap().join();
    std::process::exit(if matches!(ran, Ok(true)) { 0 } else { 1 });
}

/// The smallest stack, to 4 KB, a step of the deepest view runs on.
fn smallest() -> usize {
    let me = std::env::current_exe().unwrap();
    let runs = |bytes: usize| std::process::Command::new(&me).arg(bytes.to_string()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    let (mut low, mut high) = (16 << 10, 16 << 20);
    assert!(runs(high), "a step of 100 levels runs on 16 MB");
    while high - low > 4 << 10 {
        let mid = (low + high) / 2;
        if runs(mid) { high = mid } else { low = mid }
    }
    high
}

fn main() {
    if let Some(bytes) = std::env::args().nth(1) {
        child(bytes.parse().unwrap());
    }
    let deepest = tree::LEVELS;
    match step(deepest) {
        Ok(node) if levels(&node) == deepest => println!("ok   a step of {deepest} levels runs"),
        other => panic!("a step of {deepest} levels: {other:?}"),
    }
    assert_eq!(step(deepest + 1), Err(Error::Output(tree::DEEP.into())), "a step of 101 levels fails the view");
    println!("ok   a step of {} levels fails the view: {}", deepest + 1, tree::DEEP);
    match live(deepest) {
        Ok(node) if levels(&node) == deepest => println!("ok   a live render of {deepest} levels shows, on the live thread"),
        other => panic!("a live render of {deepest} levels: {other:?}"),
    }
    assert_eq!(live(deepest + 1), Err(Some(Ended::View(tree::DEEP.into()))), "a live render of 101 levels ends the run");
    println!("ok   a live render of {} levels ends the run in phase view: {}", deepest + 1, tree::DEEP);
    let need = smallest();
    println!("     a step of {deepest} levels needs {} KB of stack ({})", need >> 10, if cfg!(debug_assertions) { "debug" } else { "release" });
}
