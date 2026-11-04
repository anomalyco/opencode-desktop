use super::narrow::{self, HeaderDrop, TITLE_FLOOR, TITLE_MIN, text_w};
use super::*;
use crate::session_menu::{Spot, Target};

const SLASH: TextStyle = TextStyle::new(11.0, 500.0);
const NUMBERS: TextStyle = UI_SMALL.features("\"tnum\"");

pub(super) fn header(s: &App, p: &Pal, info: Option<&SessionInfo>, t: Option<&Transcript>) -> El {
    header_in(s, p, info, t, "")
}

/// The session header, keyed `{scope}-header` when `scope` isn't empty (copies on one stage).
/// In a narrow window it drops its lower items, in `HeaderDrop`'s order, until the title has
/// `TITLE_FLOOR` (p263); the title takes what's left, fading at its edge.
pub(crate) fn header_in(s: &App, p: &Pal, info: Option<&SessionInfo>, t: Option<&Transcript>, scope: &str) -> El {
    // Context and spend: the last finished step's context window, the session's cost.
    let last = t.and_then(session_view::usage::context);
    let ctx = last.unwrap_or(0);
    // The session's own total (it includes subagents); the transcript's sum when it's ahead.
    let summed = t.map_or(0.0, session_view::usage::spent);
    let cost = info.map_or(0.0, |i| i.cost).max(summed);
    let (tokens, spend) = session_view::usage::labels(ctx, cost);
    let shows_usage = t.is_some() && last.is_some();
    let reserve_side = !s.right_column_open() && s.stage.is_none();

    let project = info.and_then(|i| s.project(&i.project));
    let proj = project.map_or("", |x| x.name.as_str());
    let parent = info.and_then(|i| i.parent.as_deref()).and_then(|pid| s.session(pid));
    let (k, titled) = drops(s, info, parent, proj, shows_usage, reserve_side, [&tokens, &spend], scope);
    let keep = |d: HeaderDrop| d as usize >= k;

    let mut crumbs = row().key("crumbs").gap(0.0).center_y().shrink().min_w(0.0);
    if let Some(info) = info {
        if keep(HeaderDrop::Badge) {
            crumbs = crumbs.child(crate::icons::badge(s, p, project, proj).mr(8.0));
        }
        if keep(HeaderDrop::Name) {
            crumbs = crumbs
                .child(text(proj.to_owned()).font(UI_MED).color(p.faint))
                .child(text("/").font(SLASH).color(p.faint).pl(8.0).pr(4.0));
        }
        if let Some(parent) = parent.filter(|_| keep(HeaderDrop::Parent)) {
            let pid = parent.id.clone();
            crumbs = crumbs
                .child(
                    row().key("parent-crumb").role("button").label(format!("Open {}", title_of(parent))).h(24.0).px(4.0).center_y().radius(4.0).shrink().min_w(0.0)
                        .hover(p.hover).hover_motion(INSTANT, INSTANT)
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                            tx.instant();
                            s.open(&pid, tx);
                        })
                        .child(text(title_of(parent)).font(UI_MED).color(p.faint).truncate_fade()),
                )
                .child(text("/").font(SLASH).color(p.faint).pl(4.0).pr(4.0));
        }
        if titled {
            crumbs = crumbs.children(header_title(s, p, info, keep(HeaderDrop::More)));
        }
        // A running subagent's header spins beside its title (`SessionWorkingIndicator`).
        if info.parent.is_some() && s.subtree_running(&info.id) {
            crumbs = crumbs.child(activity_ring(p).key("working").ml(6.0));
        }
    }
    // The session model's context window, or 200k when the server doesn't say.
    let limit = info
        .and_then(|i| s.pickers.limits.get(&format!("{}:{}", i.model.provider, i.model.id)))
        .map_or(200_000.0, |&n| n.max(1) as f32);
    // Each session's numbers are their own odometers, so opening another one shows its numbers
    // at once rather than rolling from this one's.
    let sid = info.map(|i| i.id.as_str()).or(t.map(|t| t.session.as_str())).unwrap_or("");
    let usage = session_view::usage::row_for(p, sid, ctx, cost, limit, keep(HeaderDrop::Ring), keep(HeaderDrop::Tokens));
    row()
        .key(scoped(scope, "header"))
        .h(48.0)
        .pl(14.0)
        .pr(14.0)
        .gap(if reserve_side { 8.0 } else { 0.0 })
        .center_y()
        .window_drag()
        .motion(crate::panels::view::SIDE_SPRING)
        .child(collapsed_slot(s))
        .child(crumbs.mr(if reserve_side { 0.0 } else { 8.0 }))
        .child(spacer())
        .children(crate::find::bar(s, p).map(|bar| bar.shrink().min_w(0.0)))
        .children(shows_usage.then_some(usage))
        .children(s.stage.is_none().then(|| {
            el().key("header-side-slot")
                .w(if reserve_side { 30.0 } else { 0.0 })
                .h(28.0)
                .motion(crate::panels::view::SIDE_SPRING)
        }))
}

