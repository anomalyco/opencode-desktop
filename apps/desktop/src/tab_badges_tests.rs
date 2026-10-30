use opencode_kit::model::{parse_projects, parse_session};
use serde_json::json;

use super::{root, scene};
use crate::App;
use crate::icons::palette;
use crate::palette::pal;

fn app() -> App {
    let mut s = App::fixture_session();
    s.icons.set_sync(true);
    scene(&mut s);
    s
}

fn project_of<'a>(s: &'a App, session: &str) -> &'a str {
    let info = s.session(session).expect(session);
    s.project_for_session(info).map_or("none", |p| p.id.as_str())
}

#[test]
fn each_tab_resolves_to_its_own_project() {
    let s = app();
    // A repo whose sessions keep the ID of the project it was before: the project rooted at its
    // folder now, as the server's location lookup says (was right, keep it).
    assert_eq!(project_of(&s, "ses_badge_beacon"), "beac0002");
    // A worktree with no sandbox entry: its session's project, not the folder of worktrees that
    // contains it (was `w0rk0000`, "W").
    assert_eq!(project_of(&s, "ses_badge_feature"), "beac0002");
    // Repos under a parent folder opened once: their own projects with their own icons, not the
    // parent (both were `c0de0000`, sharing whatever icon a walk of the parent found).
    assert_eq!(project_of(&s, "ses_badge_signal"), "s1gn0001");
    assert_eq!(project_of(&s, "ses_badge_ios"), "p1ck0001");
}

#[test]
fn badges_are_the_server_image_or_the_letter_tile_never_a_file_found_in_the_folder() {
    let s = app();
    let pal = palette(pal(true));
    let image = |session: &str| {
        let p = s.project_for_session(s.session(session).unwrap()).unwrap();
        s.icons.image(&pal, p, 16.0).is_some()
    };
    // `beacon/apps/desktop/assets/icon.icns` is another app's icon, not the project's (was shown).
    assert!(root().join("beacon/apps/desktop/assets/icon.icns").exists());
    assert!(!image("ses_badge_beacon"));
    assert!(!image("ses_badge_feature"));
    assert!(image("ses_badge_signal"));
    assert!(image("ses_badge_ios"));
}

#[test]
fn of_two_projects_at_one_folder_the_sessions_own_wins() {
    let mut s = App::fixture_session();
    s.projects = parse_projects(&json!([
        { "id": "a", "canonical": "/x/wraith", "sandboxes": [] },
        { "id": "b", "canonical": "/x/wraith", "sandboxes": [] },
    ]));
    s.sessions = vec![parse_session(&json!({ "id": "ses_w", "projectID": "b", "location": { "directory": "/x/wraith" } }))];
    assert_eq!(project_of(&s, "ses_w"), "b");
}

#[test]
fn a_global_folder_resolves_by_directory() {
    let mut s = App::fixture_session();
    s.projects = parse_projects(&json!([{ "id": "scratch", "canonical": "/x/scratch", "sandboxes": [] }]));
    s.sessions = vec![parse_session(&json!({ "id": "ses_g", "projectID": "global", "location": { "directory": "/x/scratch" } }))];
    assert_eq!(project_of(&s, "ses_g"), "scratch");
}
