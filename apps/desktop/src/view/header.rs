use std::cell::Cell;

use super::narrow::{self, HeaderDrop, TITLE_FLOOR, TITLE_MIN, text_w};
use super::*;
use crate::session_menu::{Spot, Target};

/// A crumb's `/` (`Slash`): the neighbouring 13 pt text's size in the regular weight.
const SLASH: TextStyle = TextStyle::new(13.0, 400.0).lh(16.0);
/// A crumb title (`text-[13px] font-[530] leading-4 tracking-[-0.04px]`).
const CRUMB: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// What the header's actions are thought to give back when they draw compact.
const DETAILS_W: f32 = 40.0;

/// The key of what plugins' header popovers hang from (Desktop's `Popover.Anchor`, 12 from the
/// header's end, 10 down, 28 tall): `session-header-anchor`.
pub(crate) const ANCHOR: &str = "session-header-anchor";

thread_local! {
    /// Whether the header's actions drew compact at the last layout, so the fit doesn't flip back
    /// the frame after they shrank.
    static COMPACT: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn header(s: &App, p: &Pal, info: Option<&SessionInfo>, t: Option<&Transcript>) -> El {
    header_in(s, p, info, t, "")
}

/// The session header (`SessionIdentityHeader`), keyed `{scope}-header` when `scope` isn't empty
/// (copies on one stage): the location, a subagent's ancestors, the title, then the actions
/// (the Find bar and plugins' `session.header` slots: the context ring, Session details). In a
/// narrow window it drops its lower items, in `HeaderDrop`'s order, until the title has
/// `TITLE_FLOOR` (p263); the title takes what's left, fading at its edge.
pub(crate) fn header_in(s: &App, p: &Pal, info: Option<&SessionInfo>, _t: Option<&Transcript>, scope: &str) -> El {
    let reserve_side = !s.right_column_open() && s.stage.is_none();
    let project = info.and_then(|i| s.project(&i.project));
    let proj = info.map(|i| super::project_or_dir(project, &i.directory)).unwrap_or_default();
    let parent = info.and_then(|i| i.parent.as_deref()).and_then(|pid| s.session(pid));
    let (k, titled) = drops(s, info, parent, &proj, reserve_side, scope);
    let keep = |d: HeaderDrop| d as usize >= k;
    let compact = !keep(HeaderDrop::Tokens);
    if scope.is_empty() {
        COMPACT.set(compact);
    }

    let mut crumbs = row().key("crumbs").gap(0.0).center_y().shrink().min_w(0.0);
    if let Some(info) = info {
        if keep(HeaderDrop::Badge) {
            crumbs = crumbs.child(super::location::trigger(s, p, project, &proj, keep(HeaderDrop::Name)));
            // The separator after the location: 8 of space each side, less the trigger's 6 and the
            // title's 4 padding (`SessionHeaderSeparator gap 8 before 6 after 4`), and the glyph's
            // own side bearing, which Desktop's `Slash` cancels.
            crumbs = crumbs.child(slash(p, 0.5, 3.5));
        }
        if let Some(parent) = parent.filter(|_| keep(HeaderDrop::Parent)) {
            let pid = parent.id.clone();
            crumbs = crumbs
                .child(
                    row()
                        .key("parent-crumb")
                        .role("button")
                        .label(format!("Open {}", title_of(parent)))
                        .help(title_of(parent))
                        .pl(4.0)
                        .max_w(204.0)
                        .shrink()
                        .min_w(0.0)
                        .center_y()
                        .hover_group()
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                            tx.instant();
                            s.open(&pid, tx);
                        })
                        .child(text(title_of(parent)).font(CRUMB).color(p.faint).group_color(p.muted).truncate().shrink()),
                )
                .child(slash(p, 6.5, 3.5));
        }
        if titled {
            crumbs = crumbs.children(header_title(s, p, info, keep(HeaderDrop::More)));
        }
        // A running subagent's header spins beside its title (`SessionWorkingIndicator`).
        if info.parent.is_some() && s.subtree_running(&info.id) {
            crumbs = crumbs.child(activity_ring(p).key("working").ml(6.0));
        }
    }
    let sid = info.map(|i| i.id.as_str()).unwrap_or("");
    // The actions: the Find bar and the plugins' `session.header` slots, 8 apart, 6 after the
    // title (`.session-header-actions`).
    let flags = opencode_plugin::abi::RENDER_ACTIVE | if compact { opencode_plugin::abi::RENDER_COMPACT } else { 0 };
    let slots = if info.is_some() { crate::plugin_host::slot_with(s, p, "session.header", sid, flags) } else { Vec::new() };
    let actions = row()
        .key("header-actions")
        .ml(6.0)
        .gap(8.0)
        .center_y()
        .children(crate::find::bar(s, p).map(|bar| bar.shrink().min_w(0.0)))
        .children(slots)
        .children(s.stage.is_none().then(|| {
            el().key("header-side-slot")
                .w(if reserve_side { 28.0 } else { 0.0 })
                .h(28.0)
                .motion(crate::panels::view::SIDE_SPRING)
        }));
    row()
        .key(scoped(scope, "header"))
        .h(48.0)
        .pl(10.0)
        .pr(12.0)
        .center_y()
        .window_drag()
        .motion(crate::panels::view::SIDE_SPRING)
        .child(collapsed_slot(s))
        .child(crumbs)
        .child(spacer())
        .child(actions)
        .children(scope.is_empty().then(|| el().key(ANCHOR).size(0.0, 28.0).pin(None, Some(10.0), Some(12.0), None)))
}

