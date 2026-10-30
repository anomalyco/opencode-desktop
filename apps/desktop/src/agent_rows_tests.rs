use rune::Runtime;

use crate::{App, Route};
use crate::agent_stories as st;
use crate::fixture_runtime as rt;

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

/// Each run time on screen, by key.
fn times(rt: &Runtime<App>) -> Vec<(String, String)> {
    rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| Some((e.key.filter(|k| k.starts_with("time-call_story_agent_"))?, e.text?))).collect()
}

/// Run times tick on the app clock's whole seconds, exactly a second apart, and in step however
/// far apart their agents started.
#[test]
fn run_times_tick_together_a_second_apart_on_the_clocks_whole_seconds() {
    let mut rt = rt();
    rt.update(st::spawn);
    play(&mut rt, 0.37);
    rt.update(st::spawn);
    play(&mut rt, 0.5);
    let mut last = times(&rt);
    assert_eq!(last.len(), 2, "two rows show a time: {last:?}");
    let mut ticks: Vec<i64> = Vec::new();
    for _ in 0..(4.5 * 120.0) as usize {
        rt.frame(1.0 / 120.0);
        let now = times(&rt);
        let changed = now.iter().zip(&last).filter(|(a, b)| a != b).count();
        assert!(changed == 0 || changed == now.len(), "every row ticks in the same frame: {last:?} → {now:?}");
        if changed > 0 {
            ticks.push(rt.state.now_ms);
        }
        last = now;
    }
    assert!(ticks.len() >= 4, "{ticks:?}");
    let frame = 9;
    for t in &ticks {
        assert!(t.rem_euclid(1000) < frame, "a tick lands on a whole second of the app clock: {ticks:?}");
    }
    for w in ticks.windows(2) {
        assert!((w[1] - w[0] - 1000).abs() < frame, "ticks land 1.0 s apart: {ticks:?}");
    }
}

/// Inside a running subagent's session its bar ticks on its own, with nothing else changing.
#[test]
fn the_subagent_bar_ticks_once_a_second_on_its_own() {
    let mut rt = rt();
    rt.update(st::spawn);
    play(&mut rt, 0.3);
    let child = rt.state.sessions.iter().rev().find(|x| x.parent.is_some()).unwrap().id.clone();
    rt.update(|s, tx| s.open(&child, tx));
    play(&mut rt, 0.5);
    let label = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(&*format!("bar-time-{child}"))).and_then(|e| e.text);
    let mut last = label(&rt);
    assert!(last.is_some(), "the bar shows its time");
    let mut ticks: Vec<i64> = Vec::new();
    for _ in 0..(3.5 * 120.0) as usize {
        rt.frame(1.0 / 120.0);
        let now = label(&rt);
        if now != last {
            ticks.push(rt.state.now_ms);
        }
        last = now;
    }
    assert!(ticks.len() >= 3, "{ticks:?}");
    for w in ticks.windows(2) {
        assert!((w[1] - w[0] - 1000).abs() < 9, "ticks land 1.0 s apart: {ticks:?}");
    }
}

/// The poll asks for a redraw once a second while a running time shows, and never on a page
/// whose only run is its own top-level turn.
#[test]
fn the_poll_redraws_once_a_second_while_a_run_time_shows() {
    let mut rt = rt();
    rt.update(st::spawn);
    play(&mut rt, 0.3);
    let s = &mut rt.state;
    let start = s.now_ms;
    let redraws = (1..=3000).filter(|ms| {
        s.now_ms = start + ms;
        crate::agent_rows::poll(s, &mut rune::Tx::default())
    });
    assert_eq!(redraws.count(), 3);
    let Route::Session(id) = s.route.clone() else { panic!("no open session") };
    s.running.retain(|r| *r == id);
    s.running.insert(id);
    assert!((1..=3000).all(|ms| {
        s.now_ms = start + 3000 + ms;
        !crate::agent_rows::poll(s, &mut rune::Tx::default())
    }));
}
