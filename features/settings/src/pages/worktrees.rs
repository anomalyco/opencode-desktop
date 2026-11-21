//! Settings › Worktrees (`settings/workspaces/workspaces.tsx`): every worktree the server keeps
//! for the projects, each with its folder, how many sessions it holds and in which project, when
//! its latest session was last active, its sessions, and Delete; the toolbar's ⋯ menu deletes
//! those without sessions, or all of them. Deleting runs through `delete`'s machine, its
//! confirmations in `dialog`.

pub mod delete;
mod dialog;

use opencode_i18n::t;
use opencode_kit::client::Server;
use opencode_kit::model::{Project, SessionInfo};
use opencode_kit::path;
use opencode_search::query::percent_encode;
use opencode_ui::popup::{self, Item, Menu, MenuButton as _, PopupHost};
use rune::{Align, El, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;
use opencode_ui::focus::FocusExt;
use opencode_ui::palette::Pal;

pub use self::delete::{Effect, Event, Inspection, Machine, Phase, Preflight, Run, Scope, Skip, Target};
pub use self::dialog::dialog;

use super::{header_with, page};
use crate::WORKTREE_DELETE_OVERLAY_ID;
use crate::host::SettingsHost;
use crate::jobs::{SettingsJob, SettingsRequest};

const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// `.settings-workspaces-active`: 11 on 11 at 440.
const ACTIVE: TextStyle = TextStyle::new(11.0, 440.0).lh(11.0);

#[derive(Default)]
pub struct State {
    /// The deletes' machine.
    pub delete: Machine,
    /// Its timers on the app clock (ms).
    timers: Vec<(i64, Due)>,
    /// The toolbar's project menu (`projectFilter`), back to All projects each time the page opens.
    pub filter: Filter,
}

/// Which project's worktrees the page lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Project(String),
}

/// `.settings-workspaces-row-motion`: a deleted row closes to nothing over 150 ms and fades
/// over 120 ms, both ease-out.
pub(crate) const COLLAPSE: rune::Anim = rune::Anim::new(rune::Curve::tween(delete::COLLAPSE_MS as f32 / 1000.0, rune::Easing::EASE_OUT));
const FADE: rune::Anim = rune::Anim::new(rune::Curve::tween(0.12, rune::Easing::EASE_OUT));

#[derive(Clone, Copy, Debug, PartialEq)]
enum Due {
    Inspect(u64),
    Delete(u64),
    Collapse(u64),
}

/// Asks for each Git project's worktrees.
pub fn load(s: &mut dyn SettingsHost) {
    let ids: Vec<String> = s.projects().iter().map(|p| p.id.clone()).collect();
    for id in ids {
        s.list_worktrees(&id);
    }
}

struct Worktree {
    project: String,
    name: String,
    dir: String,
}

impl Worktree {
    fn key(&self) -> String {
        path::trim(&self.dir).to_owned()
    }

    fn target(&self) -> Target {
        Target { key: self.key(), directory: self.dir.clone(), project: self.project.clone() }
    }
}

fn worktrees(s: &dyn SettingsHost, only: Option<&str>) -> Vec<Worktree> {
    inventory(s)
        .into_iter()
        .filter(|p| only.is_none_or(|id| p.id == id))
        .flat_map(|p| {
            let name = super::projects::name(p);
            s.project_worktrees(&p.id, &p.directory)
                .into_iter()
                .map(move |dir| Worktree { project: p.id.clone(), name: name.clone(), dir })
        })
        .filter(|w| !s.settings_pages().worktrees.delete.removed.contains(&w.key()))
        .collect()
}

fn sessions_in<'a>(s: &'a dyn SettingsHost, dir: &str) -> Vec<&'a SessionInfo> {
    let mut list: Vec<&SessionInfo> = s.sessions().iter().filter(|x| x.archived.is_none() && contains(dir, &x.directory)).collect();
    list.sort_by_key(|a| std::cmp::Reverse(a.updated));
    list
}

fn contains(root: &str, dir: &str) -> bool {
    let (root, dir) = (path::trim(root), path::trim(dir));
    dir == root || dir.strip_prefix(root).is_some_and(|rest| rest.starts_with(['/', '\\']))
}

pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    view_for(s, p, None)
}

fn inventory(s: &dyn SettingsHost) -> Vec<&Project> {
    let mut list: Vec<_> = s.projects().iter().collect();
    list.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.id.cmp(&b.id)));
    list
}

fn with_worktrees(s: &dyn SettingsHost) -> Vec<(String, String)> {
    inventory(s)
        .into_iter()
        .filter(|p| !s.project_worktrees(&p.id, &p.directory).is_empty())
        .map(|p| (p.id.clone(), super::projects::name(p)))
        .collect()
}

fn selection(s: &dyn SettingsHost, only: Option<&str>) -> Option<String> {
    if let Some(id) = only {
        return Some(id.to_owned());
    }
    match &s.settings_pages().worktrees.filter {
        Filter::Project(id) if with_worktrees(s).iter().any(|(p, _)| p == id) => Some(id.clone()),
        _ => None,
    }
}

/// Worktrees, of project `only` alone on its own pages (`SettingsWorkspaces projectID`).
pub fn view_for<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, only: Option<&str>) -> El {
    let selected = selection(s, only);
    let list = worktrees(s, selected.as_deref());
    let empty = list.is_empty();
    let active = s.active_session_directory();
    let n = list.len();
    let closing = |w: &Worktree| s.settings_pages().worktrees.delete.row(&w.key()).1;
    let rows: Vec<El> = list
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let divided = i + 1 < n && !(i + 2 == n && closing(&list[i + 1]));
            worktree_row(s, p, w, active.as_deref(), i + 1 < n, divided, selected.is_some())
        })
        .collect();
    // Keep the empty-state wrapper mounted so deleting the last worktree smoothly animates it
    // open over 150 ms ease-out while fading opacity over 120 ms (`settings.css:1967-1999`).
    let empty_content = col()
        .key("settings-workspaces-empty")
        .w_full()
        .gap(4.0)
        .child(text(t!("settings.workspaces.empty")).font(NAME).color(p.text).wrap())
        .child(text(t!("settings.workspaces.empty.description")).font(BODY).color(p.muted).wrap());
    let empty_motion = col()
        .key("settings-workspaces-empty-motion")
        .w_full()
        .clip_soft(0.0)
        .opacity(if empty { 1.0 } else { 0.0 })
        .opacity_motion(FADE)
        .child(empty_content);
    let empty_motion = if empty { empty_motion } else { empty_motion.h(0.0) };
    let pad = if crate::rows_wrap() { 14.0 } else { 16.0 };
    let inventory = col().key("settings-workspaces-inventory").w_full().child(empty_motion).children(rows);
    let inventory = if empty {
        inventory
    } else {
        inventory.padding(pad).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border))
    };
    let body = col()
        .w_full()
        .children((!empty).then(|| toolbar(s, p, &list, active, only.is_none().then_some(selected.as_deref()))))
        .child(inventory);
    let body = if s.narrow_sm() || !s.narrow() { body } else { col().w_full().px(12.0).child(body) };
    let switcher = if only.is_none() { super::servers::switcher(s, p) } else { None };
    page(p, "settings-workspaces-page", header_with(p, t!("settings.tab.workspaces"), t!("settings.workspaces.description"), switcher), body)
}

fn pick_bulk_delete(s: &mut dyn SettingsHost, picked: Option<&str>, unused: &[Target], all: &[Target], active: &Option<String>, tx: &mut Tx) {
    let (scope, targets) = match picked {
        Some("unused") => (Scope::Unused, unused.to_vec()),
        Some("all") => (Scope::All, all.to_vec()),
        _ => return,
    };
    tx.instant();
    dispatch(s, Event::ConfirmOpened { scope, active: active.clone(), targets });
}

