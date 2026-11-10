use rune::Shell;

use super::tear::hit;
use crate::App;
use crate::tabs::Tab;

const DT: f64 = 1.0 / 60.0;

/// A window on the fixture with the first `n` root sessions as its tabs, the first open.
fn window(n: usize) -> rune::Runtime<App> {
    let mut s = App::fixture_session();
    s.tabs = roots(&s).into_iter().take(n).collect();
    let mut rt = s.into_runtime();
    if let Some(first) = rt.state.tabs.first().map(|t| t.id.clone()) {
        rt.update(move |s, tx| {
        s.select_tab(&first, tx);
    });
    }
    for _ in 0..30 {
        rt.frame(DT);
    }
    rt
}

fn roots(s: &App) -> Vec<Tab> {
    s.sessions.iter().filter(|x| x.parent.is_none()).map(|x| Tab::session(x.id.clone(), x.title.clone(), x.directory.clone())).collect()
}

fn ids(rt: &rune::Runtime<App>) -> Vec<String> {
    rt.state.tabs.iter().map(|t| t.id.clone()).collect()
}

fn tab_rect(rt: &rune::Runtime<App>, id: &str) -> [f32; 4] {
    let key = format!("t-{id}");
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key.as_str())).unwrap_or_else(|| panic!("tab {id}")).rect
}

/// A point on the strip just inside the leading edge of tab `index`, where a tab let go lands
/// before it.
fn before_tab(rt: &rune::Runtime<App>, index: usize) -> [f32; 2] {
    let r = tab_rect(rt, &rt.state.tabs[index].id);
    let vertical = rt.state.tab_layout().has_sidebar();
    if vertical { [r[0] + r[2] / 2.0, r[1] + 4.0] } else { [r[0] + 4.0, r[1] + r[3] / 2.0] }
}

#[test]
fn a_point_is_on_the_strip_in_the_window_or_outside_it() {
    let strip = Some([80.0, 0.0, 600.0, 40.0]);
    assert_eq!(hit([100.0, 20.0], strip, [1280.0, 800.0]), (true, true));
    // The strip's slack reaches a little past the window's edge.
    assert_eq!(hit([100.0, -8.0], strip, [1280.0, 800.0]), (true, true));
    assert_eq!(hit([400.0, 400.0], strip, [1280.0, 800.0]), (false, true), "the body");
    assert_eq!(hit([400.0, 900.0], strip, [1280.0, 800.0]), (false, false));
    assert_eq!(hit([400.0, 400.0], None, [1280.0, 800.0]), (false, true), "no strip (the Inbox)");
}

#[test]
fn a_tab_over_another_windows_strip_opens_a_gap_and_lands_in_it() {
    let mut target = window(3);
    let incoming = roots(&target.state)[3].clone();
    let before = ids(&target);
    let [first, second, third] = [0, 1, 2].map(|i| tab_rect(&target, &before[i]));
    let at = before_tab(&target, 1);
    target.update(move |s, tx| {
        s.tab_incoming(Some(at), tx);
    });
    for _ in 0..40 {
        target.frame(DT);
    }
    assert_eq!(target.state.tab_drag.incoming_slot(), Some(1), "the gap opens before the second tab");
    let vertical = target.state.tab_layout().has_sidebar();
    let along = |r: [f32; 4]| if vertical { r[1] } else { r[0] };
    assert!(along(tab_rect(&target, &before[1])) > along(second) + 10.0, "the second tab stepped aside");
    assert!(along(tab_rect(&target, &before[2])) > along(third) + 10.0, "and the third");
    assert!((along(tab_rect(&target, &before[0])) - along(first)).abs() < 0.5, "the first stays");

    let id = incoming.id.clone();
    target.update(move |s, tx| {
        s.receive_tab(incoming, Some("half a thought".into()), Some(at), tx);
    });
    target.frame(DT);
    assert_eq!(ids(&target), vec![before[0].clone(), id.clone(), before[1].clone(), before[2].clone()]);
    assert_eq!(target.state.current_tab().as_deref(), Some(id.as_str()), "it's in front");
    assert_eq!(target.state.tab_drag.incoming_slot(), None, "the gap is filled");
    assert_eq!(target.state.draft_text(&id).as_deref(), Some("half a thought"), "its draft came with it");
}

#[test]
fn a_tab_let_go_over_another_windows_body_joins_the_end_of_its_strip() {
    let mut target = window(2);
    let incoming = roots(&target.state)[2].clone();
    let id = incoming.id.clone();
    target.update(move |s, tx| {
        s.receive_tab(incoming, None, None, tx);
    });
    target.frame(DT);
    assert_eq!(ids(&target).last(), Some(&id));
    assert_eq!(target.state.current_tab().as_deref(), Some(id.as_str()));
}

#[test]
fn a_tab_that_leaves_goes_from_its_window_and_the_window_closes_when_it_was_the_last() {
    let mut source = window(2);
    let [a, b] = [0, 1].map(|i| source.state.tabs[i].id.clone());
    let gone = a.clone();
    source.update(move |s, tx| {
        s.tab_left(&gone, tx);
    });
    source.frame(DT);
    assert_eq!(ids(&source), vec![b.clone()], "the tab left, no closed-tab entry: it lives on elsewhere");
    assert_eq!(source.state.current_tab().as_deref(), Some(b.as_str()), "its neighbour shows");
    assert_eq!(source.shell_request, None, "a window with tabs stays");

    let last = b.clone();
    source.update(move |s, tx| {
        s.tab_left(&last, tx);
    });
    source.frame(DT);
    assert!(source.state.tabs.is_empty());
    assert_eq!(source.shell_request, Some(Shell::Close), "the empty window closes");
}

#[test]
fn moving_a_tab_there_and_back_puts_it_where_it_was() {
    let mut one = window(2);
    let mut two = window(0);
    let tab = one.state.tabs[1].clone();
    let id = tab.id.clone();
    // Over to the empty window...
    let (gone, moved) = (id.clone(), tab.clone());
    one.update(move |s, tx| {
        s.tab_left(&gone, tx);
    });
    two.update(move |s, tx| {
        s.receive_tab(moved, None, None, tx);
    });
    two.frame(DT);
    assert_eq!(ids(&two), vec![id.clone()]);
    // ...and back, to the start of the first window's strip.
    for _ in 0..10 {
        one.frame(DT);
    }
    let at = before_tab(&one, 0);
    let back = id.clone();
    two.update(move |s, tx| {
        s.tab_left(&back, tx);
    });
    one.update(move |s, tx| {
        s.receive_tab(tab, None, Some(at), tx);
    });
    one.frame(DT);
    two.frame(DT);
    assert_eq!(ids(&one)[0], id);
    assert_eq!(two.shell_request, Some(Shell::Close), "the window it left empty closes");
}
