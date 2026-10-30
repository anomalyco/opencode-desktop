use opencode_kit::client::{Incoming, Write};
use opencode_kit::model::{LocationInfo, Project, ProjectIcon, SessionInfo};
use rune::{FileDialogKind, Key, KeyEvent, Mods, Runtime};

use crate::pickers::{ADD_PROJECT_KEY, Kind};
use crate::{App, Route, fixture_runtime};

fn cmd_o(rt: &mut Runtime<App>) {
    rt.key(KeyEvent { key: Key::Command('o'), mods: Mods { cmd: true, ..Default::default() } });
    rt.frame(1.0 / 60.0);
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let e = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).expect(key);
    let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn cmd_o_opens_folder_dialog_and_cancel_restores_composer_focus() {
    let mut rt = fixture_runtime();
    rt.update(|s, _| s.composer.focused = true);
    rt.frame(1.0 / 60.0);
    let before = rt.state.route.clone();

    cmd_o(&mut rt);
    let (kind, opts) = rt.file_dialog().expect("⌘O asked for a folder dialog");
    assert_eq!(kind, FileDialogKind::Folder);
    assert_eq!(opts.title, "Open project");

    rt.resolve_file_dialog(None).unwrap();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.route, before);
    assert!(rt.state.composer.focused, "cancelling returns focus to the composer");
}

#[test]
fn picking_a_folder_adds_the_project_opens_a_session_and_puts_it_in_the_sidebar() {
    let mut rt = fixture_runtime();
    let tabs_before = rt.state.tabs.len();

    cmd_o(&mut rt);
    rt.resolve_file_dialog(Some("/Users/demo/code/orbit/".into())).unwrap();
    rt.frame(1.0 / 60.0);

    let project = rt.state.projects.iter().find(|p| p.directory == "/Users/demo/code/orbit").expect("orbit added");
    assert_eq!(project.name, "orbit");
    assert_eq!(rt.state.project_state.used.first().map(String::as_str), Some(project.id.as_str()));

    let Route::Session(sid) = rt.state.route.clone() else { panic!("expected session route, got {:?}", rt.state.route) };
    let info = rt.state.session(&sid).expect("session created");
    assert_eq!(info.directory, "/Users/demo/code/orbit");
    assert_eq!(info.project, project.id);
    assert!(rt.state.composer.focused);

    assert_eq!(rt.state.tabs.len(), tabs_before + 1);
    let tab = rt.state.tabs.last().unwrap();
    assert_eq!(tab.id, sid);
    assert_eq!(tab.directory, "/Users/demo/code/orbit");

    let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&format!("t-{sid}"))).expect("tab row in sidebar");
    assert!(!row.moving && row.rect == row.target && (row.opacity - 1.0).abs() < 1e-3, "sidebar tab lands settled");
}

#[test]
fn draft_project_picker_add_project_and_cmd_k_trigger_open_project() {
    let mut rt = fixture_runtime();
    rt.update(|s, tx| s.new_session(tx));
    rt.frame(1.0 / 60.0);

    tap(&mut rt, "project");
    assert_eq!(rt.state.pickers.open, Some(Kind::Project));
    tap(&mut rt, &format!("option-{ADD_PROJECT_KEY}"));
    assert_eq!(rt.state.pickers.open, None);
    assert_eq!(rt.file_dialog().map(|(k, _)| k), Some(FileDialogKind::Folder));
    rt.resolve_file_dialog(Some("/Users/demo/code/nova".into())).unwrap();
    rt.frame(1.0 / 60.0);
    assert!(matches!(&rt.state.route, Route::Session(id) if rt.state.session(id).unwrap().directory == "/Users/demo/code/nova"));

    // ⌘K finds "Open project" and triggers the folder picker too.
    rt.key(KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ..Default::default() } });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Text("open project".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.switcher_entries().iter().any(|e| e.key == "command:open-project" && e.hint == "⌘O"));
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.switcher.open);
    assert_eq!(rt.file_dialog().map(|(k, _)| k), Some(FileDialogKind::Folder));
}

#[test]
fn live_open_project_queues_write_and_applies_server_reply() {
    let mut rt = fixture_runtime();
    let writes = rt.state.fake_writes();
    let (inbox_tx, inbox_rx) = std::sync::mpsc::channel();
    rt.state.inbox = Some(inbox_rx);

    cmd_o(&mut rt);
    rt.resolve_file_dialog(Some("/Users/demo/code/beacon".into())).unwrap();
    rt.frame(1.0 / 60.0);

    let sent = writes.try_recv().expect("queued OpenProject write");
    assert!(matches!(sent, Write::OpenProject { ref directory, start_session: true } if directory == "/Users/demo/code/beacon"));

    let mut projects = rt.state.projects.clone();
    projects.push(Project {
        id: "proj_beacon".into(),
        name: "beacon".into(),
        directory: "/Users/demo/code/beacon".into(),
        active: rt.state.now_ms + 1000,
        icon: ProjectIcon::default(),
    });
    inbox_tx
        .send(Incoming::ProjectOpened {
            location: LocationInfo {
                directory: "/Users/demo/code/beacon".into(),
                project_id: "proj_beacon".into(),
                project_directory: "/Users/demo/code/beacon".into(),
                canonical: "/Users/demo/code/beacon".into(),
            },
            projects,
            session: Some(SessionInfo {
                id: "ses_live_beacon_0001".into(),
                project: "proj_beacon".into(),
                directory: "/Users/demo/code/beacon".into(),
                updated: rt.state.now_ms + 1000,
                created: rt.state.now_ms + 1000,
                ..Default::default()
            }),
        })
        .unwrap();
    rt.frame(1.0 / 60.0);

    assert_eq!(rt.state.route, Route::Session("ses_live_beacon_0001".into()));
    assert!(rt.state.tabs.iter().any(|t| t.id == "ses_live_beacon_0001"));
    assert_eq!(rt.state.projects[0].id, "proj_beacon");
    assert!(rt.state.composer.focused);
}
