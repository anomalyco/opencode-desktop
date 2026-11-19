use crate::popup::MenuButton as _;
use std::cell::Cell;

use super::header_fit::{HeaderFit, Measured};
use super::narrow::text_w;
use super::*;
use crate::session_menu::{Spot, Target};
use opencode_i18n::t;

/// A crumb's `/` (`Slash`): the neighbouring 13 pt text's size. Its `font-weight:
/// var(--font-weight-normal)` names a variable Desktop never sets, so it inherits the page's 440.
const SLASH: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);
/// A crumb title (`text-[13px] font-[530] leading-4 tracking-[-0.04px]`).
const CRUMB: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// The title section's gap (`.session-header-title`).
const TITLE_GAP: f32 = 2.0;

/// The key of what plugins' header popovers hang from (Desktop's `Popover.Anchor`, 12 from the
/// header's end, 10 down, 28 tall): `session-header-anchor`.
pub(crate) const ANCHOR: &str = "session-header-anchor";

thread_local! {
    /// The routed header's fit at the last layout (`createSessionHeaderFit`).
    static FIT: Cell<HeaderFit> = const { Cell::new(HeaderFit::Full) };
}

pub(super) fn header(s: &App, p: &Pal, info: Option<&SessionInfo>, t: Option<&Transcript>) -> El {
    let inputs = (
        (p.theme, p.generation, p.dark),
        opencode_i18n::locale().id(),
        info.map(|i| (i.id.clone(), i.title.clone(), i.directory.clone(), i.project.clone(), i.parent.clone(), crate::plugin_host::services::session_version(s, &i.id))),
        crate::plugin_host::surfaces_version(s),
    );
    rune::region_with("session-header", inputs, || header_in(s, p, info, t, ""))
}

