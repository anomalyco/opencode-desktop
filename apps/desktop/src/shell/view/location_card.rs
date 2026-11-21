//! The location card the header's location opens (`SessionLocation`'s `Menu.Content`): the
//! project, the workspace (with the Session details plugin's Move session to), the branch and the
//! folder as submenus (Copy branch name, Open in terminal; Copy path, Reveal, Open in), the
//! session's changes, the server when it isn't this computer's only one, and Edit project.

use std::collections::HashMap;

use rune::Tx;
use rune_theme_opencode::icons as ic;

use super::location::TRIGGER;
use crate::App;
use crate::jobs::Apply;
use crate::popup::{self, Item, Menu};
use opencode_i18n::t;

/// Each folder's branch, once read (`None` for a folder that isn't on one), and the recovery
/// dialog for a new worktree the session didn't move into.
#[derive(Default)]
pub struct Location {
    branches: HashMap<String, Option<String>>,
    changes: Changes,
    pub(super) recovery: Option<super::move_recovery::Recovery>,
}

/// The location menu's working-tree count is ungrouped, independent of the Review tab's scope.
#[derive(Default)]
enum Changes {
    #[default]
    Closed,
    Reading(String),
    Ready(usize, u32, u32),
    Failed,
}

/// Opens the location card for the routed session, and reads the branches it shows: its
/// folder's, and its project root's for a folder on none.
pub(super) fn open_card(s: &mut App, tx: &mut Tx) {
    tx.instant();
    if s.popup.open_at(TRIGGER) {
        return popup::dismiss(s, tx);
    }
    let crate::Route::Session { id, .. } = &*s.route else { return };
    let Some(info) = s.session(id).cloned() else { return };
    let session_dir = s.queue.starts.directory(&info.id).unwrap_or(&info.directory).to_owned();
    // The changes row keeps the session's changes fresh while the card shows, and Move session
    // to lists the project's worktrees as they are now.
    if s.live() {
        let dir = session_dir.clone();
        s.location.changes = Changes::Reading(dir.clone());
        s.job(move |srv| {
            let path = format!("/api/vcs/diff?mode=working&location%5Bdirectory%5D={}", crate::switcher::encode(&dir));
            let answer = srv.get(&path).ok().and_then(|v| {
                let rows = v["data"].as_array().or_else(|| v.as_array())?;
                let (a, d) = rows.iter().fold((0, 0), |(a, d), f| (a + f["additions"].as_u64().unwrap_or_default() as u32, d + f["deletions"].as_u64().unwrap_or_default() as u32));
                Some(Changes::Ready(rows.len(), a, d))
            }).unwrap_or(Changes::Failed);
            Box::new(move |s: &mut App, tx: &mut Tx| {
                if matches!(&s.location.changes, Changes::Reading(p) if p == &dir) {
                    s.location.changes = answer;
                    reshow(s, tx);
                }
            }) as Apply
        });
    } else {
        s.ensure_session_review(&info.id);
    }
    s.list_worktrees(&info.project);
    show(s, &info, tx);
    let root = s.project(&info.project).or_else(|| s.project_at(&session_dir)).map(|p| p.directory.clone());
    for dir in std::iter::once(session_dir).chain(root) {
        if s.location.branches.contains_key(&dir) {
            continue;
        }
        s.job(move |srv| {
            let path = format!("/api/vcs?location%5Bdirectory%5D={}", crate::switcher::encode(&dir));
            let branch = srv.get(&path).ok().map(|v| v["data"]["branch"]["current"].as_str().or_else(|| v["branch"]["current"].as_str()).map(str::to_owned));
            Box::new(move |s: &mut App, tx: &mut Tx| {
                let Some(branch) = branch else { return };
                s.location.branches.insert(dir, branch);
                reshow(s, tx);
            }) as Apply
        });
    }
}

/// Draws the open card again with what it's learned.
fn reshow(s: &mut App, tx: &mut Tx) {
    if s.popup.open_at(TRIGGER)
        && let crate::Route::Session { id, .. } = &*s.route
        && let Some(info) = s.session(id).cloned()
    {
        tx.instant();
        show(s, &info, tx);
    }
}

