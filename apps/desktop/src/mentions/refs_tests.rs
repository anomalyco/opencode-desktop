use opencode_kit::client::Request;
use rune::Runtime;
use serde_json::json;

use super::super::chip_stories::{GONE, scene};
use crate::fixture_ids::{CHILD, MAIN};
use crate::{App, Route, Source};

const DT: f64 = 1.0 / 60.0;

fn reply() -> Runtime<App> {
    let mut app = App::fixture_session();
    scene(&mut app);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(DT);
    }
    rt
}

/// The paragraph naming the subagent, as drawn.
fn paragraph(rt: &Runtime<App>) -> rune::ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.text.as_deref().is_some_and(|t| t.contains("came back clean"))).expect("the reply's paragraph")
}

#[test]
fn a_known_id_reads_as_its_title_and_the_rest_stay_as_written() {
    let rt = reply();
    let text = paragraph(&rt).text.unwrap();
    assert!(text.contains("ran in Review the reconnect change and"), "{text}");
    assert!(text.contains("notes are in Review the reconnect change."), "{text}");
    assert!(!text.contains(CHILD), "{text}");
    assert!(text.contains(GONE), "an id no session has stays text: {text}");
    let code = rt.elements().into_iter().any(|e| e.text.as_deref().is_some_and(|t| t.contains(&format!("opencode export {CHILD}"))));
    assert!(code, "the code block keeps the id");
}

#[test]
fn pressing_the_chip_opens_the_subagent() {
    let mut rt = reply();
    assert_eq!(rt.state.route, Route::here(MAIN.into()));
    let e = paragraph(&rt);
    let y = e.rect[1] + 11.0;
    let mut x = e.rect[0] + 2.0;
    while x < e.rect[0] + e.rect[2] && rt.state.route == Route::here(MAIN.into()) {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(DT);
        x += 6.0;
    }
    assert_eq!(rt.state.route, Route::here(CHILD.into()));
}

#[test]
fn an_unknown_id_is_looked_up_once_and_a_miss_stays_text() {
    let mut app = App::fixture_session();
    let (tx, requests) = std::sync::mpsc::channel();
    let _writes = app.fake_writes();
    if let Source::Live { requests: r, .. } = &mut app.source {
        *r = Some(tx);
    }
    assert!(app.session_ref_of(GONE).is_none());
    assert!(app.session_ref_of(GONE).is_none());
    let asked: Vec<_> = requests.try_iter().filter(|r| matches!(r, Request::Lookup(id) if id == GONE)).collect();
    assert_eq!(asked.len(), 1, "one lookup, however often the row builds");
    assert!(app.session_refs.asked(GONE));
    app.apply_event("session.lookup.missing", &json!({ "sessionID": GONE }));
    assert!(app.session_refs.missing.contains(GONE));
    assert!(app.session_ref_of(GONE).is_none());
    assert_eq!(requests.try_iter().count(), 0, "a miss isn't asked again");
}

#[test]
fn a_subagents_ref_names_its_parent() {
    let app = App::fixture_session();
    let r = app.session_ref_of(CHILD).expect("the fixture lists it");
    assert_eq!(r.title, "Review the reconnect change");
    assert_eq!(r.parent.as_deref(), Some("Fix flaky reconnect in the event stream"));
}
