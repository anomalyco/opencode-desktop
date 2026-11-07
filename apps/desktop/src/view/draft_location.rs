//! Where a new session starts, under the composer: Desktop's `NewSessionLocation`
//! (`new-session/view.tsx:274-463`). The project, which opens the project picker; for a Git
//! project, a slash and where it runs (`Local`, or the worktree), which opens its worktrees
//! with a dot until a worktree has been picked once; the branch; and the folder, faint at the
//! end, cut in the middle on a separator when it doesn't fit.

use std::cell::Cell;

use opencode_kit::model::project_git;
use opencode_kit::path;

use super::*;

/// The worktree onboarding's flag (Desktop's `workspace-onboarding`): once a worktree is
/// picked, the dot after `Local` goes for good.
const ONBOARDING: &str = "workspace-onboarding.json";

thread_local! {
    static USED: Cell<Option<bool>> = const { Cell::new(None) };
}

/// The row (`view.tsx:321`): at least 36 tall, 8 in and 6 from the end, 12 between the
/// pickers and the folder. `width` is the panel's: at 560 or less the folder goes
/// (`@max-[560px]:hidden`).
pub(in crate::view) fn location(s: &App, p: &Pal, width: f32) -> El {
    let dir = s.draft_dir();
    let project = s.draft_project();
    let name = project_or_dir(project, dir);
    // Desktop's `TRIGGER`: 28 tall, 6 in and 8 from the end, 4 pt corners, muted words that
    // turn base under the pointer with the hover overlay. It opens the location dialog on its
    // projects (`view.tsx:127`).
    let button = trigger("project", "Choose project")
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.open_project_switcher();
        })
        .child(crate::icons::badge(s, p, project, &name))
        .child(text(name.clone()).font(UI).color(p.muted).group_color(p.text).truncate_fade());
    let branch = s.sidebar_inbox.branches.get(dir);
    let creating = s.draft_creates();
    // Worktrees are a Git project's (`view.tsx:128`).
    let place = project.filter(|p| project_git(&p.id) || branch.is_some()).map(|project| {
        let worktree = s.project_worktrees(&project.id, &project.directory).iter().any(|w| path::trim(w) == path::trim(dir));
        let (glyph, size, label) = match (creating, worktree) {
            (true, _) => (&ic::PLUS, 14.0, "New worktree".to_owned()),
            (false, true) => (&ic::OUTLINE_WORKTREE, 16.0, path::basename(dir).to_owned()),
            (false, false) => (&ic::MONITOR, 14.0, "Local".to_owned()),
        };
        let id = project.id.clone();
        // The onboarding's own large tooltip isn't drawn here yet; after it, Desktop's plain one.
        let button = trigger("worktree", "Choose worktree");
        let button = if used() { button.help(crate::keymap::Bind::LocationSelect.hint("Choose worktree")) } else { button };
        button
            .hover(p.hover)
            .press(p.pressed)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.open_project_switcher();
                s.drill_project_worktrees(&id, tx);
            })
            .child(icon(glyph, size).color(p.icon_muted))
            .child(text(label).font(UI).color(p.muted).group_color(p.text).truncate_fade())
            .children((!used()).then(|| el().key("onboarding-dot").size(6.0, 6.0).radius(3.0).fill(p.accent)))
    });
    // A new worktree's branch is the one it starts from, and can be picked (`selector.tsx`).
    let branch = branch.map(|b| if creating { crate::new_worktree::trigger(s, p, b) } else { branch_label(p, b) });
    let pickers = row()
        .key("location-pickers")
        .shrink()
        .min_w(0.0)
        .py(6.0)
        .center_y()
        .child(button)
        .children(place.map(|place| row().key("location-place").shrink().min_w(48.0).center_y().child(session_view::composer::slash(p, 2.0)).child(place)))
        .children(branch);
    // A worktree still to be made has no folder yet (`view.tsx:312-316`).
    row()
        .key("location")
        .w_full()
        .min_h(36.0)
        .pl(8.0)
        .pr(6.0)
        .gap(12.0)
        .center_y()
        .child(pickers)
        .children((width > 560.0 && !creating).then(|| folder(p, dir)))
}