/// `SessionHeaderSeparator`: a 13 pt `/` in `icon-faint`, `before` and `after` points of margin.
fn slash(p: &Pal, before: f32, after: f32) -> El {
    text("/").font(SLASH).color(p.icon_faint).ml(before).mr(after)
}

/// How many of `HEADER_DROPS` the header drops to fit the width it had at the last layout, and
/// whether the title still has `TITLE_MIN` (a sliver of it would read as a glitch). The first
/// two steps draw the actions compact rather than dropping them.
fn drops(s: &App, info: Option<&SessionInfo>, parent: Option<&SessionInfo>, proj: &str, reserve_side: bool, scope: &str) -> (usize, bool) {
    let Some(info) = info else { return (0, true) };
    let room = rune::memo::layout_with(&scoped(scope, "header"), |r| r[2] - 22.0);
    let actions = rune::memo::layout_with("header-actions", |r| r[2]).unwrap_or(0.0) + 6.0;
    // What the actions take drawn in full: what they measured, plus what compact gave back.
    let actions = actions + if COMPACT.get() { DETAILS_W } else { 0.0 };
    let slash = text_w("/", SLASH) + 5.0;
    let root = info.parent.is_none();
    let name = text_w(proj, TextStyle::new(13.0, 400.0)) + 8.0;
    let parent = parent.map_or(0.0, |p| 4.0 + text_w(&title_of(p), CRUMB).min(200.0) + 11.0 + slash);
    let whole = text_w(&title_of(info), CRUMB) + 8.0;
    let title = whole.min(TITLE_FLOOR);
    let working = if !root && s.subtree_running(&info.id) { 20.0 } else { 0.0 };
    let more = if root { 28.0 } else { 0.0 };
    let toggle = chrome().toggle_space(s.sidebar_open);
    let side = if reserve_side { 0.0 } else { -28.0 };
    let fixed = toggle + side + actions - DETAILS_W + title + working + 28.0 + slash;
    let widths = [name, DETAILS_W, parent, more, 0.0];
    let need = |k: usize| fixed + widths[k..].iter().sum::<f32>();
    let k = narrow::steps(room, widths.len(), need);
    let left = room.map_or(f32::INFINITY, |room| room - (need(k) - title));
    (k, left >= TITLE_MIN.min(whole))
}

/// `key`, or `{scope}-{key}` for a copy on a stage.
pub(super) fn scoped(scope: &str, key: &str) -> String {
    if scope.is_empty() { key.to_owned() } else { format!("{scope}-{key}") }
}

/// The header's session title (`h1`: 13/530, 4 by 6 padding). A root session's has Desktop's
/// menu (a right-click, or the ⋯ button beside it when `more`) and renames when clicked; a
/// subagent's is plain.
fn header_title(s: &App, p: &Pal, info: &SessionInfo, more: bool) -> Vec<El> {
    if info.parent.is_some() {
        return vec![text(title_of(info)).key("title").font(CRUMB).color(p.text).truncate_fade().px(4.0).py(6.0).shrink().min_w(0.0)];
    }
    let target = Target::new(&info.id, Spot::Header);
    let marked = s.menus.on.as_ref() == Some(&target);
    let dots_target = target.clone();
    let title = match crate::session_menu_view::field(s, p, &target, CRUMB) {
        Some(field) => row().key("title").h(28.0).px(4.0).center_y().min_w(0.0).shrink().child(field),
        None => {
            let begin = target.clone();
            let mut t = row().key("title").role("button").focusable(true).label(title_of(info)).h(28.0).px(4.0).center_y().min_w(0.0).shrink().radius(6.0).hover(p.hover).hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.begin_rename(&begin);
                })
                .child(crate::title_change::title(&s.knobs, &format!("title-{}", info.id), title_of(info), CRUMB, p.text));
            if marked {
                t = t.fill(p.hover);
            }
            crate::session_menu::on(t, target)
        }
    };
    if !more {
        return vec![title];
    }
    let dots = el().key("header-more").role("button").label("More options").size(28.0, 28.0).ml(2.0).radius(6.0).center().hover(p.hover).hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| crate::session_menu::open(s, dots_target.clone(), "header-more", tx))
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    vec![title, dots]
}
