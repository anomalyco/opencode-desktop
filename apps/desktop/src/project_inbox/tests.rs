use opencode_kit::client::Write;
use rune::Tx;

use super::{Kind, Said, TaskInfo};
use super::stories::seed;
use crate::fixture_ids::{MAIN, SIXTH};
use crate::session_menu::{self, Spot, Target};
use crate::{App, Route};

fn world() -> App {
    super::force(true);
    let mut s = App::fixture_session();
    seed(&mut s);
    s
}

fn ids(s: &App, kind: Kind) -> Vec<String> {
    s.inbox_rows().iter().filter(|r| r.kind == kind).map(|r| r.info.id.clone()).collect()
}

fn done(s: &App, id: &str) -> bool {
    ids(s, Kind::Done).iter().any(|r| r == id)
}

#[test]
fn tasks_waiting_on_you_and_done_ones_fill_the_inbox() {
    let s = world();
    assert_eq!(ids(&s, Kind::Needs), ["ses_ib01", "ses_ib05", "ses_ib13"], "a helper's question shows on its task, and a blocked task");
    assert_eq!(ids(&s, Kind::Done), ["ses_ib11", "ses_ib02", "ses_ib12", "ses_ib09", "ses_ib06"]);
    assert_eq!(s.lead_of("ses_ib03"), Some("ses_ib01"), "a task's subagent is its helper");
    assert_eq!(s.project_of_task("ses_ib03"), Some(MAIN));
    assert_eq!(s.project_of_task(MAIN), None, "a project is never its own task");
    assert_eq!((s.project_working(MAIN), s.project_working(SIXTH)), (3, 2), "working tasks only count, and one waiting on its background work");
}

#[test]
fn a_task_with_its_own_session_says_where_it_stands() {
    let s = world();
    let said = |id: &str| s.inbox_rows().iter().find(|r| r.info.id == id).map(|r| r.said);
    assert_eq!(said("ses_ib11"), Some(Said::Done));
    assert_eq!(said("ses_ib12"), Some(Said::Stopped));
    assert_eq!(said("ses_ib13"), Some(Said::Blocked));
    assert_eq!(said("ses_ib02"), Some(Said::Nothing));
    assert_eq!(said("ses_ib10"), None, "waiting on its background work: working");
    assert_eq!(s.lead_of("ses_ib15"), Some("ses_ib14"), "a helper inherits the tag but folds into its task");
    assert_eq!(s.project_of_task("ses_ib15"), Some(MAIN));
    assert!(s.is_task_session("ses_ib14") && !s.is_task_session("ses_ib15") && !s.is_task_session("ses_ib01"));
}

#[test]
fn a_reply_to_a_task_puts_it_back_to_working() {
    let mut s = world();
    let mut meta = s.session("ses_ib11").unwrap().metadata.clone();
    meta["seerTask"]["state"] = "working".into();
    s.apply_event("session.metadata.updated", &serde_json::json!({ "sessionID": "ses_ib11", "metadata": meta }));
    assert!(!done(&s, "ses_ib11"));
    assert_eq!(s.project_working(MAIN), 4);
}

#[test]
fn opening_a_task_session_gives_it_the_composer_but_no_tab() {
    let mut s = world();
    let _writes = s.fake_writes();
    let tabs: Vec<String> = s.tabs.iter().map(|t| t.id.clone()).collect();
    s.open("ses_ib11", &mut Tx::default());
    assert_eq!(s.route, Route::Session("ses_ib11".into()));
    assert_eq!(s.tabs.iter().map(|t| t.id.clone()).collect::<Vec<_>>(), tabs, "⌘1–9 stay put");
    assert!(s.composer.focused, "it's top-level, so you can reply");
    assert!(done(&s, "ses_ib11"), "the open one stays until E");
}

#[test]
fn task_sessions_stay_out_of_tabs_the_switcher_and_home() {
    let s = world();
    assert!(!s.recent_tabs().iter().any(|t| t.id.starts_with("ses_ib1")));
    assert!(!s.home_sessions(None).iter().any(|x| x.is_seer_task()));
    assert!(!s.switcher_entries().iter().any(|e| e.key == "session:ses_ib11"));
}

#[test]
fn inherited_metadata_doesnt_make_a_child_a_project() {
    let mut s = world();
    let meta = s.session(MAIN).unwrap().metadata.clone();
    s.sessions.iter_mut().find(|i| i.id == "ses_ib02").unwrap().metadata = meta;
    assert!(!s.is_project("ses_ib02"));
    assert_eq!(s.project_marks().len(), 2);
}