fn show(s: &mut App, info: &opencode_kit::model::SessionInfo, tx: &mut Tx) {
    let dir = s.queue.starts.directory(&info.id).unwrap_or(&info.directory).to_owned();
    let project = s.project(&info.project).or_else(|| s.project_at(&dir)).cloned();
    let name = project.as_ref().map_or_else(|| crate::view::project_or_dir(None, &dir), |p| p.name.clone());
    let sid = info.id.clone();
    let git = project.as_ref().is_some_and(|p| opencode_kit::model::project_git(&p.id)) || s.location.branches.get(&dir).is_some_and(Option::is_some);
    let worktree = super::location::worktree_name(project.as_ref(), &dir);
    let child = info.parent.is_some();

    let mut menu = Menu::new(name.clone()).width(320.0).card();
    let mut head = Item::new("project", name.clone()).enabled(project.is_some());
    if let Some(p) = &project {
        head = head.project(p.directory.clone());
    }
    menu = menu.item(head);

    if worktree.is_some() || git {
        let (glyph, label) = match &worktree {
            Some(w) => (&ic::OUTLINE_WORKTREE, w.clone()),
            None => (&ic::MONITOR, t!("plugin.details.workspace.local").to_owned()),
        };
        // Session details' workspace row: where a root session can move (a subagent's only tells).
        menu = match &project {
            Some(p) if !child => menu.submenu_icon("workspace", label, glyph, move_menu(s, p, &dir, info)),
            _ => menu.item(Item::new("workspace", label).icon(glyph).detail()),
        };
    }

    let branch = s.location.branches.get(&dir).cloned().flatten();
    if git {
        menu = match &branch {
            Some(b) => {
                let mut sub = Menu::new(b.clone()).min_width(172.0).item(Item::new("copy-branch", t!("session.location.copyBranch")).icon(&ic::OUTLINE_COPY));
                // The terminal plugin's row (`session.location.branch.menu`).
                if s.live_terminals().is_none_or(|(_, observer)| !observer) {
                    sub = sub.item(Item::new("open-terminal", t!("plugin.terminal.location.open")).icon(&ic::OUTLINE_CONSOLE));
                }
                menu.submenu_icon("branch", b.clone(), &ic::BRANCH, sub)
            }
            None => {
                let base = project.as_ref().filter(|p| p.directory != dir).and_then(|p| s.location.branches.get(&p.directory).cloned().flatten());
                let line = match base {
                    Some(base) => format!("{} \u{b7} {}", t!("plugin.details.noBranch"), t!("plugin.details.basedOn", branch = base)),
                    None => t!("plugin.details.noBranch").to_owned(),
                };
                menu.item(Item::new("no-branch", line).icon(&ic::BRANCH).detail())
            }
        };
    }

    // Session details' changes row (`session.location.menu`): picking it opens the review.
    if git {
        menu = menu.item(changes_row(s, &sid));
    }

    menu = menu.separator();
    if let crate::Source::Live { server, .. } = &*s.source
        && !server.is_local()
    {
        menu = menu.item(Item::new("server", server.base.clone()).icon(&ic::SERVER).detail());
    }
    let mut folder = Menu::new(dir.clone()).item(Item::new("copy-path", t!("session.location.copyPath")).icon(&ic::OUTLINE_COPY));
    if matches!(&*s.source, crate::Source::Live { server, .. } if server.is_local()) {
        folder = folder.item(Item::new("reveal", reveal_label()).icon(&ic::OUTLINE_ARROW_UP_RIGHT));
    }
    // The file plugin's row (`session.location.path.menu`): Open in the chosen app.
    if let Some((_, label)) = crate::open_in::current_non_finder(s) {
        folder = folder.item(Item::new("open-in", t!("plugin.file.open.ariaLabel", app = label)).icon(&ic::OUTLINE_ARROW_UP_RIGHT));
    }
    menu = menu.submenu_icon("path", elide_middle(&dir, PATH_W), &ic::FOLDER, folder);
    menu = menu.separator().item(Item::new("settings", t!("dialog.project.edit.title")).icon(&ic::SETTINGS_GEAR).enabled(project.is_some()));

    let project_id = project.as_ref().map(|p| p.id.clone());
    let root = project.as_ref().map(|p| p.directory.clone());
    popup::open_drawn(s, menu, TRIGGER, tx, move |s, picked, tx| match picked {
        Some("project") => {
            let server = crate::servers::runtime::active(s).clone();
            if s.project_navigation() && let Some(proj) = project.as_ref() {
                return super::home::browse::history(s, &server, proj, tx);
            }
            crate::projects::home::show(s, &server, root.as_deref());
            s.go_home(tx);
        }
        Some("copy-branch") => copied(s, branch.as_deref().unwrap_or_default(), tx),
        Some("open-terminal") => s.create_terminal_tab(None, tx),
        Some("changes") => s.show_review(&sid, tx),
        Some("copy-path") => copied(s, &dir, tx),
        Some("reveal") => {
            if let Err(e) = reveal(&dir) {
                crate::status_toast::fail(s, t!("common.requestFailed"), e);
            }
        }
        Some("open-in") => {
            if let Some((app, _)) = crate::open_in::current_non_finder(s) {
                s.open_in_app(&app, "", false);
            }
        }
        Some("settings") => match root.as_deref() {
            Some(dir) => s.edit_project(dir, tx),
            None => {
                tx.instant();
                s.open_settings();
            }
        },
        Some(id) if id.starts_with("move-") => move_session(s, &sid, &dir, project_id.as_deref(), root.as_deref(), id),
        _ => {}
    });
}

