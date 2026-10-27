use rune::{Env, Runtime};
use serde_json::json;

use crate::agent_stories as st;
use crate::agents::{FOLD_MS, HOLD_MS, State, step_of};
use crate::{App, Route};

const MAIN: &str = "ses_fx0001aaaaaaaaaaaaaaaaaa";
const FIRST: &str = "ses_story_agent_00";
const DT: f64 = 1.0 / 120.0;

/// The fixture's session in ⌘⇧K Subagents style `style`, with one background agent started.
fn rt(style: usize) -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    app.knobs.set(&crate::knobs::AGENTS_STYLE, style as f32);
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt.update(st::spawn);
    run(&mut rt, 0.1);
    rt
}

fn run(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        rt.frame(DT);
    }
}

fn texts(rt: &Runtime<App>, path: &str) -> Vec<String> {
    rt.elements().into_iter().filter(|e| !e.exiting && e.path.contains(path)).filter_map(|e| e.text).collect()
}

fn press(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
}

#[test]
fn a_tool_call_reads_as_one_short_step() {
    assert_eq!(step_of("read", &json!({ "filePath": "/repo/src/net/backoff.ts" })), "Read backoff.ts");
    assert_eq!(step_of("grep", &json!({ "pattern": "reconnect" })), "Grep reconnect");
    assert_eq!(step_of("shell", &json!({ "command": "bun test src/net\nmore" })), "Shell bun test src/net");
    assert_eq!(step_of("todowrite", &json!({})), "todowrite");
}

#[test]
fn a_stack_row_follows_its_agent_then_holds_its_mark_and_folds() {
    let mut rt = rt(1);
    let row = format!("agent-{FIRST}");
    assert!(texts(&rt, &row).iter().any(|t| t == "Working"));
    rt.update(|s, tx| crate::stories::emit(s, "session.tool.called", json!({ "sessionID": FIRST, "name": "read", "input": { "filePath": "src/net/backoff.ts" } }), tx));
    run(&mut rt, 1.2);
    assert!(texts(&rt, &row).iter().any(|t| t == "Read backoff.ts"), "{:?}", texts(&rt, &row));
    assert!(texts(&rt, &row).iter().any(|t| t == "1s"), "it counts its run time");

    rt.update(st::finish_oldest);
    run(&mut rt, 0.1);
    let held = rt.state.agents_of(MAIN);
    assert_eq!((held[0].state, held[0].folded), (State::Done, false));
    assert!(texts(&rt, &row).iter().any(|t| t == "Done"));

    run(&mut rt, HOLD_MS as f64 / 1000.0);
    assert!(rt.state.agents_of(MAIN)[0].folded, "it folds once it has held its ✓");
    run(&mut rt, FOLD_MS as f64 / 1000.0);
    assert!(rt.state.agents_of(MAIN).is_empty());
    run(&mut rt, 0.5);
    assert!(rt.find(&format!("{row}/line")).is_none(), "the folded row is gone");
}

#[test]
fn a_stack_row_opens_its_agent() {
    let mut rt = rt(1);
    press(&mut rt, &format!("agent-{FIRST}/line"));
    assert_eq!(rt.state.route, Route::Session(FIRST.into()));
}

#[test]
fn the_live_capsule_opens_its_agent() {
    let mut rt = rt(3);
    assert!(texts(&rt, "agent-lead").iter().any(|t| t == "Working"));
    press(&mut rt, "agent-lead");
    assert_eq!(rt.state.route, Route::Session(FIRST.into()));
}

#[test]
fn an_inline_card_scrolled_away_has_a_chip_back_to_it() {
    let mut rt = rt(2);
    run(&mut rt, 0.5);
    let row = "a-call_story_agent_00";
    assert!(texts(&rt, row).iter().any(|t| t == "Working"), "the card itself is live");
    assert!(!rt.state.agents.off(row));
    for _ in 0..20 {
        rt.wheel(640.0, 300.0, 400.0);
        run(&mut rt, 0.1);
    }
    run(&mut rt, 0.6);
    assert!(rt.state.agents.off(row), "scrolled away");
    press(&mut rt, "agent-jump");
    run(&mut rt, 1.5);
    assert!(!rt.state.agents.off(row), "the chip scrolled back to it");
}