#[test]
fn only_what_was_started_after_the_mark_is_a_task() {
    let mut s = world();
    let now = s.now_ms;
    for id in [MAIN, SIXTH] {
        s.sessions.iter_mut().find(|i| i.id == id).unwrap().metadata["seerProject"]["since"] = now.into();
    }
    assert!(s.inbox_rows().iter().all(|r| r.info.is_seer_task()), "a task with its own session needs no mark time");
}

#[test]
fn opening_a_task_reads_it_on_the_server_and_it_leaves_once_moved_on_from() {
    let mut s = world();
    let writes = s.fake_writes();
    let mut tx = Tx::default();
    s.open("ses_ib06", &mut tx);
    assert!(matches!(writes.try_recv(), Ok(Write::View { session, .. }) if session == "ses_ib06"));
    assert!(done(&s, "ses_ib06"), "the open one stays");
    s.open(MAIN, &mut tx);
    assert!(!done(&s, "ses_ib06"));
}

#[test]
fn a_task_finishing_under_its_open_project_stays_unread() {
    let mut s = world();
    s.route = Route::Session(MAIN.into());
    s.apply_event("session.execution.succeeded", &serde_json::json!({ "sessionID": "ses_ib07" }));
    assert!(s.session("ses_ib07").unwrap().unread());
    assert!(done(&s, "ses_ib07"));
}

#[test]
fn e_reads_a_task_until_it_goes_idle_again() {
    let mut s = world();
    let writes = s.fake_writes();
    s.mark_task_read("ses_ib09", &mut Tx::default());
    assert!(matches!(writes.try_recv(), Ok(Write::View { session, .. }) if session == "ses_ib09"));
    assert!(!done(&s, "ses_ib09"));
    s.now_ms += 1000;
    s.apply_event("session.execution.succeeded", &serde_json::json!({ "sessionID": "ses_ib09" }));
    assert!(done(&s, "ses_ib09"), "it came back by itself");
}

#[test]
fn close_archives_a_task_and_another_client_can_bring_it_back() {
    let mut s = world();
    let writes = s.fake_writes();
    let t = Target::new("ses_ib09", Spot::Inbox);
    assert!(session_menu::lines(&s, &t).iter().any(|l| matches!(l, session_menu::Line::Item(i) if i.title == "Close")));
    session_menu::run(&mut s, &t, "close-task", &mut Tx::default());
    assert!(matches!(writes.try_recv(), Ok(Write::Archive(id)) if id == "ses_ib09"));
    assert!(!done(&s, "ses_ib09"));
    s.apply_event("session.unarchived", &serde_json::json!({ "sessionID": "ses_ib09" }));
    assert!(done(&s, "ses_ib09"));
}

#[test]
fn the_menu_marks_and_unmarks_a_project_in_its_metadata() {
    super::force(true);
    let mut s = App::fixture_session();
    let writes = s.fake_writes();
    let t = Target::new(crate::fixture_ids::THIRD, Spot::Tab);
    session_menu::run(&mut s, &t, "project", &mut Tx::default());
    assert!(s.is_project(&t.id));
    let Ok(Write::Metadata { session, metadata }) = writes.try_recv() else { panic!("sent the mark") };
    assert_eq!((session.as_str(), metadata["seerProject"]["id"].as_str()), (t.id.as_str(), Some(t.id.as_str())));
    assert!(session_menu::lines(&s, &t).iter().any(|l| matches!(l, session_menu::Line::Item(i) if i.title == "Stop being a project")));
    session_menu::run(&mut s, &t, "project", &mut Tx::default());
    assert!(!s.is_project(&t.id));
    assert!(matches!(writes.try_recv(), Ok(Write::Metadata { metadata, .. }) if metadata["seerProject"].is_null()));
}

#[test]
fn another_clients_mark_arrives_as_an_event() {
    super::force(true);
    let mut s = App::fixture_session();
    let id = crate::fixture_ids::THIRD;
    s.apply_event("session.metadata.updated", &serde_json::json!({ "sessionID": id, "metadata": { "seerProject": { "id": id, "since": 1 } } }));
    assert!(s.is_project(id));
}

#[test]
fn new_project_marks_the_session_it_opens() {
    super::force(true);
    let mut s = App::fixture_session();
    let _writes = s.fake_writes();
    s.project_inbox.mark_next = true;
    super::created(&mut s, crate::fixture_ids::THIRD, &mut Tx::default());
    assert!(s.is_project(crate::fixture_ids::THIRD));
    assert!(!s.project_inbox.mark_next);
    super::created(&mut s, MAIN, &mut Tx::default());
    assert!(!s.is_project(MAIN), "only the next one");
}

