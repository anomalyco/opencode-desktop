//! Settings › Worktrees (`settings/workspaces/workspaces.tsx`): every worktree the server keeps
//! for the projects (its inventory and their sandboxes), each with its folder, how many sessions
//! it holds and in which project, and Delete (the worktree and its branch, from the server); the
//! active one can't go. With none, "No worktrees yet".

use opencode_kit::path;
use rune::{El, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::json;

use super::{header_with, page};
use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use crate::popup::{self, Item, Menu};
use opencode_i18n::t;

const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);

#[derive(Default)]
pub struct State {
    /// Worktrees being deleted, so their rows wait.
    pub deleting: Vec<String>,
}

/// Asks for each Git project's worktrees.
pub fn load(s: &App) {
    for p in &s.projects {
        s.list_worktrees(&p.id);
    }
}

/// `(project id, project name, worktree folder)` for every worktree, by project; `only`'s alone
/// when given.
fn worktrees(s: &App, only: Option<&str>) -> Vec<(String, String, String)> {
    s.projects.iter().filter(|p| only.is_none_or(|id| p.id == id)).flat_map(|p| {
        let name = super::projects::name(p);
        s.project_worktrees(&p.id, &p.directory).into_iter().map(move |dir| (p.id.clone(), name.clone(), dir))
    }).collect()
}

pub fn view(s: &App, p: &Pal) -> El {
    view_for(s, p, None)
}

/// Worktrees, of project `only` alone on its own pages (`SettingsWorkspaces projectID`).
pub fn view_for(s: &App, p: &Pal, only: Option<&str>) -> El {
    let list = worktrees(s, only);
    let body = if list.is_empty() {
        col().key("settings-workspaces-empty").w_full().gap(4.0)
            .child(text(t!("settings.workspaces.empty")).font(NAME).color(p.text))
            .child(text(t!("settings.workspaces.empty.description")).font(BODY).color(p.muted))
    } else {
        let n = list.len();
        let active = s.active_session_id().and_then(|id| s.session(&id).map(|i| i.directory.clone()));
        let rows: Vec<El> = list.iter().enumerate().map(|(i, (project, name, dir))| worktree_row(s, p, project, name, dir, active.as_deref(), i + 1 < n)).collect();
        col().w_full().gap(8.0)
            .child(row().key("settings-workspaces-toolbar").w_full().pb(8.0)
                .child(text(opencode_i18n::plural!("settings.workspaces.count", n)).font(BODY).color(p.muted)))
            .child(col().key("settings-workspaces-inventory").w_full().padding(16.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border)).children(rows))
    };
    let switcher = if only.is_none() { super::servers::switcher(s, p) } else { None };
    page(p, "settings-workspaces-page", header_with(p, t!("settings.tab.workspaces"), t!("settings.workspaces.description"), switcher), body)
}

fn worktree_row(s: &App, p: &Pal, project: &str, project_name: &str, dir: &str, active: Option<&str>, divided: bool) -> El {
    let sessions = s.sessions.iter().filter(|x| path::trim(&x.directory) == path::trim(dir)).count();
    let is_active = active.is_some_and(|a| path::trim(a) == path::trim(dir));
    let deleting = s.settings_pages.worktrees.deleting.iter().any(|d| d == dir);
    let base = path::basename(dir).to_owned();
    let parent = dir.strip_suffix(&base).unwrap_or_default().to_owned();
    let key = format!("settings-worktree-delete-{}", path::trim(dir));
    let (project, dir_owned, anchor) = (project.to_owned(), dir.to_owned(), key.clone());
    let confirm = t!("workspace.delete.confirm", name = base);
    let delete = row().key(key).role("button").label(confirm.clone()).help(if is_active { t!("settings.workspaces.delete.blocked.active").to_owned() } else { confirm })
        .size(24.0, 24.0).center().radius(4.0)
        .child(icon(&ic::OUTLINE_TRASH, 16.0).color(if is_active || deleting { p.icon_faint } else { p.icon_muted }));
    let delete = if is_active || deleting { delete.disabled(true) } else {
        delete.hover(p.hover).press(p.pressed).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let menu = Menu::new(t!("workspace.delete.button")).align(rune::Align::End).group(t!("settings.workspaces.delete.warning")).item(Item::new("delete", t!("workspace.delete.button")).danger(true));
            let (project, dir) = (project.clone(), dir_owned.clone());
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut App, picked, _| {
                if picked == Some("delete") {
                    remove(s, project, dir);
                }
            });
        })
    };
    // The project's name in its own ink, found from the end as Desktop's `sessionCount` finds it.
    let label = opencode_i18n::plural!("settings.workspaces.sessions", sessions, project = project_name);
    let (before, after) = label.rfind(project_name).map_or((label.as_str(), ""), |at| (&label[..at], &label[at + project_name.len()..]));
    let meta = row().gap(0.0)
        .children((!before.is_empty()).then(|| text(before.to_owned()).font(BODY).color(p.faint)))
        .children(label.rfind(project_name).map(|_| text(project_name.to_owned()).font(BODY).color(p.muted)))
        .children((!after.is_empty()).then(|| text(after.to_owned()).font(BODY).color(p.faint)));
    col().key(format!("settings-worktree-{}", path::trim(dir))).w_full().gap(12.0).pb(if divided { 20.0 } else { 0.0 }).mb(if divided { 20.0 } else { 0.0 })
        .child(row().w_full().gap(20.0).items_start()
            .child(col().grow(1.0).shrink().min_w(0.0).gap(8.0)
                .child(row().min_w(0.0).help(dir.to_owned()).child(text(parent).font(BODY).color(p.muted).shrink().min_w(0.0).truncate_head()).child(text(base).font(NAME).color(p.text)))
                .child(meta))
            .child(row().gap(12.0).center_y()
                .children(is_active.then(|| text(t!("rune.settings.workspaces.active")).font(BODY).color(p.faint)))
                .child(delete)))
        .children(divided.then(|| el().pin(Some(0.0), None, Some(0.0), Some(0.0)).h(0.5).fill(p.border)))
}

/// Deletes the worktree on the server (`DELETE /api/worktree`), then lists the project again.
fn remove(s: &mut App, project: String, dir: String) {
    s.settings_pages.worktrees.deleting.push(dir.clone());
    s.job(move |srv| {
        let body = json!({ "projectID": project, "directory": dir, "force": false });
        let failed = srv.write("DELETE", "/api/worktree", Some(body)).err();
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.settings_pages.worktrees.deleting.retain(|d| *d != dir);
            if let Some(e) = failed {
                crate::status_toast::say(s, t!("workspace.delete.failed.title"), &e);
            }
            s.list_worktrees(&project);
        }) as Apply
    });
}
