use opencode_kit::client::Write;
use opencode_kit::model::Pending;
use rune::{Key, KeyEvent, Mods};
use serde_json::json;

use crate::App;
use crate::fixture_runtime as runtime;

fn ask(app: &mut App, id: &str) {
    app.apply_event("permission.asked", &json!({ "id": id, "sessionID": crate::stories::sid(app), "action": "bash" }));
}

fn decisions(w: &std::sync::mpsc::Receiver<Write>) -> Vec<(String, String)> {
    w.try_iter()
        .filter_map(|w| match w {
            Write::Permission { request, decision, .. } => Some((request, decision)),
            _ => None,
        })
        .collect()
}

#[test]
fn off_by_default_so_a_request_waits() {
    let mut app = App::fixture_session();
    let writes = app.fake_writes();
    ask(&mut app, "per_1");
    assert!(decisions(&writes).is_empty());
    assert!(app.pending.iter().any(|p| p.id() == "per_1"));
}

#[test]
fn turning_it_on_answers_the_open_requests_once_and_later_ones_as_they_come() {
    let mut app = App::fixture_session();
    let writes = app.fake_writes();
    ask(&mut app, "per_1");
    app.set_auto_accept(true);
    assert_eq!(decisions(&writes), [("per_1".to_owned(), "once".to_owned())]);
    // The request leaves once the server says it's answered (05cf747e).
    let sid = crate::stories::sid(&app);
    app.apply_event("client.request.replied", &json!({ "requestID": "per_1" }));
    app.apply_event("permission.replied", &json!({ "requestID": "per_1", "sessionID": sid }));
    assert!(app.pending.is_empty());
    ask(&mut app, "per_2");
    assert_eq!(decisions(&writes), [("per_2".to_owned(), "once".to_owned())]);
    app.set_auto_accept(false);
    ask(&mut app, "per_3");
    assert!(decisions(&writes).is_empty());
}

#[test]
fn forms_still_wait() {
    let mut app = App::fixture_session();
    let writes = app.fake_writes();
    app.set_auto_accept(true);
    app.add_pending(Pending::Form { id: "frm_1".into(), session: crate::stories::sid(&app), title: String::new(), fields: vec![] });
    assert!(decisions(&writes).is_empty());
    assert!(app.pending.iter().any(|p| p.id() == "frm_1"));
}

#[test]
fn shift_command_a_toggles_it_and_the_composer_shows_the_mark() {
    let mut rt = runtime();
    assert!(rt.find("auto-accept").is_none());
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, shift: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.settings.auto_accept);
    assert!(rt.find("auto-accept").is_some());
    let (x, y) = rt.find("auto-accept").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.settings.auto_accept);
}

#[test]
fn it_is_kept_in_desktops_settings_shape() {
    let mut s = crate::settings::Settings::default();
    s.auto_accept = true;
    assert_eq!(s.to_json()["permissions"]["autoApprove"], true);
    assert!(crate::settings::Settings::from_json(&s.to_json()).auto_accept);
    assert!(!crate::settings::Settings::from_json(&json!({})).auto_accept);
}