/// The session header (`SessionIdentityHeader`, `SessionHeaderLayout`), keyed `{scope}-header`
/// when `scope` isn't empty (copies on one stage): the location, a subagent's parent, the title
/// section, then the actions (the Find bar and plugins' `session.header` slots: the context ring,
/// Session details). In a narrow window the location's worktree name hides, then its project
/// name, then the actions drop their details; only then does the title truncate.
pub(crate) fn header_in(s: &App, p: &Pal, info: Option<&SessionInfo>, _t: Option<&Transcript>, scope: &str) -> El {
    let reserve_side = !s.side_open() && s.fixture_run.stage.is_none();
    let project = info.and_then(|i| s.project(&i.project));
    let proj = info.map(|i| super::project_or_dir(project, &i.directory)).unwrap_or_default();
    let parent = info.and_then(|i| i.parent.as_deref()).and_then(|pid| s.session(pid));
    let worktree = info.filter(|i| i.parent.is_none()).and_then(|i| super::location::worktree_name(project, &i.directory));
    let fit = fit(s, info, &proj, worktree.as_deref(), scope);
    let compact = fit.compact();

    let mut location = row().key("header-location").gap(0.0).center_y();
    let mut title_box = row().key("header-title").gap(TITLE_GAP).center_y().grow(1.0).min_w(0.0);
    if let Some(info) = info {
        location = location.child(super::location::trigger(s, p, project, &proj, worktree.as_deref(), fit.collapse()));
        // The separator after the location: 8 of space each side, less the trigger's 6 and the
        // title's 4 padding and the text's 0.5 side bearing (`SessionHeaderSeparator gap 8 before 6
        // after 4`, `session-identity-header.css:114-121`), its glyph pulled in 0.25 and 0.34 at
        // its sides (`slash.css`): 1.25 before the glyph's box, 3.16 after.
        location = location.child(slash(p, 1.25, 3.16));
        if let Some(parent) = parent {
            let pid = parent.id.clone();
            // Beside a subagent's ancestors the location takes at most 60% and gives way first.
            location = location
                .shrink()
                .min_w(0.0)
                .child(
                    row()
                        .key("parent-crumb")
                        .role("button")
                        .label(t!("rune.session.header.openParent", title = title_of(parent)))
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
                        .child(text(title_of(parent)).font(CRUMB).color(p.faint).group_color(p.muted).truncate().shrink().dir(rune::Direction::Auto)),
                )
                // The parent's own 6 of end padding, which its crumb here leaves out, then as above.
                .child(slash(p, 7.25, 3.16));
        }
        // A running subagent's header spins beside its title (`SessionWorkingIndicator`): the
        // 16 px session progress indicator in a 16 px box, 4 after the crumbs and 2 before the
        // title on top of the gap (`ms-1 me-0.5 h-4 w-4`).
        if info.parent.is_some() && s.subtree_running(&info.id) {
            title_box = title_box.child(row().key("working").size(16.0, 16.0).ml(4.0).mr(2.0).center().child(activity_ring_sized(p, 16.0)));
        }
        title_box = title_box.children(header_title(s, p, info));
    }
    let crumbs = row().key("crumbs").gap(0.0).center_y().grow(1.0).min_w(0.0).child(location).child(title_box);
    let sid = info.map(|i| i.id.as_str()).unwrap_or("");
    // The actions: the Find bar and the plugins' `session.header` slots, 8 apart, 6 after the
    // title (`.session-header-actions`).
    let slots = if info.is_some() { crate::plugin_host::slot_with(s, p, "session.header", sid, true, compact) } else { Vec::new() };
    let actions = row()
        .key("header-actions")
        .ml(6.0)
        .gap(8.0)
        .center_y()
        .children(crate::session_state::status(s, p))
        .children(crate::find::bar(s, p).map(|bar| bar.shrink().min_w(0.0)))
        .children(slots)
        // Pulled back over its gap, so with the side region open it takes no room at all: the
        // actions end 12 from the header's end, as Desktop's do.
        .children(s.fixture_run.stage.is_none().then(|| {
            el().key("header-side-slot")
                .ml(-8.0)
                .w(if reserve_side { 36.0 } else { 0.0 })
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
        .child(actions)
        .children(scope.is_empty().then(|| el().key(ANCHOR).size(0.0, 28.0).pin(None, Some(10.0), Some(12.0), None)))
}

/// `SessionHeaderSeparator`: a 13 pt `/` in `icon-faint`, `before` and `after` points of margin.
fn slash(p: &Pal, before: f32, after: f32) -> El {
    text("/").font(SLASH).color(p.icon_faint).ml(before).mr(after)
}

/// The header's fit for the width its pieces had at the last layout (`fit.ts`): the routed
/// header keeps its machine's state from one layout to the next; a copy on a stage starts full.
fn fit(s: &App, info: Option<&SessionInfo>, proj: &str, worktree: Option<&str>, scope: &str) -> HeaderFit {
    let Some(info) = info else { return HeaderFit::Full };
    let width = |key: &str| rune::memo::layout_with(&scoped(scope, key), |r| r[2]);
    // The location and the actions are as wide as what they show, and a root's limit is none, so
    // a resize moves none of these; only the title's box grows and shrinks with the window.
    let Some(location) = width("header-location") else { return HeaderFit::Full };
    let actions = rune::memo::layout_with("header-actions", |r| r[2]).unwrap_or(0.0);
    let limit = if info.parent.is_some() { (width("header").unwrap_or(0.0) - 22.0) * 0.6 } else { f32::INFINITY };
    let label = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
    let base = Measured {
        location,
        title_box: 0.0,
        title: title_width(s, info),
        project: text_w(proj, label) + 8.0,
        worktree: worktree.map(|w| text_w(w, label) + 8.0),
        limit,
        actions,
    };
    let state = if scope.is_empty() { FIT.get() } else { HeaderFit::Full }.settled(actions);
    // Read as the fit it gives, so a resize re-runs the view only when the fit flips.
    let Some(next) = rune::memo::layout_with(&scoped(scope, "header-title"), move |r| state.measured(Measured { title_box: r[2], ..base })) else {
        return HeaderFit::Full;
    };
    if scope.is_empty() {
        FIT.set(next);
    }
    next
}

/// What the title section's content needs with the title whole (`measureTitleWidth`): the title
/// and its 4 padding each side, the ⋯ button, a subagent's working ring, and the section's gaps
/// (the header gap after them counts as one).
fn title_width(s: &App, info: &SessionInfo) -> f32 {
    let title = text_w(&title_of(info), CRUMB) + 8.0;
    let root = info.parent.is_none();
    let more = if root { 28.0 } else { 0.0 };
    let working = if !root && s.subtree_running(&info.id) { 16.0 + 4.0 + 2.0 } else { 0.0 };
    let count = 2 + usize::from(root) + usize::from(working > 0.0);
    title + more + working + (count - 1) as f32 * TITLE_GAP
}

/// `key`, or `{scope}-{key}` for a copy on a stage.
pub(super) fn scoped(scope: &str, key: &str) -> String {
    if scope.is_empty() { key.to_owned() } else { format!("{scope}-{key}") }
}

/// The header's session title (`h1`: 13/530, 4 by 6 padding), truncating last. A root
/// session's has Desktop's menu (a right-click, or the ⋯ button beside it) and renames when
/// clicked; a subagent's is plain and takes at most 45% of the header.
fn header_title(s: &App, p: &Pal, info: &SessionInfo) -> Vec<El> {
    if info.parent.is_some() {
        // The page's one level-1 heading, as official's `<h1>`.
        return vec![text(title_of(info)).key("title").role("heading").level(1).font(CRUMB).color(p.text).truncate_fade().px(4.0).py(6.0).shrink().min_w(0.0).dir(rune::Direction::Auto)];
    }
    let target = Target::on(crate::servers::runtime::active(s), &info.id, Spot::Header);
    let marked = s.kernel.slots.get(&crate::session_menu::MENUS).on.as_ref() == Some(&target);
    let dots_target = target.clone();
    let title = match crate::session_menu_view::field(s, p, &target, CRUMB) {
        Some(field) => row().key("title").h(28.0).px(4.0).center_y().min_w(0.0).shrink().child(field),
        None => {
            let begin = target.clone();
            // The title in its own direction (`session-identity-header.tsx:263`): an `h1` that renames
            // on a click, no Tab stop (`message-timeline.tsx:540-547`).
            let t = row().key("title").dir(rune::Direction::Auto).role("heading").level(1).label(title_of(info)).h(28.0).px(4.0).center_y().min_w(0.0).shrink().radius(6.0).hover(p.hover).hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.begin_rename(&begin);
                })
                .child(crate::title_change::title(&s.diag.knobs, &format!("title-{}", info.id), title_of(info), CRUMB, p.text));
            crate::session_menu::on(t, target)
        }
    };
    // While its menu is open the `…` keeps its wash, as an expanded trigger does; the title takes none.
    let dots = el().key("header-more").role("button").label_static(t!("common.moreOptions")).size(28.0, 28.0).radius(6.0).center().hover(p.hover).hover_motion(INSTANT, INSTANT)
        .menu_button("header-more", move |s: &mut App, tx: &mut Tx| crate::session_menu::open(s, dots_target.clone(), "header-more", tx))
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    let dots = if marked { dots.fill(p.hover) } else { dots };
    vec![title, dots]
}
