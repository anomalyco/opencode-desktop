use rune::{Env, Runtime};

use crate::App;
use crate::agent_stories as st;

fn rt() -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
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
    let mut rt = rt();
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
fn one_agent_is_a_card_with_one_row() {
    let mut rt = rt();
    rt.update(st::spawn);
    play(&mut rt, 0.5);
    let card = keys(&rt, "a-call_story_agent_00");
    assert!(card.iter().any(|k| k == "agent-card"), "{card:?}");
    assert_eq!(card.iter().filter(|k| k.starts_with("member-")).count(), 1, "{card:?}");
}

#[test]
fn agents_started_together_share_the_first_ones_card() {
    let mut rt = rt();
    rt.update(st::spawn);
    play(&mut rt, 0.3);
    rt.update(st::spawn);
    play(&mut rt, 0.5);
    let first = keys(&rt, "a-call_story_agent_00");
    assert!(first.iter().any(|k| k == "member-call_story_agent_01"), "the second joins the first: {first:?}");
    assert!(keys(&rt, "a-call_story_agent_01").is_empty(), "the second has no row of its own");
}