fn toolbar<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, list: &[Worktree], active: Option<String>, filter: Option<Option<&str>>) -> El {
    let busy = s.settings_pages().worktrees.delete.busy();
    let all: Vec<Target> = list.iter().map(Worktree::target).collect();
    let unused: Vec<Target> = list.iter().filter(|w| sessions_in(s, &w.dir).is_empty()).map(Worktree::target).collect();
    let open = s.popup().open_at("settings-workspaces-more");
    let dots = row()
        .key("settings-workspaces-more")
        .role("button")
        .label_static(t!("common.moreOptions"))
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .focus_default(p)
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    let dots = if busy {
        dots.disabled(true).opacity(0.5)
    } else {
        let dots = if open { dots.fill(p.pressed) } else { dots.hover(p.hover) };
        dots.press(p.pressed).menu_button("settings-workspaces-more", move |s: &mut H, tx: &mut Tx| {
            let mut menu = Menu::new(t!("common.moreOptions")).align(Align::End).gap(4.0);
            if !unused.is_empty() {
                menu = menu.item(Item::new("unused", t!("settings.workspaces.deleteWithoutSessions"))).separator();
            }
            menu = menu.item(Item::new("all", t!("settings.workspaces.deleteAll")).danger(true)).hug();
            let (all, unused, active) = (all.clone(), unused.clone(), active.clone());
            popup::open(s, menu, "settings-workspaces-more", tx, move |s: &mut H, picked, tx| pick_bulk_delete(s, picked, &unused, &all, &active, tx));
        })
    };
    let count = text(opencode_i18n::plural!("settings.workspaces.count", list.len())).font(NAME).color(p.text);
    let filter_el = filter.and_then(|f| project_menu(s, p, f));
    if crate::rows_wrap() {
        let mut actions = row().key("settings-workspaces-toolbar-actions").w_full().gap(4.0).center_y();
        if let Some(f) = filter_el {
            actions = actions.justify_between().child(f).child(dots);
        } else {
            actions = actions.child(dots);
        }
        return col()
            .key("settings-workspaces-toolbar")
            .w_full()
            .pb(8.0)
            .gap(8.0)
            .items_start()
            .child(count)
            .child(actions);
    }
    row()
        .key("settings-workspaces-toolbar")
        .w_full()
        .pb(8.0)
        .gap(16.0)
        .center_y()
        .child(count.grow(1.0).shrink().min_w(0.0))
        .child(row().key("settings-workspaces-toolbar-actions").gap(4.0).center_y().children(filter_el).child(dots))
}

fn pick_project_filter(s: &mut dyn SettingsHost, picked: Option<&str>) {
    let Some(id) = picked else { return };
    s.settings_pages_mut().worktrees.filter = if id == ALL { Filter::All } else { Filter::Project(id.to_owned()) };
}

fn project_menu<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, selected: Option<&str>) -> Option<El> {
    let projects = with_worktrees(s);
    if projects.len() < 2 {
        return None;
    }
    let all = t!("settings.workspaces.filter.all").to_owned();
    let label = selected.and_then(|id| projects.iter().find(|(p, _)| p == id)).map_or(all.clone(), |(_, name)| name.clone());
    let current = selected.unwrap_or(ALL).to_owned();
    let open = s.popup().open_at(FILTER);
    let b = row()
        .key(FILTER)
        .role("button")
        .label(label.clone())
        .h(24.0)
        .px(9.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .max_w(192.0)
        .min_w(0.0)
        .focus_default(p)
        .child(text(label).font(rune::TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04)).color(p.muted).truncate().shrink().min_w(0.0))
        .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_muted));
    let b = if open { b.fill(p.pressed) } else { b.hover(p.hover) };
    Some(b.press(p.pressed).menu_button(FILTER, move |s: &mut H, tx: &mut Tx| {
        tx.instant();
        let items = std::iter::once((ALL.to_owned(), all.clone())).chain(projects.iter().cloned()).map(|(id, name)| Item::new(id.clone(), name).checked(id == current));
        let menu = Menu::new(t!("settings.tab.workspaces")).align(Align::End).gap(6.0).items(items).hug().min_width(160.0);
        popup::open(s, menu, FILTER, tx, |s: &mut H, picked, _| pick_project_filter(s, picked));
    }))
}

const FILTER: &str = "settings-workspaces-filter";
const ALL: &str = "workspaces-filter-all";

