//! The fuzzer (`fuzz`, run by hand), and the runs it found, kept as replays.

use std::collections::{BTreeMap, HashMap};

use super::ops::{self, *};
use super::oracle::Kind;
use super::shrink::shrink;
use super::sim::Sim;
use super::world::{Outcome, run};

/// Plays `ops` from `seed` and returns how it went, printing the timeline.
fn replay(seed: u64, ops: &[Op]) -> Outcome {
    let out = run(seed, ops, false);
    println!("{}\n{}", ops::replay_code(seed, ops), out.trace);
    out
}

/// Fails with the timeline when the run fails at all.
fn passes(seed: u64, ops: &[Op]) {
    let out = replay(seed, ops);
    if let Some(f) = out.failure {
        panic!("{:?} at {:.2}s: {}\n\n{}\n\nlink:\n{}", f.kind, f.t, f.detail, out.trace, out.log.join("\n"));
    }
}

/// Fails unless the run fails with `kind`: for bugs still open, so a fix flips it.
fn fails_with(seed: u64, ops: &[Op], kind: Kind) {
    let out = replay(seed, ops);
    assert_eq!(out.failure.as_ref().map(|f| f.kind), Some(kind), "{}", out.trace);
}

/// `FUZZ_SEEDS` seeds from `FUZZ_FROM`: each failure is shrunk, and the smallest run of each
/// kind is printed and written to `captures/interaction-fuzz/report-<FUZZ_FROM>.txt`.
#[test]
#[ignore = "t1387: run by hand, FUZZ_SEEDS=200 cargo test -p opencode-desktop interaction_fuzz::tests::fuzz -- --ignored --nocapture"]
fn fuzz() {
    let seeds: u64 = std::env::var("FUZZ_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
    let from: u64 = std::env::var("FUZZ_FROM").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let budget: usize = std::env::var("FUZZ_SHRINK").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    // One entry per kind of failure and shape of its shrunk message, prompt names aside.
    let shape = |kind: Kind, detail: &str| format!("{kind:?}: {}", detail.chars().map(|c| if c.is_ascii_digit() { 'N' } else { c }).collect::<String>().replace("NN", "N"));
    let mut found: BTreeMap<String, (u64, Vec<Op>, usize)> = BTreeMap::new();
    // The unshrunk shape a seed failed with, to the entry its shrunk run went to.
    let mut shrunk_to: HashMap<String, String> = HashMap::new();
    let mut failed = 0;
    for seed in from..from + seeds {
        let ops = ops::generate(seed);
        let Some(f) = run(seed, &ops, true).failure else { continue };
        failed += 1;
        let raw = shape(f.kind, &f.detail);
        if let Some(e) = shrunk_to.get(&raw).and_then(|k| found.get_mut(k))
            && e.1.len() <= 5
        {
            e.2 += 1;
            continue;
        }
        let small = shrink(seed, &ops, f.kind, budget);
        let detail = run(seed, &small, true).failure.map_or(f.detail.clone(), |g| g.detail);
        println!("seed {seed}: {:?} in {} → {} steps: {detail}\n  {}", f.kind, ops.len(), small.len(), ops::replay_code(seed, &small));
        let key = shape(f.kind, &detail);
        shrunk_to.insert(raw, key.clone());
        let e = found.entry(key).or_insert((seed, small.clone(), 0));
        e.2 += 1;
        if small.len() < e.1.len() {
            (e.0, e.1) = (seed, small);
        }
    }
    let mut report = format!("{failed} of {seeds} seeds from {from} failed; {} kinds\n", found.len());
    for (kind, (seed, ops, n)) in &found {
        let out = run(*seed, ops, false);
        let detail = out.failure.as_ref().map(|f| f.detail.clone()).unwrap_or_else(|| "(passed on replay)".into());
        report.push_str(&format!("\n== {kind} ({n} seeds): {detail}\n{}\n{}\n", ops::replay_code(*seed, ops), out.trace));
    }
    println!("{report}");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../captures/interaction-fuzz");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("report-{from}.txt")), &report);
}

#[test]
fn a_plain_send_settles_clean() {
    passes(1, &[Send, Wait(10_000)]);
}

#[test]
fn a_steer_and_a_queued_prompt_settle_clean() {
    passes(2, &[Send, Wait(300), Send, Queue, Wait(20_000)]);
}

#[test]
fn the_same_seed_plays_the_same() {
    let ops = ops::generate(7);
    let (a, b) = (run(7, &ops, false), run(7, &ops, false));
    assert_eq!(a.trace, b.trace);
}

// Fixed bugs, kept as regressions.

/// t1361 (#347): prompts sent after a stop that left steers waiting jumped above them, then
/// reshuffled. With #347 reverted it settles as [p1 p4 p5 p2 p3] against the server's order.
#[test]
fn t1361_prompts_after_a_stop_keep_their_place() {
    passes(3, &[Send, Wait(500), Send, Send, Stop, Send, Send, Wait(20_000)]);
}

/// t1309: Stop showed for an idle session whose only work was a background helper.
#[test]
fn t1309_a_background_helper_alone_shows_send() {
    passes(4, &[Helper(40), Wait(35_000)]);
}

/// t1347 (#342): ↵ in an empty composer stops a run; a held ↵ after sending doesn't.
#[test]
fn t1347_empty_enter_stops_and_held_enter_doesnt() {
    passes(5, &[Send, Wait(500), EnterEmpty, Wait(5_000)]);
    passes(6, &[HoldEnter, Wait(10_000)]);
}

/// t1350: a send whose reply was lost said Sending… for good, though the server had it; and one
/// that never arrived never said it failed. The receipt check now settles both.
#[test]
fn t1350_a_lost_reply_settles_as_delivered() {
    passes(0xa, &[DropResponse, Send]);
}

// Open bugs (t1387), smallest runs the fuzzer found, in `bugs.tsv` too for `real.sh confirm`.
// Each fails today; a fix makes it pass, so take its `#[ignore]` off then.

/// A send is lost; the bubble rightly says Failed to send, but Working keeps spinning under it
/// on an idle session: the failed prompt stays last in the transcript, which reads as a turn
/// still waiting for its reply (`Transcript::busy`).
#[test]
#[ignore = "t1387: open bug, a failed prompt leaves Working up for good"]
fn failed_send_keeps_working() {
    fails_with(0x0, &[LoseWrite, Send], Kind::SettledWrong);
}

/// A send on a dead network times out and the toast says "OpenCode didn't answer in time".
#[test]
#[ignore = "t1387: open bug, a dead network is blamed on OpenCode"]
fn dead_send_blames_opencode() {
    fails_with(0x58, &[Dead, Send, Wait(53_339)], Kind::BlamesServer);
}

/// The network dies, ↵ stops the run, and the toast says "The server didn't confirm the stop".
#[test]
#[ignore = "t1387: open bug, a dead network is blamed on the server"]
fn dead_stop_blames_server() {
    fails_with(0x61, &[Send, Wait(1_500), Dead, EnterEmpty, Wait(7_500)], Kind::BlamesServer);
}

/// On a slow link another client's prompt reaches the server first; the screen keeps yours
/// above it for good.
#[test]
#[ignore = "t1387: open bug, a racing client's prompt shows in the wrong place"]
fn other_client_race() {
    fails_with(0x50, &[Slow, Other, Send], Kind::SettledWrong);
}

/// While a run goes, your steer and another client's both wait as Pending, yours first; the
/// server took theirs first, and when both land the bubbles swap places.
#[test]
#[ignore = "t1387: open bug, racing steers swap places when they land"]
fn racing_steers_swap() {
    fails_with(0x67, &[Other, Wait(500), Slow, Send, Other], Kind::Jump);
}

/// The open session's read failing for over 30 s puts up "Still trying" on the virtual clock.
/// Only a read already on its way when the link dies fails (the app reads the page when a step
/// starts), so the link dies a little after the send, at one of a few moments.
#[test]
fn a_long_silence_says_still_trying() {
    let mut traces = vec![];
    for ms in (40..=200).step_by(10) {
        let out = replay(8, &[Send, Wait(ms), Dead, Wait(120_000), Fine, Wait(60_000)]);
        if out.trace.contains("Still trying") {
            return;
        }
        traces.push(out.trace);
    }
    panic!("no moment showed Still trying:\n{}", traces.join("\n"));
}

/// Plays `FUZZ_OPS` once against the real server in the `FUZZ_REAL` file (`real.rs`), and
/// writes the timeline to `captures/interaction-fuzz/real-<FUZZ_NAME>.txt`.
#[test]
#[ignore = "t1387: needs a Drive server on a worker, FUZZ_REAL=server.json FUZZ_OPS='Send, Wait(500), Stop' cargo test -p opencode-desktop interaction_fuzz::tests::real_replay -- --ignored --nocapture"]
fn real_replay() {
    let cfg = super::real::Config::read(&std::env::var("FUZZ_REAL").expect("FUZZ_REAL: the server file"));
    let ops = ops::parse(&std::env::var("FUZZ_OPS").expect("FUZZ_OPS: the steps")).unwrap();
    let name = std::env::var("FUZZ_NAME").unwrap_or_else(|_| "replay".into());
    let out = super::real::run(&cfg, &ops);
    let verdict = match &out.failure {
        Some(f) => format!("REPRODUCED {:?} at {:.2}s: {}", f.kind, f.t, f.detail),
        None => "NOT REPRODUCED: the run passed on the real server".into(),
    };
    let text = format!("{name}: {verdict}\n{ops:?}\n\n{}\n\nproxy:\n{}\n", out.trace, out.log.join("\n"));
    println!("{text}");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../captures/interaction-fuzz");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("real-{name}.txt")), &text);
}

#[test]
fn ops_parse_as_a_replay_prints_them() {
    let ops = ops::generate(11);
    let code = ops::replay_code(11, &ops);
    let list = code.split_once("&[").unwrap().1.trim_end_matches(")");
    assert_eq!(ops::parse(list).unwrap(), ops);
}

// The sim alone.

fn sim() -> Sim {
    let template = opencode_kit::model::SessionInfo { id: "ses_fuzzsim0000000000000000".into(), ..Default::default() };
    Sim::new(9, template)
}

#[test]
fn sim_runs_a_prompt_to_the_end() {
    let mut s = sim();
    s.admit("msg_a", "p1", false);
    assert!(s.busy() && s.delivered("msg_a"));
    s.advance(120.0);
    assert!(!s.busy());
    assert!(s.out.iter().any(|e| e.1 == "session.execution.succeeded"));
}

#[test]
fn sim_admits_a_prompt_once() {
    let mut s = sim();
    s.admit("msg_a", "p1", false);
    s.admit("msg_a", "p1", false);
    assert_eq!(s.users().iter().filter(|id| *id == "msg_a").count(), 1);
}

#[test]
fn sim_takes_a_steer_at_the_next_step_and_a_queued_prompt_after_the_run() {
    let mut s = sim();
    s.admit("msg_a", "p1", false);
    s.admit("msg_b", "p2", true);
    s.admit("msg_c", "p3", false);
    assert_eq!(s.waiting(false), vec!["msg_c"]);
    s.advance(400.0);
    let users = s.users();
    let at = |id: &str| users.iter().position(|u| u == id).unwrap();
    assert!(at("msg_a") < at("msg_c") && at("msg_c") < at("msg_b"));
    assert!(!s.busy());
}

#[test]
fn sim_interrupt_ends_the_run_and_keeps_the_inbox() {
    let mut s = sim();
    s.admit("msg_a", "p1", false);
    s.admit("msg_b", "p2", true);
    assert!(s.interrupt());
    assert!(!s.interrupt());
    s.advance(1.0);
    assert!(!s.busy());
    assert_eq!(s.waiting(true), vec!["msg_b"]);
    assert!(s.out.iter().any(|e| e.1 == "session.execution.interrupted" && e.2["reason"] == "user"));
}

#[test]
fn sim_comes_back_after_a_restart() {
    let mut s = sim();
    s.admit("msg_a", "p1", false);
    s.restart(3.0);
    assert!(!s.up && !s.busy());
    s.advance(5.0);
    assert!(s.up);
}

#[test]
fn every_bug_in_the_list_parses() {
    for line in include_str!("bugs.tsv").lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (name, steps) = line.split_once('\t').expect("name<TAB>ops");
        assert!(ops::parse(steps).is_ok(), "{name}: {steps}");
    }
}


