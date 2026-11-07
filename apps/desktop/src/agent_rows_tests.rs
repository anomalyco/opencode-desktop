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
fn one_agent_is_a_card_with_one_row() {
    let mut rt = rt();
    rt.update(st::spawn);
    play(&mut rt, 0.5);
    let card = keys(&rt, "a-call_story_agent_00");
    assert!(card.iter().any(|k| k == "agent-line"), "{card:?}");
    assert_eq!(card.iter().filter(|k| k.starts_with("member-")).count(), 1, "{card:?}");
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
