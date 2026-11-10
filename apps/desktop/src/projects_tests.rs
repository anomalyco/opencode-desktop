use opencode_kit::client::{Incoming, Write};
use opencode_kit::model::{LocationInfo, Project, ProjectIcon, SessionInfo};
use rune::{FileDialogKind, Key, KeyEvent, Mods, Runtime, Tx};

use crate::project_switcher::StepMode;
use crate::{App, Route, fixture_runtime};

fn cmd_o(rt: &mut Runtime<App>) {
    rt.key(KeyEvent { key: Key::Command('o'), mods: Mods { cmd: true, ..Default::default() } });
    rt.frame(1.0 / 60.0);
}

fn shift_cmd_o(rt: &mut Runtime<App>) {
    rt.key(KeyEvent { key: Key::Command('O'), mods: Mods { cmd: true, shift: true, ..Default::default() } });
    rt.frame(1.0 / 60.0);
}

fn key(rt: &mut Runtime<App>, k: Key) {
    rt.key(KeyEvent { key: k, mods: Mods::default() });
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

    let Route::Session { id: sid, .. } = rt.state.route.clone() else { panic!("expected session route, got {:?}", rt.state.route) };
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

    // The project button opens Desktop's location dialog, whose last row adds a project (b005ad28).
    tap(&mut rt, "project");
    assert!(rt.state.project_switcher.open);
    tap(&mut rt, "psw-+add");
    assert!(!rt.state.project_switcher.open);
    assert_eq!(rt.file_dialog().map(|(k, _)| k), Some(FileDialogKind::Folder));
    rt.resolve_file_dialog(Some("/Users/demo/code/nova".into())).unwrap();
    rt.frame(1.0 / 60.0);
    assert!(matches!(&rt.state.route, Route::Session { id, .. } if rt.state.session(id).unwrap().directory == "/Users/demo/code/nova"));

    // ⌘K finds "Open project" and "Search projects".
    rt.key(KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ..Default::default() } });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Text("project".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.switcher_entries().iter().any(|e| e.key == "command:open-project" && e.hint == Some(crate::keymap::Bind::OpenProject)));
    assert!(rt.state.switcher_entries().iter().any(|e| e.key == "command:select-project" && e.hint == Some(crate::keymap::Bind::SelectProject)));
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);

    // ⌘K -> open project
    rt.key(KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ..Default::default() } });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Text("open project".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    key(&mut rt, Key::Enter);
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

    assert_eq!(rt.state.route, Route::here("ses_live_beacon_0001".into()));
    assert!(rt.state.tabs.iter().any(|t| t.id == "ses_live_beacon_0001"));
    assert_eq!(rt.state.projects[0].id, "proj_beacon");
    assert!(rt.state.composer.focused);
}

#[test]
fn shift_cmd_o_switches_recent_projects_worktrees_and_other_projects() {
    let mut rt = fixture_runtime();
    // Hide kiln so it sits under "Other projects".
    let kiln_id = rt.state.projects.iter().find(|p| p.name == "kiln").unwrap().id.clone();
    rt.update(|s, tx| s.hide_project(&kiln_id, tx));
    rt.frame(1.0 / 60.0);

    shift_cmd_o(&mut rt);
    assert!(rt.state.project_switcher.open);
    let rows = rt.state.project_switcher_rows();
    assert_eq!(rows[0].title, "lumen");
    assert!(rows.iter().any(|r| r.key == "other"), "hidden project shows Other projects row");
    assert_eq!(rows.last().map(|r| r.key.as_str()), Some("+add"));

    // → on lumen drills into its worktrees: its checkout, New worktree, then the server's
    // (b005ad28).
    key(&mut rt, Key::Right);
    assert!(matches!(rt.state.project_switcher.mode, StepMode::Worktrees { .. }));
    let wt_rows = rt.state.project_switcher_rows();
    let wt_titles: Vec<&str> = wt_rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(wt_titles, ["lumen", "New worktree", "reconnect-fix"]);

    // ← steps back to projects; picking reconnect-fix starts a new session there, as Desktop's
    // location dialog does (b005ad28).
    key(&mut rt, Key::Left);
    assert_eq!(rt.state.project_switcher.mode, StepMode::Projects);
    key(&mut rt, Key::Right);
    key(&mut rt, Key::Down);
    key(&mut rt, Key::Down);
    key(&mut rt, Key::Enter);
    assert!(!rt.state.project_switcher.open);
    assert!(matches!(rt.state.route, Route::Draft(_)), "{:?}", rt.state.route);
    assert_eq!(rt.state.draft_dir(), "/Users/demo/code/worktrees/lumen/reconnect-fix");

    // On Draft, ⇧⌘O updates draft_dir in place (including finding hidden kiln by search) and keeps Route::Draft.
    rt.update(|s, tx| s.new_session(tx));
    rt.frame(1.0 / 60.0);
    shift_cmd_o(&mut rt);
    key(&mut rt, Key::Text("kiln".into()));
    key(&mut rt, Key::Enter);
    assert!(matches!(rt.state.route, Route::Draft(_)));
    assert_eq!(rt.state.draft_dir(), "/Users/demo/code/kiln");
    assert_eq!(rt.state.used_projects()[0].name, "kiln", "picking kiln moves it to most-recent used");

    // ⌘O inside ⇧⌘O closes the switcher and opens the native folder dialog.
    shift_cmd_o(&mut rt);
    cmd_o(&mut rt);
    assert!(!rt.state.project_switcher.open);
    assert_eq!(rt.file_dialog().map(|(k, _)| k), Some(FileDialogKind::Folder));
}

