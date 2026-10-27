use rune::{Env, Runtime};

use crate::App;
use crate::agent_stories as st;

fn rt(style: usize) -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    app.knobs.set(&crate::knobs::SUBAGENT_ROWS, style as f32);
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt
}

fn play(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs * 120.0) as usize {
        rt.frame(1.0 / 120.0);
    }
}

/// Live keys under `path`, in tree order.
fn keys(rt: &Runtime<App>, path: &str) -> Vec<String> {
    rt.elements().into_iter().filter(|e| !e.exiting && e.path.contains(path)).filter_map(|e| e.key).collect()
}

#[test]
fn each_new_step_is_a_new_line_that_rolls_in_over_the_last() {
    let mut rt = rt(1);
    rt.update(st::spawn);
    play(&mut rt, 0.5);
    let now = |rt: &Runtime<App>| keys(rt, "agent-line/now/").into_iter().find(|k| k.starts_with("step-")).unwrap();
    let before = now(&rt);
    rt.update(st::advance);
    rt.frame(1.0 / 120.0);
    let after = now(&rt);
    assert_ne!(before, after, "the step's words never swap in place");
    let leaving = rt.elements().into_iter().any(|e| e.exiting && e.key.as_deref() == Some(before.as_str()));
    assert!(leaving, "the old step leaves on its own transition");
}

#[test]
fn agents_started_together_share_the_first_ones_row() {
    for style in [2, 3, 4, 6] {
        let mut rt = rt(style);
        rt.update(st::spawn);
        play(&mut rt, 0.3);
        rt.update(st::spawn);
        play(&mut rt, 0.5);
        let first = keys(&rt, "a-call_story_agent_00");
        assert!(first.iter().any(|k| k == "member-call_story_agent_01"), "style {style}: the second joins the first: {first:?}");
        assert!(keys(&rt, "a-call_story_agent_01").is_empty(), "style {style}: the second has no row of its own");
    }
}

#[test]
fn a_summary_opens_to_a_row_per_agent_at_once() {
    let mut rt = rt(5);
    rt.update(st::spawn);
    rt.update(st::spawn);
    play(&mut rt, 0.5);
    assert!(rt.find("member-call_story_agent_00").is_none(), "closed, it's one line");
    rt.frame(1.0 / 120.0);
    let r = rt.elements().into_iter().find(|e| e.path.ends_with("a-call_story_agent_00/#0/agent-summary/summary")).expect("the summary line").rect;
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 120.0);
    let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("member-call_story_agent_01")).expect("open, a row per agent");
    assert!(row.opacity > 0.99, "open at once: {}", row.opacity);
}
