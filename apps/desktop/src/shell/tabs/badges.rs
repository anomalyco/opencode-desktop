//! The `tab-badges` scene: sidebar tabs whose projects are easy to get wrong, from Kit's own
//! tabs (Oct 30). A repo whose sessions keep an old project ID, a worktree under a folder of
//! worktrees that was once opened as a project, two repos under a parent folder (`~/code`) that
//! was once opened, each with the server's own icon, and a subfolder holding another app's icon.

use std::path::{Path, PathBuf};

use opencode_kit::model::{parse_projects, parse_session};
use serde_json::{Value, json};

use crate::App;
use crate::tabs::Tab;

/// The Rune app's icon, written two folders down in the repos below as an embedded app repo has it: an ICNS
/// holding only the representation the icon finder takes from the app's (`icns_png`: the 256 px
/// PNG), 57 KB where the whole `icon.icns` was 1.4 MB of the exe.
const STRAY_ICON: &[u8] = include_bytes!("../../../assets/stray-icon.icns");

const SIGNAL_SVG: &str = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxNiAxNiI+PHJlY3Qgd2lkdGg9IjE2IiBoZWlnaHQ9IjE2IiBmaWxsPSIjMTExIi8+PGNpcmNsZSBjeD0iOCIgY3k9IjgiIHI9IjQiIGZpbGw9IiNmZjdhMWEiLz48L3N2Zz4=";
const PICKS_SVG: &str = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxNiAxNiI+PHJlY3Qgd2lkdGg9IjE2IiBoZWlnaHQ9IjE2IiBmaWxsPSIjMTMxMDEwIi8+PHJlY3QgeD0iNCIgeT0iMyIgd2lkdGg9IjgiIGhlaWdodD0iMTAiIGZpbGw9IiNmZmYiLz48cmVjdCB4PSI2IiB5PSI3IiB3aWR0aD0iNCIgaGVpZ2h0PSI0IiBmaWxsPSIjNWE1ODU4Ii8+PC9zdmc+";

/// Where the scene's folders live: real ones, so a directory walk has something to find.
pub(crate) fn root() -> PathBuf {
    std::env::temp_dir().join("rune-desktop-tab-badges")
}

fn stray_icon(repo: &Path) {
    let dir = repo.join("apps/desktop/assets");
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(dir.join("icon.icns"), STRAY_ICON).ok();
}

fn project(id: &str, dir: &str, icon: Option<&str>) -> Value {
    let mut p = json!({ "id": id, "canonical": dir, "vcs": "git", "time": { "created": 1, "updated": 1 }, "sandboxes": [] });
    if let Some(svg) = icon {
        p["icon"] = json!({ "url": format!("data:image/svg+xml;base64,{svg}") });
    }
    p
}

/// The scene's sessions: (id, title, project ID, directory).
pub(crate) fn sessions(root: &str) -> [(&'static str, &'static str, &'static str, String); 4] {
    [
        ("ses_badge_beacon", "BEACON", "beac0001", format!("{root}/beacon")),
        ("ses_badge_feature", "BEACON FEATURE", "beac0002", format!("{root}/worktrees/beacon-feature")),
        ("ses_badge_signal", "SIGNAL", "s1gn0001", format!("{root}/signal")),
        ("ses_badge_ios", "SIGNAL IOS", "p1ck0001", format!("{root}/signal-ios-loading")),
    ]
}

pub(crate) fn scene(s: &mut App) {
    let root = root();
    let r = root.to_string_lossy().into_owned();
    for repo in ["beacon", "signal", "signal-ios-loading", "worktrees/beacon-feature"] {
        std::fs::create_dir_all(root.join(repo)).ok();
    }
    stray_icon(&root.join("beacon"));
    stray_icon(&root.join("signal"));
    let projects = json!([
        // `~/code`, opened once: every repo below sits inside it.
        project("c0de0000", &r, None),
        // The folder of worktrees, opened once.
        project("w0rk0000", &format!("{r}/worktrees"), None),
        // The repo, its sessions still on the project ID it had before it moved.
        project("beac0002", &format!("{r}/beacon"), None),
        project("beac0001", &format!("{r}/old/beacon-rewrite"), None),
        // Repos now in a folder of another name, each with the server's icon.
        project("s1gn0001", &format!("{r}/voice"), Some(SIGNAL_SVG)),
        project("p1ck0001", &format!("{r}/signal-ios-picks"), Some(PICKS_SVG)),
    ]);
    s.projects.extend(parse_projects(&projects));
    let mut tabs = s.tabs.first().cloned().into_iter().collect::<Vec<_>>();
    for (id, title, project, directory) in sessions(&r) {
        let info = json!({ "id": id, "projectID": project, "title": title, "location": { "directory": directory }, "time": { "created": 1, "updated": 1 } });
        s.sessions.push(parse_session(&info));
        tabs.push(Tab::session(id, title, directory));
    }
    *s.tabs = tabs;
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("tab-badges", scene)]);