fn open_delete_one(s: &mut dyn SettingsHost, active: Option<String>, target: Target, tx: &mut Tx) {
    tx.instant();
    dispatch(s, Event::ConfirmOpened { scope: Scope::One, active, targets: vec![target] });
}

fn worktree_row<H: SettingsHost + 'static>(s: &H, p: &Pal, w: &Worktree, active: Option<&str>, followed: bool, divided: bool, filtered: bool) -> El {
    let linked = sessions_in(s, &w.dir);
    let machine = &s.settings_pages().worktrees.delete;
    let (deleting, collapsing) = machine.row(&w.key());
    let busy = machine.busy();
    let base = path::basename(&w.dir).to_owned();
    let parent = w.dir.strip_suffix(&base).unwrap_or_default().to_owned();
    let key = format!("settings-worktree-delete-{}", w.key());
    let confirm = t!("workspace.delete.confirm", name = base);
    let delete = row()
        .key(key)
        .role("button")
        .label(confirm)
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .focus_default(p)
        .child(icon(&ic::OUTLINE_TRASH, 16.0).color(p.icon_muted));
    let target = w.target();
    let active_dir = active.map(str::to_owned);
    let delete = if busy {
        delete.disabled(true).opacity(0.5)
    } else {
        delete.hover(p.hover).press(p.pressed).on_press_tx(move |s: &mut H, tx: &mut Tx| open_delete_one(s, active_dir.clone(), target.clone(), tx))
    };
    let meta = if filtered {
        row().child(text(opencode_i18n::plural!("settings.workspaces.sessions.filtered", linked.len())).font(BODY).color(p.faint))
    } else {
        let label = opencode_i18n::plural!("settings.workspaces.sessions", linked.len(), project = w.name);
        let (before, after) = label.rfind(&w.name).map_or((label.as_str(), ""), |at| (&label[..at], &label[at + w.name.len()..]));
        row()
            .gap(0.0)
            .children((!before.is_empty()).then(|| text(before.to_owned()).font(BODY).color(p.faint)))
            .children(label.rfind(&w.name).map(|_| text(w.name.clone()).font(BODY).color(p.muted)))
            .children((!after.is_empty()).then(|| text(after.to_owned()).font(BODY).color(p.faint)))
    };
    let compact = crate::rows_wrap();
    let folder = text(w.dir.clone())
        .font(BODY)
        .color(p.text)
        .spans(vec![rune::Span::new(parent.len(), w.dir.len()).weight(530.0)])
        .shrink()
        .min_w(0.0);
    let folder = if compact { folder.wrap_anywhere() } else { folder.truncate().help_when_truncated(w.dir.clone()).help_above() };
    let faint = |words: String| text(words).font(ACTIVE).color(p.faint);
    let now_ms = s.now_ms();
    let status = if compact {
        None
    } else if deleting {
        Some(faint(t!("workspace.lifecycle.deleting").to_owned()).key("settings-worktree-deleting"))
    } else {
        linked.first().filter(|x| x.updated > 0).map(|x| {
            faint(relative_time(now_ms, x.updated))
                .key("settings-worktree-active")
                .focusable(true)
                .focus_default(p)
                .help(t!("settings.workspaces.lastActiveSession").to_owned())
                .help_above()
        })
    };
    let mut r = col()
        .key(format!("settings-worktree-{}", w.key()))
        .w_full()
        .gap(12.0)
        .pb(if divided { 20.0 } else { 0.0 })
        .mb(if divided { 20.0 } else { 0.0 })
        .child(
            row()
                .w_full()
                .gap(20.0)
                .items_start()
                .child(col().grow(1.0).shrink().min_w(0.0).gap(8.0).child(row().w_full().min_w(0.0).child(folder)).child(meta))
                .child(row().gap(12.0).center_y().children(status).children((!deleting).then_some(delete))),
        );
    if !linked.is_empty() {
        r = r.child(sessions(now_ms, p, &linked));
    }
    let r = r.children(followed.then(|| {
        el().pin(Some(0.0), None, Some(0.0), Some(0.0))
            .h(0.5)
            .fill(p.border)
            .opacity(if divided { 1.0 } else { 0.0 })
            .opacity_motion(FADE)
    }));
    let motion = col()
        .key(format!("settings-worktree-motion-{}", w.key()))
        .w_full()
        .clip_soft(0.0)
        .opacity(if collapsing { 0.0 } else { 1.0 })
        .opacity_motion(FADE)
        .child(r);
    if collapsing { motion.h(0.0) } else { motion }
}

