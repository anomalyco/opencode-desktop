use opencode_kit::model::Message;
use rune::{Env, Key, KeyEvent, Mods, Runtime};

use super::*;

const DT: f64 = 1.0 / 30.0;

fn runtime() -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    start(&mut app);
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt
}

fn play(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        rt.frame(DT);
    }
}

fn keys(rt: &mut Runtime<App>, c: char) {
    rt.key(KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, alt: true, ..Default::default() } });
}

fn sid(rt: &Runtime<App>) -> String {
    match &rt.state.route {
        Route::Session { id, .. } => id.clone(),
        _ => panic!("no open session"),
    }
}

fn messages(rt: &Runtime<App>) -> Vec<Message> {
    rt.state.transcripts.get(&sid(rt)).map(|t| t.messages.clone()).unwrap_or_default()
}

fn end(ids: &Ids) -> f64 {
    demo_script::events(ids).last().unwrap().0
}

#[test]
fn the_story_runs_a_minute_or_so_and_is_the_same_every_time() {
    let ids = Ids::new(1);
    let a = demo_script::events(&ids);
    assert_eq!(a, demo_script::events(&ids));
    assert!(a.windows(2).all(|w| w[0].0 <= w[1].0), "ordered by time");
    let secs = end(&ids);
    assert!((60.0..=90.0).contains(&secs), "{secs:.1} s");
}

#[test]
fn the_turn_plays_to_its_end_without_writing_anywhere() {
    let mut rt = runtime();
    let secs = end(&Ids::new(1));
    play(&mut rt, secs + 2.0);
    let s = &rt.state;
    let id = sid(&rt);
    assert!(s.playback.is_empty());
    assert!(!s.running.iter().any(|r| r.starts_with("ses_demo")), "nothing still runs: {:?}", s.running);
    assert!(!s.question_shown(), "the question was answered");
    assert!(s.inboxes.get(&id).is_none_or(|i| i.items.is_empty()), "the steer was delivered");
    assert!(s.composer.text.is_empty());
    assert!(!s.status.contains("Not connected") && !s.status.contains("Read-only"), "{}", s.status);
    assert_eq!(s.session(&id).map(|i| i.title.as_str()), Some(demo_script::TITLE));
    let msgs = messages(&rt);
    let users: Vec<&str> = msgs.iter().filter_map(|m| if let Message::User { text, .. } = m { Some(text.as_str()) } else { None }).collect();
    assert_eq!(users, [demo_script::PROMPT, demo_script::STEER]);
    let answers: Vec<_> = msgs.iter().filter_map(|m| if let Message::Assistant(a) = m { Some(a) } else { None }).collect();
    assert_eq!(answers.len(), 5);
    assert!(answers.iter().all(|a| a.completed.is_some()));
    assert!((answers.iter().map(|a| a.cost).sum::<f64>() - 0.45).abs() < 1e-9);
    let children: Vec<_> = s.sessions.iter().filter(|x| x.parent.as_deref() == Some(id.as_str())).collect();
    assert_eq!(children.len(), 2);
}

#[test]
fn a_pause_holds_the_story_and_its_clock() {
    let mut rt = runtime();
    let ids = Ids::new(1);
    let child = ids.child(0);
    let agents = demo_script::events(&ids).into_iter().find(|e| e.1 == "session.execution.started" && e.2["sessionID"] == child.as_str()).unwrap().0;
    let from = agents + 3.0;
    play(&mut rt, from);
    keys(&mut rt, 'p');
    play(&mut rt, 0.1);
    let (held, at) = (format!("{:?}", messages(&rt)), rt.state.now_ms);
    assert!(rt.state.running.contains(&child), "subagents run at the pause");
    play(&mut rt, 10.0);
    assert_eq!(format!("{:?}", messages(&rt)), held, "nothing arrives while paused");
    assert_eq!(rt.state.now_ms, at, "running times stand still");
    keys(&mut rt, 'p');
    play(&mut rt, end(&ids) - from + 2.0);
    assert!(rt.state.playback.is_empty());
    assert!(!rt.state.running.iter().any(|r| r.starts_with("ses_demo")));
}

#[test]
fn replay_starts_over_in_a_fresh_session() {
    let mut rt = runtime();
    play(&mut rt, 30.0);
    let first = sid(&rt);
    keys(&mut rt, 'r');
    play(&mut rt, 0.1);
    let second = sid(&rt);
    assert_ne!(first, second);
    assert!(rt.state.session(&first).is_none(), "the first play's session is gone");
    assert!(messages(&rt).is_empty(), "the prompt is still being typed");
    play(&mut rt, end(&Ids::new(2)) + 2.0);
    assert_eq!(messages(&rt).len(), 7);
}

#[test]
fn the_keys_do_nothing_outside_the_demo() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    let before = sid(&rt);
    keys(&mut rt, 'r');
    assert_eq!(sid(&rt), before);
    assert!(rt.state.playback.is_empty());
}
