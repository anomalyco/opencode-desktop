//! The session header's location (`SessionLocation` in `session-identity-header.tsx`): the
//! project's avatar and name as a button that opens the location card: the project (back to it
//! on Home), its workspace, the branch, the folder, and Edit project.

use std::collections::HashMap;

use rune::{El, TextStyle, Tx, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use crate::popup::{self, Item, Menu};

/// The trigger's key: the card opens beside it.
pub const TRIGGER: &str = "location-trigger";

/// 13 on 16 at the regular weight, as the trigger's `text-[13px] font-normal`.
const LABEL: TextStyle = TextStyle::new(13.0, 400.0).lh(16.0).tracking(-0.04);

/// Each folder's branch, once read (`None` for a folder that isn't on one).
#[derive(Default)]
pub struct Location {
    branches: HashMap<String, Option<String>>,
}

/// The trigger: the 16 pt project avatar, the project's name, and for a session in one of the
/// project's worktrees a slash, the worktree glyph and its name, muted until hovered or open.
/// `collapse` is the header's fit: 1 hides the worktree's name, 2 the project's too.
pub fn trigger(s: &App, p: &Pal, project: Option<&opencode_kit::model::Project>, name: &str, worktree: Option<&str>, collapse: u8) -> El {
    let open = s.popup.open_at(TRIGGER);
    let ink = if open { p.text } else { p.muted };
    let label = match worktree {
        Some(w) => format!("{name} / {w}"),
        None => name.to_owned(),
    };
    let mut t = row()
        .key(TRIGGER)
        .role("button")
        .label(label.clone())
        .help(label)
        .h(28.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .shrink()
        .min_w(0.0)
        .hover_group()
        .hover(p.hover)
        .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| open_card(s, tx))
        .child(crate::icons::badge(s, p, project, name));
    if collapse < 2 {
        t = t.child(text(name.to_owned()).key("location-project").font(LABEL).color(ink).group_color(p.text).single_line());
    }
    if let Some(w) = worktree {
        // `SessionHeaderSeparator gap 0`: the trigger's own gap each side, less the glyph's bearing.
        t = t
            .child(text("/").font(TextStyle::new(13.0, 400.0).lh(16.0)).color(p.icon_faint).mx(-0.5))
            .child(rune::icon(&ic::OUTLINE_WORKTREE, 16.0).color(p.icon_muted));
        if collapse < 1 {
            t = t.child(text(w.to_owned()).key("location-worktree").font(LABEL).color(ink).group_color(p.text).single_line());
        }
    }
    if open { t.fill(p.hover) } else { t }
}

/// The worktree a session in `dir` runs in, by its folder's name: one of the project's sandboxes
/// (its root left out) holding `dir` (`worktree()` in `SessionLocation`).
pub fn worktree_name(project: Option<&opencode_kit::model::Project>, dir: &str) -> Option<String> {
    let project = project?;
    // Windows' paths compare without case, as Desktop's `pathKey` does there.
    let norm = |d: &str| {
        let d = d.trim_end_matches(['/', '\\']).replace('\\', "/");
        if cfg!(windows) { d.to_lowercase() } else { d }
    };
    let root = norm(&project.directory);
    let dir = norm(dir);
    opencode_kit::model::project_sandboxes(&project.id)
        .into_iter()
        .filter(|w| norm(w) != root)
        .find(|w| {
            let w = norm(w);
            dir == w || dir.starts_with(&format!("{w}/"))
        })
        .map(|w| opencode_kit::path::basename(w.trim_end_matches(['/', '\\'])).to_owned())
}

/// Opens the location card for the routed session, and reads its folder's branch for it.
fn open_card(s: &mut App, tx: &mut Tx) {
    tx.instant();
    if s.popup.open_at(TRIGGER) {
        return popup::dismiss(s, tx);
    }
    let crate::Route::Session(id) = &s.route else { return };
    let Some(info) = s.session(id).cloned() else { return };
    let dir = info.directory.clone();
    show(s, &info, tx);
    if !s.location.branches.contains_key(&dir) {
        s.job(move |srv| {
            let path = format!("/api/vcs?location%5Bdirectory%5D={}", crate::switcher::encode(&dir));
            let branch = srv.get(&path).ok().map(|v| v["data"]["branch"]["current"].as_str().or_else(|| v["branch"]["current"].as_str()).map(str::to_owned));
            Box::new(move |s: &mut App, tx: &mut Tx| {
                let Some(branch) = branch else { return };
                s.location.branches.insert(dir, branch);
                // The card shows the branch once it's known.
                if s.popup.open_at(TRIGGER)
                    && let crate::Route::Session(id) = &s.route
                    && let Some(info) = s.session(id).cloned()
                {
                    tx.instant();
                    show(s, &info, tx);
                }
            }) as Apply
        });
    }
}

fn show(s: &mut App, info: &opencode_kit::model::SessionInfo, tx: &mut Tx) {
    let project = s.project(&info.project).cloned();
    let name = project.as_ref().map_or_else(|| crate::view::project_or_dir(None, &info.directory), |p| p.name.clone());
    let dir = info.directory.clone();
    let git = project.as_ref().is_some_and(|p| opencode_kit::model::project_git(&p.id)) || s.location.branches.get(&dir).is_some_and(Option::is_some);
    let worktree = worktree_name(project.as_ref(), &dir);
    let mut menu = Menu::new(name.clone()).width(320.0);
    let mut head = Item::new("project", name.clone()).enabled(project.is_some());
    if let Some(p) = &project {
        head = head.project(p.directory.clone());
    }
    menu = menu.item(head);
    if worktree.is_some() || git {
        let (glyph, label) = match &worktree {
            Some(w) => (&ic::OUTLINE_WORKTREE, w.clone()),
            None => (&ic::MONITOR, "Local repository".to_owned()),
        };
        menu = menu.item(Item::new("workspace", label).icon(glyph));
    }
    let branch = s.location.branches.get(&dir).cloned().flatten();
    if git {
        menu = menu.item(match &branch {
            Some(b) => Item::new("copy-branch", b.clone()).icon(&ic::BRANCH),
            None => Item::new("no-branch", "No branch").icon(&ic::BRANCH),
        });
    }
    menu = menu.separator().item(Item::new("copy-path", dir.clone()).icon(&ic::FOLDER));
    if matches!(&s.source, crate::Source::Live { server, .. } if server.is_local()) {
        menu = menu.item(Item::new("reveal", REVEAL).icon(&ic::OUTLINE_ARROW_UP_RIGHT));
    }
    menu = menu.separator().item(Item::new("settings", "Edit project").icon(&ic::SETTINGS_GEAR).enabled(project.is_some()));
    let project_id = project.map(|p| p.id);
    popup::open_drawn(s, menu, TRIGGER, tx, move |s, picked, tx| match picked {
        Some("project") => {
            s.project_state.home_selected = project_id.clone();
            s.go_home(tx);
        }
        Some("copy-branch") => copied(s, branch.as_deref().unwrap_or_default(), tx),
        Some("copy-path") => copied(s, &dir, tx),
        Some("reveal") => {
            if let Err(e) = reveal(&dir) {
                crate::status_toast::fail(s, "Request failed", e);
            }
        }
        Some("settings") => {
            tx.instant();
            s.open_settings();
        }
        _ => {}
    });
}

fn copied(s: &mut App, value: &str, tx: &mut Tx) {
    tx.copy(value.to_owned());
    crate::status_toast::say(s, "Copied", value);
}

/// The folder's row in the card (Desktop's `fileManagerApp(os).actionLabel`).
#[cfg(target_os = "windows")]
const REVEAL: &str = "Reveal in File Explorer";
#[cfg(target_os = "macos")]
const REVEAL: &str = "Reveal in Finder";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const REVEAL: &str = "Open containing folder";

/// Shows folder `dir` in the system's file manager; a headless run opens nothing.
fn reveal(dir: &str) -> Result<(), String> {
    if crate::headless() {
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("/usr/bin/open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}