fn sessions(now_ms: i64, p: &Pal, linked: &[&SessionInfo]) -> El {
    let n = linked.len();
    let rows = linked.iter().enumerate().map(|(i, x)| {
        let time = (n > 1 && x.updated > 0).then(|| text(relative_time(now_ms, x.updated)).font(BODY).color(p.faint));
        let title = if x.title.is_empty() { t!("rune.session.untitled").to_owned() } else { x.title.clone() };
        row()
            .key(format!("settings-worktree-session-{}", x.id))
            .w_full()
            .py(10.0)
            .pl(12.0)
            .pr(16.0)
            .gap(12.0)
            .center_y()
            .border_sides([0.0, 0.0, if i + 1 < n { 0.5 } else { 0.0 }, 0.0], p.border)
            .border_layout()
            .child(text(title).font(BODY).color(p.text).truncate().grow(1.0).shrink().min_w(0.0))
            .children(time)
    });
    col()
        .key("settings-worktree-sessions")
        .w_full()
        .radius(6.0)
        .fill(p.bg_base)
        .border(0.5, p.border)
        .border_layout()
        .clip_soft(0.0)
        .children(rows)
}

/// Tells the deletes' machine `event` and runs what it asks for.
pub fn dispatch(s: &mut dyn SettingsHost, event: Event) {
    let was_confirming = s.settings_pages().worktrees.delete.confirming();
    let state = std::mem::take(&mut s.settings_pages_mut().worktrees.delete);
    let (next, effects) = delete::step(state, event);
    let is_confirming = next.confirming();
    s.settings_pages_mut().worktrees.delete = next;
    if !was_confirming && is_confirming {
        s.on_overlay_opened(WORKTREE_DELETE_OVERLAY_ID, None);
    } else if was_confirming && !is_confirming {
        s.on_overlay_closed(WORKTREE_DELETE_OVERLAY_ID);
    }
    for e in effects {
        run(s, e);
    }
}

/// Fires the machine's due timers; true when one fired.
pub fn poll(s: &mut dyn SettingsHost, tx: &mut Tx) -> bool {
    if !s.settings_pages().worktrees.delete.removed.is_empty() {
        let keys: Vec<String> = s
            .projects()
            .iter()
            .flat_map(|p| s.project_worktrees(&p.id, &p.directory))
            .map(|d| path::trim(&d).to_owned())
            .collect();
        if s.settings_pages().worktrees.delete.removed.iter().any(|k| !keys.contains(k)) {
            dispatch(s, Event::InventorySynced { keys });
        }
    }
    let now = s.now_ms();
    if s.settings_pages().worktrees.timers.is_empty() {
        return false;
    }
    let st = &mut s.settings_pages_mut().worktrees;
    let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut st.timers).into_iter().partition(|(at, _)| *at <= now);
    st.timers = later;
    if let Some(next) = st.timers.iter().map(|(at, _)| *at).min() {
        tx.wake_after(std::time::Duration::from_millis((next - now).max(0) as u64));
    }
    for (_, d) in &due {
        let event = match *d {
            Due::Inspect(attempt) => Event::InspectTimedOut { attempt },
            Due::Delete(attempt) => Event::DeleteTimedOut { attempt },
            Due::Collapse(attempt) => {
                // Animate the empty-state wrapper opening when the last collapsed row leaves.
                tx.motion_curve(COLLAPSE);
                Event::CollapseCompleted { attempt }
            }
        };
        dispatch(s, event);
    }
    !due.is_empty()
}