/// How many of `HEADER_DROPS` the header drops to fit the width it had at the last layout, and
/// whether the title still has `TITLE_MIN` (a sliver of it would read as a glitch).
#[allow(clippy::too_many_arguments)]
fn drops(s: &App, info: Option<&SessionInfo>, parent: Option<&SessionInfo>, proj: &str, usage: bool, reserve_side: bool, [tokens, spend]: [&str; 2], scope: &str) -> (usize, bool) {
    let Some(info) = info else { return (0, true) };
    let room = rune::memo::layout_with(&scoped(scope, "header"), |r| r[2] - 28.0);
    let slash = text_w("/", SLASH);
    let root = info.parent.is_none();
    let name = text_w(proj, UI_MED) + 12.0 + slash;
    let parent = parent.map_or(0.0, |p| 8.0 + text_w(&title_of(p), UI_MED) + 8.0 + slash);
    let whole = text_w(&title_of(info), UI_MED);
    let title = 8.0 + whole.min(TITLE_FLOOR);
    let working = if !root && s.subtree_running(&info.id) { 20.0 } else { 0.0 };
    let more = if root { 28.0 } else { 0.0 };
    // What never goes: the sidebar toggle's room, the side slot, the find bar, the gaps and the cost.
    let children = 3 + usize::from(s.find.open) + usize::from(usage) + usize::from(s.stage.is_none());
    let gaps = if reserve_side { 8.0 * (children - 1) as f32 } else { 8.0 };
    let toggle = if s.sidebar_open { 0.0 } else { COLLAPSED_SLOT - 4.0 };
    let side = if reserve_side { 30.0 } else { 0.0 };
    let find = if s.find.open { 200.0 } else { 0.0 };
    let fixed = toggle + side + find + gaps + title + working + if usage { text_w(spend, NUMBERS) } else { 0.0 };
    let (tokens, ring) = if usage { (6.0 + text_w(tokens, NUMBERS), 22.0) } else { (0.0, 0.0) };
    let widths = [name, tokens, ring, parent, more, 24.0];
    let need = |k: usize| fixed + widths[k..].iter().sum::<f32>();
    let k = narrow::steps(room, widths.len(), need);
    let left = room.map_or(f32::INFINITY, |room| room - (need(k) - title) - 8.0);
    (k, left >= TITLE_MIN.min(whole))
}

/// `key`, or `{scope}-{key}` for a copy on a stage.
pub(super) fn scoped(scope: &str, key: &str) -> String {
    if scope.is_empty() { key.to_owned() } else { format!("{scope}-{key}") }
}

/// The header's session title. A root session's has Desktop's menu (a right-click, or the ⋯
/// button beside it when `more`) and renames when clicked; a subagent's is plain.
fn header_title(s: &App, p: &Pal, info: &SessionInfo, more: bool) -> Vec<El> {
    if info.parent.is_some() {
        return vec![text(title_of(info)).font(UI_MED).color(p.text).truncate_fade().pl(4.0)];
    }
    let target = Target::new(&info.id, Spot::Header);
    let marked = s.menus.on.as_ref() == Some(&target);
    let dots_target = target.clone();
    let title = match crate::session_menu_view::field(s, p, &target, UI_MED) {
        Some(field) => row().key("title").h(28.0).px(4.0).center_y().min_w(0.0).shrink().child(field),
        None => {
            let begin = target.clone();
            let mut t = row().key("title").role("button").focusable(true).label(title_of(info)).h(28.0).px(4.0).center_y().min_w(0.0).shrink().radius(6.0).hover(p.hover).hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.begin_rename(&begin);
                })
                .child(crate::title_change::title(&s.knobs, &format!("title-{}", info.id), title_of(info), UI_MED, p.text));
            if marked {
                t = t.fill(p.hover);
            }
            crate::session_menu::on(t, target)
        }
    };
    if !more {
        return vec![title];
    }
    let dots = el().key("header-more").role("button").label("More options").size(28.0, 28.0).radius(6.0).center().hover(p.hover).hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| crate::session_menu::open(s, dots_target.clone(), "header-more", tx))
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    vec![title, dots]
}