#[test]
fn the_sidebar_lists_projects_instead_of_their_tabs() {
    let mut rt = world().into_runtime();
    rt.frame(0.0);
    for want in ["h-Needs you · 3", "h-Done · 5", "glyph-Done", "glyph-Stopped", "h-Projects", &format!("ip-{MAIN}"), &format!("ip-{SIXTH}"), "ib-ses_ib01", "ib-ses_ib09"] {
        assert!(rt.find(want).is_some(), "{want}");
    }
    assert!(rt.find("ib-ses_ib04").is_none(), "a working task has no row");
    assert!(rt.find("ib-ses_ib03").is_none(), "a helper folds into its task");
    assert!(rt.find(&format!("t-{MAIN}")).is_none(), "a project's tab is its Projects line");
}

#[test]
fn with_projects_off_the_sidebar_has_no_inbox_and_marked_sessions_are_ordinary() {
    let mut s = world();
    super::force(false);
    assert!(s.project_marks().is_empty() && s.inbox_rows().is_empty() && s.lead_of("ses_ib03").is_none());
    let writes = s.fake_writes();
    s.toggle_project(MAIN, &mut Tx::default());
    s.new_project(&mut Tx::default());
    assert!(!s.project_inbox.mark_next);
    assert!(!s.commands().iter().any(|c| c.id == "project.new"));
    let menu = session_menu::lines(&s, &Target::new(MAIN, Spot::Tab));
    assert!(!menu.iter().any(|l| matches!(l, session_menu::Line::Item(i) if i.id == "project")));
    let mut rt = s.into_runtime();
    rt.frame(0.0);
    for gone in ["inbox", "h-Projects", "h-Needs you · 3"] {
        assert!(rt.find(gone).is_none(), "{gone}");
    }
    assert!(rt.find(&format!("t-{MAIN}")).is_some(), "a marked session keeps its tab");
    assert!(writes.try_iter().next().is_none(), "nothing written to the server");
}

#[test]
fn e_on_a_row_marks_its_task_read() {
    let mut rt = world().into_runtime();
    let writes = rt.state.fake_writes();
    rt.frame(0.0);
    let (x, y) = rt.find("ib-ses_ib06").expect("its row");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.1);
    assert_eq!(rt.state.route, Route::Session("ses_ib06".into()), "a click opens it");
    rt.key(rune::KeyEvent { key: rune::Key::Text("e".into()), mods: Default::default() });
    rt.frame(0.2);
    let sent: Vec<_> = writes.try_iter().filter_map(|w| match w { Write::View { session, .. } => Some(session), _ => None }).collect();
    assert_eq!(sent, ["ses_ib06"], "opening read it; E only lets it go");
    assert!(rt.find("ib-ses_ib06").is_none());
}

#[test]
fn a_project_or_task_row_leaves_settings_for_it() {
    for (row, id) in [(format!("ip-{MAIN}"), MAIN), ("ib-ses_ib02".to_owned(), "ses_ib02")] {
        let mut rt = crate::settings::leave_tests::in_settings(world().into_runtime());
        crate::settings::leave_tests::tap(&mut rt, &row);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        assert!(!rt.state.settings.open, "{row}");
        assert_eq!(rt.state.route, Route::Session(id.into()));
    }
}

#[test]
fn a_done_task_answered_leaves_done_while_it_works_and_comes_back() {
    let mut s = world();
    let _writes = s.fake_writes();
    let mut tx = Tx::default();
    // Answered from its page, and still there when it finishes.
    s.open("ses_ib06", &mut tx);
    s.now_ms += 1000;
    s.apply_event("session.execution.started", &serde_json::json!({ "sessionID": "ses_ib06" }));
    assert!(!done(&s, "ses_ib06"), "working, it isn't done");
    s.now_ms += 1000;
    s.apply_event("session.execution.succeeded", &serde_json::json!({ "sessionID": "ses_ib06" }));
    assert!(done(&s, "ses_ib06"), "finished, it's back");
    // Answered, then left for another project.
    s.open("ses_ib09", &mut tx);
    s.now_ms += 1000;
    s.apply_event("session.execution.started", &serde_json::json!({ "sessionID": "ses_ib09" }));
    s.open(SIXTH, &mut tx);
    assert!(!done(&s, "ses_ib09"));
    s.now_ms += 1000;
    s.apply_event("session.execution.succeeded", &serde_json::json!({ "sessionID": "ses_ib09" }));
    assert!(done(&s, "ses_ib09"));
}