fn timer(s: &mut dyn SettingsHost, ms: i64, due: Due) {
    let at = s.now_ms() + ms;
    s.settings_pages_mut().worktrees.timers.push((at, due));
}

fn stop(s: &mut dyn SettingsHost, due: Due) {
    s.settings_pages_mut().worktrees.timers.retain(|(_, d)| *d != due);
}

fn run(s: &mut dyn SettingsHost, effect: Effect) {
    match effect {
        Effect::InspectRun { attempt, directory, active } => {
            timer(s, delete::INSPECT_TIMEOUT_MS, Due::Inspect(attempt));
            let linked = !sessions_in(s, &directory).is_empty();
            s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::InspectWorktree {
                attempt,
                directory,
                active,
                linked,
            }));
        }
        Effect::InspectStop { attempt } => stop(s, Due::Inspect(attempt)),
        Effect::DeleteRun { attempt, target, force, active } => {
            timer(s, delete::DELETE_TIMEOUT_MS, Due::Delete(attempt));
            let linked = !sessions_in(s, &target.directory).is_empty();
            s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::DeleteWorktree {
                attempt,
                target,
                force,
                active,
                linked,
            }));
        }
        Effect::DeleteTimerStop { attempt } => stop(s, Due::Delete(attempt)),
        Effect::CollapseStart { attempt } => timer(s, delete::COLLAPSE_MS, Due::Collapse(attempt)),
        Effect::Finalize { target } => s.list_worktrees(&target.project),
        Effect::ToastSkipped { reason } => {
            let why = match reason {
                Skip::Blocked(i) if i.active => t!("settings.workspaces.delete.blocked.active").to_owned(),
                Skip::Blocked(i) => messages(i).into_iter().next().unwrap_or_default(),
                Skip::Failed(Some(m)) => m,
                Skip::Failed(None) | Skip::TimedOut => t!("common.requestFailed").to_owned(),
            };
            s.toast_say(t!("workspace.delete.failed.title").into(), why);
        }
    }
}

/// What a check found that stops a delete (`inspectionMessages`): linked sessions, then local changes.
pub(super) fn messages(i: Inspection) -> Vec<String> {
    let mut out = Vec::new();
    if i.linked {
        out.push(t!("settings.workspaces.delete.blocked.linked").to_owned());
    }
    if i.dirty {
        out.push(t!("workspace.status.dirty").to_owned());
    }
    out
}

/// A worktree's check (`inspect`): working changes, branch changes, active folder, linked sessions.
pub(crate) fn inspect_on_server(srv: &Server, directory: &str, active: Option<&str>, linked: bool) -> Option<Inspection> {
    let loc = format!("location%5Bdirectory%5D={}", percent_encode(directory));
    let listed = |v: Value| v.get("data").unwrap_or(&v).as_array().is_some_and(|a| !a.is_empty());
    let working = listed(srv.get(&format!("/api/vcs/status?{loc}")).ok()?);
    let branch = listed(srv.get(&format!("/api/vcs/diff?{loc}&mode=branch")).ok()?);
    Some(Inspection { active: active.is_some_and(|a| contains(directory, a)), linked, dirty: working || branch })
}

/// `getRelativeTime` (`shell/time.ts`): "Just now", "5m ago", "3h ago", "2d ago", then local date.
pub fn relative_time(now: i64, then: i64) -> String {
    let s = (now - then).div_euclid(1000);
    let m = s.div_euclid(60);
    let h = s.div_euclid(3600);
    let d = s.div_euclid(86_400);
    match () {
        _ if s < 60 => t!("common.time.justNow").into(),
        _ if m < 60 => t!("common.time.minutesAgo.short", count = m),
        _ if h < 24 => t!("common.time.hoursAgo.short", count = h),
        _ if d < 7 => t!("common.time.daysAgo.short", count = d),
        _ => {
            let (at, today) = (rune::LocalTime::at(then), rune::LocalTime::at(now));
            let style = if at.year == today.year { opencode_i18n::date::Style::MonthDay } else { opencode_i18n::date::Style::MonthDayYear };
            opencode_i18n::date::format(style, opencode_ui::style::moment(at))
        }
    }
}
