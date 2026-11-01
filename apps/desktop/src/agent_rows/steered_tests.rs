use opencode_kit::model::ToolStatus;
use rune::{Runtime, Tx};
use serde_json::json;

use super::steered_stories::{self as st, CHILD};
use super::{Run, State};
use crate::App;
use crate::fixture_runtime as rt;
use crate::stories::{emit, sid};

fn play(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs * 120.0).round() as usize {
        rt.frame(1.0 / 120.0);
    }
}

/// The agent reads `file`.
fn read(rt: &mut Runtime<App>, file: &'static str) {
    rt.update(move |s, tx| emit(s, "session.tool.called", json!({ "sessionID": CHILD, "name": "read", "input": { "filePath": file } }), tx));
}

/// Call `k`'s run.
fn run(rt: &Runtime<App>, k: usize) -> Run {
    let s = &rt.state;
    s.agent_run(&sid(s), &format!("call_story_steer_{k}"), Some(CHILD), &ToolStatus::Completed)
}

fn about(ms: Option<i64>, want: i64) -> bool {
    ms.is_some_and(|ms| (ms - want).abs() < 50)
}

/// Starts the agent, then steers it twice, a second and a half apart, a read in each call.
fn steered_twice() -> Runtime<App> {
    let mut rt = rt();
    rt.update(|s, tx| st::call(s, tx));
    play(&mut rt, 0.5);
    read(&mut rt, "palette.rs");
    play(&mut rt, 1.0);
    rt.update(|s, tx| st::call(s, tx));
    play(&mut rt, 0.5);
    read(&mut rt, "theme.rs");
    play(&mut rt, 1.0);
    rt.update(|s, tx| st::call(s, tx));
    play(&mut rt, 0.5);
    read(&mut rt, "styles.md");
    play(&mut rt, 0.5);
    rt
}

#[test]
fn each_call_to_a_steered_agent_shows_only_its_own_share() {
    let mut rt = steered_twice();
    let (first, second, newest) = (run(&rt, 0), run(&rt, 1), run(&rt, 2));
    assert_eq!(first.state, State::Steered);
    assert_eq!(first.steps, ["Read palette.rs"], "{first:?}");
    assert!(about(first.ms, 1500), "stops at the steer: {first:?}");
    assert_eq!((first.line(), first.result), ("Steered".to_owned(), None));
    assert_eq!(second.state, State::Steered);
    assert_eq!(second.steps, ["Thinking", "Read theme.rs"], "{second:?}");
    assert!(about(second.ms, 1500), "{second:?}");
    assert_eq!(newest.state, State::Running);
    assert_eq!(newest.steps, ["Thinking", "Read styles.md"], "{newest:?}");
    assert!(newest.ms.is_some_and(|ms| ms <= 1000), "its clock starts at its own call: {newest:?}");

    rt.update(|s, tx| st::finish(s, tx));
    play(&mut rt, 0.5);
    let (first, second, newest) = (run(&rt, 0), run(&rt, 1), run(&rt, 2));
    assert_eq!((first.state, second.state), (State::Steered, State::Steered), "steered calls stay steered");
    assert!(about(first.ms, 1500) && about(second.ms, 1500), "and their clocks stay stopped: {first:?} {second:?}");
    assert_eq!(newest.state, State::Done);
    assert!(about(newest.ms, 1000), "{newest:?}");
    assert!(newest.result.as_deref().is_some_and(|r| r.starts_with("Seer keeps")), "{newest:?}");
    assert_eq!((first.result, second.result), (None, None), "only the newest answered");
}

#[test]
fn only_the_newest_card_spins_and_the_capsule_counts_the_agent_once() {
    let rt = steered_twice();
    let live = |key: &str| rt.elements().into_iter().filter(|e| !e.exiting && e.path.contains("agent-line/mark") && e.key.as_deref() == Some(key)).count();
    assert_eq!((live("spin"), live("steered")), (1, 2));
    let parent = sid(&rt.state);
    assert_eq!(rt.state.running_subagents(&parent).len(), 1, "one agent, however many calls");
    let words: Vec<String> = rt.elements().into_iter().filter(|e| !e.exiting && e.key.as_deref() == Some("steered")).filter_map(|e| e.text).collect();
    assert_eq!(words, ["Steered", "Steered"]);
}

#[test]
fn a_call_that_wakes_a_finished_agent_leaves_the_last_one_done() {
    let mut rt = rt();
    rt.update(|s, tx| st::call(s, tx));
    play(&mut rt, 0.5);
    read(&mut rt, "palette.rs");
    play(&mut rt, 0.5);
    rt.update(|s, tx| st::finish(s, tx));
    play(&mut rt, 1.0);
    rt.update(|s, tx: &mut Tx| {
        st::call(s, tx);
        emit(s, "session.execution.started", json!({ "sessionID": CHILD }), tx);
    });
    play(&mut rt, 0.5);
    read(&mut rt, "theme.rs");
    play(&mut rt, 0.5);
    let (first, second) = (run(&rt, 0), run(&rt, 1));
    assert_eq!(first.state, State::Done, "{first:?}");
    assert!(about(first.ms, 1000), "{first:?}");
    assert!(first.result.as_deref().is_some_and(|r| r.starts_with("Seer keeps")), "{first:?}");
    assert_eq!(second.state, State::Running);
    assert_eq!(second.steps, ["Thinking", "Read theme.rs"], "{second:?}");
}

#[test]
fn an_agent_called_once_is_unchanged() {
    let mut rt = rt();
    rt.update(|s, tx| st::call(s, tx));
    play(&mut rt, 0.5);
    let s = &rt.state;
    assert_eq!(s.window(&sid(s), "call_story_steer_0", CHILD), None, "no window of its own");
    for f in ["a.rs", "b.rs", "c.rs", "d.rs"] {
        read(&mut rt, f);
        play(&mut rt, 0.25);
    }
    let r = run(&rt, 0);
    assert_eq!((r.state, r.steps.clone(), r.taken), (State::Running, vec!["Read b.rs".to_owned(), "Read c.rs".to_owned(), "Read d.rs".to_owned()], 1));
    assert!(about(r.ms.map(|ms| ms + rt.state.now_ms - rt.state.run_clock()), 1500), "from when it was made: {r:?}");
    rt.update(|s, tx| st::finish(s, tx));
    play(&mut rt, 0.5);
    let r = run(&rt, 0);
    assert_eq!((r.state, r.line()), (State::Done, "Seer keeps a 4pt space scale and two tones per surface; notes in styles.md".to_owned()));
    // Sent more work from its own page: its row shows only the new run's steps.
    rt.update(|s, tx| emit(s, "session.execution.started", json!({ "sessionID": CHILD }), tx));
    play(&mut rt, 0.25);
    read(&mut rt, "e.rs");
    play(&mut rt, 0.25);
    let r = run(&rt, 0);
    assert_eq!((r.state, r.steps, r.taken), (State::Running, vec!["Read e.rs".to_owned()], 0));
}