#[test]
fn home_projects_filter_sessions_and_follow_zoom_at_1_6x() {
    let mut rt = fixture_runtime();
    let harbor_id = rt.state.projects.iter().find(|p| p.name == "harbor").unwrap().id.clone();

    // Wide Home (1280x820 at zoom 1.0: panel is ~1004pt >= 860pt) shows the left Projects column.
    rt.update(|s, tx| s.go_home(tx));
    rt.frame(1.0 / 60.0);
    assert!(rt.elements().iter().any(|e| !e.exiting && e.key.as_deref() == Some("home-projects")));
    tap(&mut rt, &format!("hp-{harbor_id}"));
    assert_eq!(rt.state.project_state.home_selected.as_deref(), Some(harbor_id.as_str()));

    let session_rows: Vec<String> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("r-ses_")))
        .filter_map(|e| e.key.map(|k| k.trim_start_matches("r-").to_owned()))
        .collect();
    assert!(!session_rows.is_empty());
    assert!(session_rows.iter().all(|id| rt.state.session(id).unwrap().project == harbor_id));

    // Clicking "New session" on Home while harbor is selected starts the draft in harbor.
    tap(&mut rt, "new-session");
    assert!(matches!(rt.state.route, Route::Draft(_)));
    assert_eq!(rt.state.draft_dir(), "/Users/demo/code/harbor");

    // Below 768 pt (1200 px at zoom 1.6), Home's projects are Desktop's dropdown, whose popover
    // scales with zoom (408fcd21).
    rt.set_viewport((1200.0, 820.0), 1.0);
    rt.update(|s, tx| {
        s.zoom = 1.6;
        s.project_state.home_selected = None;
        s.go_home(tx);
    });
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.elements().iter().any(|e| !e.exiting && e.key.as_deref() == Some("home-projects-trigger")));
    tap(&mut rt, "home-projects-trigger");
    assert!(rt.state.project_state.home_menu_open);

    let pop = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("home-projects-popover")).expect("popover");
    let trigger = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("home-projects-trigger")).expect("trigger");
    // As wide as its full-width trigger (408fcd21).
    assert!((pop.rect[2] - trigger.rect[2]).abs() < 1.0, "popover takes its trigger's width: {:?} {:?}", pop.rect, trigger.rect);
    assert!((pop.rect[0] - trigger.rect[0]).abs() < 1.5, "popover aligns with trigger start");

    tap(&mut rt, &format!("hp-{harbor_id}"));
    assert!(!rt.state.project_state.home_menu_open);
    assert_eq!(rt.state.project_state.home_selected.as_deref(), Some(harbor_id.as_str()));

    // Escape closes an open Home projects popover.
    tap(&mut rt, "home-projects-trigger");
    assert!(rt.state.project_state.home_menu_open);
    key(&mut rt, Key::Escape);
    assert!(!rt.state.project_state.home_menu_open);
    let _ = Tx::default();
}
