use rune::{Runtime, TextEdit};

use super::fold::add_flow;
use super::list::{automation, is_automation};
use crate::{App, FIXTURE, Route};

const DT: f64 = 1.0 / 60.0;

fn home(flow: usize, running: usize) -> App {
    let mut app = App::empty();
    app.load_fixture(FIXTURE);
    app.route = Route::Home;
    add_flow(&mut app, flow, running);
    app
}

fn run(app: App) -> Runtime<App> {
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    rt
}

fn idle(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / DT) as usize {
        rt.frame(DT);
    }
}

/// Home's session rows top to bottom, with their tops.
fn rows(rt: &Runtime<App>) -> Vec<(String, f32)> {
    let mut v: Vec<(String, f32)> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("r-ses_")))
        .map(|e| (e.key.unwrap(), e.rect[1]))
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v
}

#[test]
fn only_flow_titles_are_automations() {
    assert_eq!(automation("flow: gauntlet R5 r1-blood-b6-islands"), Some("Flow"));
    assert!(!is_automation("Fix flaky reconnect (flow: later)"));
    assert!(!is_automation("Flow chart for onboarding"));
}

#[test]
fn flow_sessions_fold_into_one_row_per_day_and_leave_the_list_to_people() {
    let app = home(80, 3);
    let groups = app.home_groups(None);
    assert!(groups.iter().all(|g| g.rows.iter().all(|s| !is_automation(&s.title))), "no Flow session gets its own row");
    let fold = groups.iter().flat_map(|g| &g.folds).find(|f| f.name == "Flow").expect("a Flow fold");
    assert_eq!((fold.running, fold.total), (3, 80));
    let people = app.sessions.iter().filter(|s| s.parent.is_none() && !is_automation(&s.title)).count();
    assert_eq!(app.home_sessions(None).len(), people, "Flow's 80 don't take the 64 places from the person's sessions");
    let inbox = app.inbox_sections();
    let listed = inbox.needs.iter().chain(&inbox.unread).chain(&inbox.recent).chain(&inbox.working);
    assert!(listed.into_iter().all(|e| !is_automation(&e.session.title)), "nor do they fill the inbox");
}

#[test]
fn search_still_finds_flow_sessions() {
    let mut app = home(10, 0);
    app.search = TextEdit::new("gauntlet");
    assert_eq!(app.home_search_results(None).len(), 10);
}

#[test]
fn the_fold_opens_to_its_sessions_and_stays_open() {
    let mut rt = run(home(12, 2));
    assert!(rows(&rt).iter().all(|(k, _)| !k.starts_with("r-ses_flow")), "shut by default");
    let (x, y) = rt.find("fold-head").expect("the Flow row");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    idle(&mut rt, 0.6);
    assert_eq!(rows(&rt).iter().filter(|(k, _)| k.starts_with("r-ses_flow")).count(), 12);
    assert!(rt.state.restore.inbox.home_automations, "remembered with the inbox's disclosures");
}

#[test]
fn rows_hold_still_under_the_pointer_and_move_when_it_leaves() {
    let mut rt = run(home(20, 0));
    let before = rows(&rt);
    let (first, top) = before[0].clone();
    let last = before.last().unwrap().0.trim_start_matches("r-").to_owned();
    let (x, y) = rt.find(&first).unwrap();
    rt.pointer_move(x, y);
    idle(&mut rt, 0.2);
    rt.update(move |s, _| {
        let now = s.now_ms + 1_000;
        s.sessions.iter_mut().find(|i| i.id == last).unwrap().updated = now;
    });
    idle(&mut rt, 1.0);
    assert_eq!(rows(&rt), before, "nothing moves while the pointer is on the list");

    rt.pointer_leave();
    rt.frame(DT);
    rt.frame(DT);
    let moving = rows(&rt);
    idle(&mut rt, 1.0);
    let after = rows(&rt);
    let at = |list: &[(String, f32)], k: &str| list.iter().find(|(x, _)| x == k).map(|(_, y)| *y);
    let between = moving.iter().filter(|(k, y)| {
        let (Some(from), Some(to)) = (at(&before, k), at(&after, k)) else { return false };
        (from - to).abs() > 1.0 && (y - from).abs() > 0.5 && (y - to).abs() > 0.5
    });
    assert!(between.count() > 0, "rows glide on the spring rather than landing at once:\n{before:?}\n{moving:?}\n{after:?}");
    assert_eq!(after[0].0, before.last().unwrap().0, "then the updated session leads");
    assert!((after[0].1 - top).abs() < 0.5);
}