/// The changes row: "N Changed files" and its lines, "No changes", or "Loading changes…".
fn changes_row(s: &App, sid: &str) -> Item {
    let item = |title: &str| Item::new("changes", title.to_owned()).icon(&ic::REVIEW);
    if s.live() {
        return match s.location.changes {
            Changes::Ready(0, ..) => item(t!("plugin.details.noChanges")),
            Changes::Ready(n, a, d) => item(&opencode_i18n::plural!("ui.sessionTurn.diffs.changed", n)).diff(a, d),
            Changes::Failed => item(t!("rune.review.readFailed")),
            _ => item(t!("plugin.details.loadingChanges")),
        };
    }
    match s.kernel.slots.get(&crate::review::REVIEW).sessions.get(sid) {
        Some(r) if r.load.ready() && r.meta.is_empty() => item(t!("plugin.details.noChanges")),
        Some(r) if r.load.ready() => {
            let (added, removed) = r.totals();
            let n = r.meta.len();
            item(&opencode_i18n::plural!("ui.sessionTurn.diffs.changed", n)).diff(added, removed)
        }
        _ => item(t!("plugin.details.loadingChanges")),
    }
}

/// Session details' Move session to (`SessionWorkspaceMenu`): the project's root, a new
/// worktree, and its other worktrees; disabled while the session runs.
fn move_menu(s: &App, project: &opencode_kit::model::Project, dir: &str, info: &opencode_kit::model::SessionInfo) -> Menu {
    let running = s.busy(&info.id);
    let norm = |d: &str| d.trim_end_matches(['/', '\\']).replace('\\', "/");
    let mut menu = Menu::new(t!("plugin.details.move.menu")).min_width(200.0).width(200.0).group(t!("plugin.details.move.menu"));
    if norm(&project.directory) != norm(dir) {
        menu = menu.item(Item::new("move-local", t!("plugin.details.workspace.local")).icon(&ic::MONITOR).enabled(!running));
    }
    menu = menu.item(Item::new("move-new", t!("plugin.details.workspace.new")).icon(&ic::PLUS).enabled(!running));
    // The server's worktree inventory, as Desktop lists it when the menu opens (`list-worktrees`).
    let worktrees: Vec<String> = s.project_worktrees(&project.id, &project.directory).into_iter().filter(|w| norm(w) != norm(dir)).collect();
    // The existing worktrees in a submenu of their own, flush against its row (`Menu.Sub gutter={0}`).
    if !worktrees.is_empty() {
        let mut existing = Menu::new(t!("plugin.details.workspace.existing")).min_width(200.0).width(200.0).gap(0.0);
        for w in worktrees {
            let name = opencode_kit::path::basename(w.trim_end_matches(['/', '\\'])).to_owned();
            existing = existing.item(Item::new(format!("move-to:{w}"), name).icon(&ic::OUTLINE_WORKTREE).enabled(!running));
        }
        menu = menu.submenu_icon("move-existing", t!("plugin.details.workspace.existing"), &ic::OUTLINE_WORKTREE, existing);
    }
    menu
}