/// A picker in the row, before its look: Desktop's `TRIGGER` (`view.tsx:274-275`).
fn trigger(key: &str, label: &str) -> El {
    row().key(key).role("button").label(label.to_owned()).hover_group().h(28.0).pl(6.0).pr(8.0).gap(6.0).center_y().radius(4.0).shrink().min_w(0.0)
}

/// The branch the session starts on (`workspace/selector.tsx:151-156`): faint and still, as it
/// is for the project's own folder.
fn branch_label(p: &Pal, branch: &str) -> El {
    row()
        .key("location-branch")
        .h(28.0)
        .max_w(220.0)
        .shrink()
        .min_w(0.0)
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .child(icon(&ic::BRANCH, 16.0).color(p.icon_muted))
        .child(text(branch.to_owned()).font(UI).color(p.faint).truncate_fade())
}

/// The folder at the row's end, as the server names it (`view.tsx:420-460`): it keeps at
/// least half the row or its shortest cut, and shows as much of its start as fits.
fn folder(p: &Pal, dir: &str) -> El {
    let room = rune::memo::layout_with("location-path", |r| r[2] - 6.0).unwrap_or(0.0);
    let row_w = rune::memo::layout_with("location", |r| r[2]).unwrap_or(f32::INFINITY);
    let (shown, minimum) = fit_path(dir, room.max(0.0));
    row()
        .key("location-path")
        .grow(1.0)
        .shrink()
        .min_w((0.5 * row_w).min(minimum.ceil() + 6.0))
        .pr(6.0)
        .justify_end()
        .center_y()
        .child(text(shown).font(UI).color(p.faint).truncate_head())
}

/// Desktop's `fitPath` (`view.tsx:251-272`): as many leading segments as fit, then `…`, then
/// the last two; the cut always lands on a separator. Also the shortest cut's width.
fn fit_path(value: &str, width: f32) -> (String, f32) {
    let sep = if value.contains('/') { "/" } else { "\\" };
    let parts: Vec<&str> = value.split(sep).collect();
    let w = |t: &str| super::narrow::text_w(t, UI);
    let first = if parts.first() == Some(&"") { 2 } else { 1 };
    let n = parts.len();
    let candidates: Vec<String> = (0..n.saturating_sub(2 + first))
        .map(|i| parts[..n - 3 - i].iter().copied().chain(["…"]).chain(parts[n - 2..].iter().copied()).collect::<Vec<_>>().join(sep))
        .collect();
    let minimum = candidates.last().cloned().unwrap_or_else(|| value.to_owned());
    let fits = |t: &str| w(t) <= width;
    let shown = if width <= 0.0 || fits(value) { value.to_owned() } else { candidates.iter().find(|c| fits(c)).cloned().unwrap_or_else(|| minimum.clone()) };
    (shown, w(&minimum))
}

/// Whether a worktree has been picked once, read from its file the first time it's asked.
fn used() -> bool {
    USED.get().unwrap_or_else(|| {
        let used = crate::dirs::data()
            .and_then(|d| std::fs::read_to_string(d.join(ONBOARDING)).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|v| v["used"].as_bool() == Some(true));
        USED.set(Some(used));
        used
    })
}

/// A new session will run in `dir`: a worktree of its project ends the onboarding
/// (`view.tsx:84-90`).
pub(crate) fn picked(s: &App, dir: &str) {
    let Some(project) = s.project_at(dir) else { return };
    if !s.project_worktrees(&project.id, &project.directory).iter().any(|w| path::trim(w) == path::trim(dir)) {
        return;
    }
    mark_used(s);
}

/// A worktree, new or not, was picked: the onboarding's dot goes for good.
pub(crate) fn mark_used(s: &App) {
    if used() {
        return;
    }
    USED.set(Some(true));
    if let (true, Some(file)) = (s.persists(), crate::dirs::data().map(|d| d.join(ONBOARDING))) {
        crate::disk::later("workspace-onboarding", move || {
            crate::disk::replace(&file, br#"{"used":true}"#);
        });
    }
}
