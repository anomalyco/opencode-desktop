use std::cell::RefCell;
use std::rc::Rc;

use opencode_kit::model::SessionInfo;
use rune::{Env, Runtime};
use serde_json::json;

use super::*;
use crate::tabs::Tab;

#[derive(Default)]
struct Log {
    focused: bool,
    posted: Vec<(String, &'static str, String, String)>,
    removed: Vec<String>,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<Log>>);

impl Sink for Fake {
    fn focused(&self) -> bool {
        self.0.borrow().focused
    }

    fn post(&mut self, note: &Note, _server: &str) {
        self.0.borrow_mut().posted.push((note.id.clone(), note.kind.title(), note.body.clone(), note.session.clone()));
    }

    fn remove(&mut self, id: &str) {
        self.0.borrow_mut().removed.push(id.to_owned());
    }
}

fn session(id: &str, title: &str, parent: Option<&str>) -> SessionInfo {
    SessionInfo { id: id.into(), title: title.into(), parent: parent.map(Into::into), ..Default::default() }
}

/// A root with a tab ("root"), its subagent ("child"), and a root without a tab ("closed").
fn app() -> (App, Fake) {
    let mut app = App::empty();
    app.sessions = vec![session("root", "Fix the build", None), session("child", "Explore", Some("root")), session("closed", "Old work", None)];
    app.tabs = vec![Tab::session("root", "Fix the build", String::new())];
    let fake = Fake::default();
    app.notify = Notifier::with_sink(fake.clone());
    (app, fake)
}

fn posted(fake: &Fake) -> Vec<(&'static str, String, String)> {
    fake.0.borrow().posted.iter().map(|(_, title, body, session)| (*title, body.clone(), session.clone())).collect()
}

fn ended(app: &mut App, ty: &str, sid: &str) {
    app.apply_event(ty, &json!({ "sessionID": sid }));
}

fn asked(app: &mut App, sid: &str, id: &str) {
    app.apply_event("permission.asked", &json!({ "id": id, "sessionID": sid, "action": "bash" }));
}

#[test]
fn desktops_defaults_and_saved_shape() {
    let d = Kinds::default();
    assert!(!d.agent && d.permissions && !d.errors);
    assert_eq!(Kinds::from_json(&json!(null)), d);
    let on = Kinds { agent: true, permissions: false, errors: true, questions: true };
    assert_eq!(Kinds::from_json(&on.to_json()), on);
    assert_eq!(Kinds::from_json(&json!({ "agent": true })), Kinds { agent: true, ..d });
}

#[test]
fn a_finished_run_notifies_only_with_the_agent_setting_on() {
    let (mut app, fake) = app();
    ended(&mut app, "session.execution.succeeded", "root");
    assert!(posted(&fake).is_empty(), "off by default, as in Desktop");
    app.settings.notify.agent = true;
    ended(&mut app, "session.execution.succeeded", "root");
    assert_eq!(posted(&fake), [("Response ready", "Fix the build".to_owned(), "root".to_owned())]);
}

#[test]
fn nothing_posts_while_the_window_is_focused() {
    let (mut app, fake) = app();
    app.settings.notify = Kinds { agent: true, permissions: true, errors: true, questions: true };
    fake.0.borrow_mut().focused = true;
    ended(&mut app, "session.execution.succeeded", "root");
    ended(&mut app, "session.execution.failed", "root");
    asked(&mut app, "root", "per_1");
    assert!(posted(&fake).is_empty());
}

#[test]
fn only_a_root_session_with_an_open_tab_notifies() {
    let (mut app, fake) = app();
    app.settings.notify = Kinds { agent: true, permissions: false, errors: true, questions: true };
    ended(&mut app, "session.execution.succeeded", "closed");
    ended(&mut app, "session.execution.succeeded", "child");
    ended(&mut app, "session.execution.failed", "child");
    ended(&mut app, "session.execution.succeeded", "unknown");
    assert!(posted(&fake).is_empty());
}

#[test]
fn a_run_with_a_subagent_still_running_isnt_done() {
    let (mut app, fake) = app();
    app.settings.notify.agent = true;
    app.running.insert("child".into());
    ended(&mut app, "session.execution.succeeded", "root");
    assert!(posted(&fake).is_empty());
    app.running.clear();
    ended(&mut app, "session.execution.succeeded", "root");
    assert_eq!(posted(&fake).len(), 1);
}

#[test]
fn errors_notify_with_desktops_title_and_fallback() {
    let (mut app, fake) = app();
    ended(&mut app, "session.execution.failed", "root");
    assert!(posted(&fake).is_empty(), "off by default");
    app.settings.notify.errors = true;
    ended(&mut app, "session.execution.failed", "root");
    app.sessions[0].title.clear();
    ended(&mut app, "session.execution.failed", "root");
    assert_eq!(
        posted(&fake),
        [("Session error", "Fix the build".to_owned(), "root".to_owned()), ("Session error", "An error occurred".to_owned(), "root".to_owned())]
    );
}

#[test]
fn a_permission_notifies_once_for_its_root() {
    let (mut app, fake) = app();
    asked(&mut app, "child", "per_1");
    asked(&mut app, "child", "per_1");
    asked(&mut app, "closed", "per_2");
    assert_eq!(posted(&fake), [("Permission required", "Fix the build".to_owned(), "root".to_owned())]);
    app.settings.notify.permissions = false;
    asked(&mut app, "root", "per_3");
    assert_eq!(posted(&fake).len(), 1);
}

#[test]
fn interrupted_runs_and_other_events_post_nothing() {
    let (mut app, fake) = app();
    app.settings.notify = Kinds { agent: true, permissions: true, errors: true, questions: true };
    ended(&mut app, "session.execution.interrupted", "root");
    ended(&mut app, "session.execution.started", "root");
    app.apply_event("form.created", &json!({ "sessionID": "root", "form": { "id": "frm_1" } }));
    assert!(posted(&fake).is_empty());
}

#[test]
fn at_most_forty_stay_and_the_oldest_goes_first() {
    let (mut app, fake) = app();
    app.settings.notify.agent = true;
    for _ in 0..CAP + 2 {
        ended(&mut app, "session.execution.succeeded", "root");
    }
    let log = fake.0.borrow();
    assert_eq!(log.posted.len(), CAP + 2);
    assert_eq!(log.removed, [log.posted[0].0.clone(), log.posted[1].0.clone()]);
    assert_eq!(app.notify.shown.len(), CAP);
}

#[test]
fn without_a_sink_nothing_is_decided() {
    let (mut app, _) = app();
    app.notify = Notifier::default();
    app.settings.notify.agent = true;
    ended(&mut app, "session.execution.succeeded", "root");
    assert!(app.notify.shown.is_empty());
}

#[test]
fn a_click_opens_its_session_and_closes_settings() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    let target = app.sessions.iter().find(|s| s.parent.is_none()).unwrap().id.clone();
    app.settings.open = true;
    let clicks = app.notify.clicks.clone();
    Notifier::click(&clicks, target.clone(), String::new());
    Notifier::click(&clicks, "ses_elsewhere".into(), "http://other.example:4096".into());
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.update_if(|s, tx| poll(s, tx)));
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.settings.open);
    assert!(matches!(&rt.state.route, crate::Route::Session(id) if *id == target), "{:?}", rt.state.route);
    assert!(rt.state.tabs.iter().any(|t| t.id == target));
    assert!(!rt.update_if(|s, tx| poll(s, tx)), "each click opens once");
}

#[test]
fn settings_lists_the_three_switches() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    app.settings.open = true;
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 1400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for key in ["notify-agent", "notify-permissions", "notify-errors"] {
        assert!(rt.find(key).is_some(), "{key}");
    }
    let (x, y) = rt.find("notify-agent").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.settings.notify.agent);
}