/// Moves session `sid` where `pick` says: the project's root, a new worktree made from `dir`,
/// or an existing worktree. A failure says so in a toast, or for a worktree it made, in the
/// recovery dialog.
fn move_session(s: &mut App, sid: &str, dir: &str, project: Option<&str>, root: Option<&str>, pick: &str) {
    let (sid, dir, project) = (sid.to_owned(), dir.to_owned(), project.unwrap_or_default().to_owned());
    let target = match pick {
        "move-local" => root.map(str::to_owned),
        "move-new" => None,
        other => other.strip_prefix("move-to:").map(str::to_owned),
    };
    if pick != "move-new" && target.is_none() {
        return;
    }
    s.job(move |srv| {
        let moved = match target {
            Some(t) => srv.move_session(&sid, &t).map(|_| None),
            None => srv.create_worktree(&project, &dir, None).map(Some),
        };
        Box::new(move |s: &mut App, _tx: &mut Tx| match moved {
            Ok(Some(created)) => super::move_recovery::move_into(s, sid, project, created),
            Ok(None) => {}
            Err(e) => crate::status_toast::fail(s, t!("plugin.details.move.failed"), e),
        }) as Apply
    });
}

/// The folder row's room for its path in the 320 card: less the card's inset, the row's
/// padding, the glyph, the chevron and their gaps.
const PATH_W: f32 = 320.0 - 4.0 - 8.0 - 8.0 - 16.0 - 8.0 - 8.0 - 16.0;

/// `path` cut in the middle to fit `max` (the folder row's `bdi`): the parent folders shrink
/// behind an ellipsis, the last folder stays whole.
fn elide_middle(path: &str, max: f32) -> String {
    let style = rune::TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
    let w = |s: &str| rune::text::measure(s, &[], style, None).0;
    if w(path) <= max {
        return path.to_owned();
    }
    let cut = path.rfind(['/', '\\']).unwrap_or(0);
    let (head, tail) = path.split_at(cut);
    let mut head: Vec<char> = head.chars().collect();
    while !head.is_empty() {
        head.pop();
        let candidate = format!("{}…{tail}", head.iter().collect::<String>());
        if w(&candidate) <= max {
            return candidate;
        }
    }
    tail.to_owned()
}

fn copied(s: &mut App, value: &str, tx: &mut Tx) {
    tx.copy(value.to_owned());
    crate::status_toast::succeed(s, t!("common.copied"), value);
}

/// The folder's row in the card (Desktop's `fileManagerApp(os).actionLabel`).
#[cfg(target_os = "windows")]
pub(crate) fn reveal_label() -> &'static str {
    opencode_i18n::t!("session.header.reveal.fileExplorer")
}
#[cfg(target_os = "macos")]
pub(crate) fn reveal_label() -> &'static str {
    opencode_i18n::t!("session.header.reveal.finder")
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn reveal_label() -> &'static str {
    opencode_i18n::t!("session.header.reveal.containingFolder")
}

/// Error string returned by [`reveal`] when a verified local path does not exist on disk,
/// so callers never need to touch `Path::exists()` on a refused UNC/mapped/reparse path.
pub(crate) const MISSING_DIR_ERR: &str = "Path doesn't exist";

/// Shows folder `dir` in the system's file manager; a headless run opens nothing. Verifies local
/// fixed drive / `MNT_LOCAL`, non-`subst`-UNC, zero symlinks/reparse points, directory kind, and
/// non-bundle on macOS BEFORE touching disk, then reveals via `browser_policy::reveal_in_file_manager`
/// (never spawns bare `explorer.exe <path>` or `/usr/bin/open <path>`).
pub(crate) fn reveal(dir: &str) -> Result<(), String> {
    if crate::headless() {
        return Ok(());
    }
    let verified = browser_policy::verify_local_path(dir).map_err(|e| match e {
        browser_policy::LocalPathError::NotFound => MISSING_DIR_ERR.to_owned(),
        other => other.to_string(),
    })?;
    if !verified.is_dir || browser_policy::is_macos_bundle(&verified.path) {
        return Err("Path is not a local folder".into());
    }
    browser_policy::reveal_in_file_manager(&verified.path).map(|_| ()).map_err(|e| e.to_string())
}
