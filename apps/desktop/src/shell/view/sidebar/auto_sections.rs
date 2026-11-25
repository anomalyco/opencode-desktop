//! Consolidated's side tabs under Settings › Group tabs (official
//! `packages/app/src/shell/titlebar/auto-tab-sections.tsx` and the sections of `tab-strip.tsx`,
//! anomalyco/opencode#66350): each project's or server's tabs under a header that collapses
//! them, then the tabs with no project after a divider.

use super::*;
use crate::tabs::auto_groups::{self, Group, Kind, Mark};

/// `text-[13px] leading-[var(--line-height-compact)] [font-weight:440]`
/// (`auto-tab-sections.tsx:143`).
const HEADER: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);

/// The groups down the sidebar, as official's one list has them (`tab-strip.tsx:1052-1073`): its
/// rows 3 apart (`tab-nav.css:179-181`), a header 8 further from what is above it, and a divider
/// before the tabs with no project when anything stands above them.
pub(super) fn sections(s: &App, p: &Pal, groups: &[Group]) -> El {
    let mut list = col().key("consolidated-tabs").w_full().gap(3.0).on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
        tx.live_only();
        s.strip_laid_out(r);
    });
    let mut first = true;
    for g in groups {
        match g.kind {
            Kind::Other if g.tabs.is_empty() => continue,
            // `mx-2 my-2 h-px` (`tab-strip.tsx:1060-1068`).
            Kind::Other if !first => list = list.child(el().key("auto-group-divider").mx(8.0).my(8.0).h(1.0).fill(p.border_muted).a11y_hidden()),
            Kind::Other => {}
            Kind::Project | Kind::Server => list = list.child(header(s, p, g, first)),
        }
        first = false;
        if g.tabs.is_empty() {
            continue;
        }
        let key = format!("auto-tabs-{}", g.key);
        let rows = g.tabs.len() as f32;
        let h = rune::memo::layout_with(&format!("{key}-list"), |r| r[3]).unwrap_or(rows * 28.0 + (rows - 1.0) * 3.0);
        // A project's header names it, so its tabs leave out their avatar (`hideAvatars`,
        // `auto-tab-sections.tsx:202`).
        list = list.child(col().key(format!("{key}-box")).w_full().h(h).child(crate::view::tab_strip::group_strip(s, p, &key, &g.tabs, g.kind == Kind::Project)));
    }
    list
}

/// `GroupHeader` (`auto-tab-sections.tsx:83-181`): one 28 pt button that collapses its group.
/// The project's icon or the server glyph, the name, the server's tag where the same project has
/// a group on another server, a collapsed group's mark, and the chevron, which shows with the
/// pointer over the header or the keyboard's focus on it and turns while the group is collapsed.
fn header(s: &App, p: &Pal, g: &Group, first: bool) -> El {
    let project = (g.kind == Kind::Project).then(|| g.all.first().and_then(|&i| s.tabs.get(i)).and_then(|t| auto_groups::project_of(s, t))).flatten();
    let label = match (project, &g.server) {
        (Some(proj), _) => proj.name.clone(),
        (None, Some(server)) => crate::servers::registry::label_of(s, server),
        (None, None) => String::new(),
    };
    let lead = match project {
        Some(proj) => crate::icons::badge(s, p, Some(proj), &proj.name),
        None => icon(&ic::SERVER, 14.0).color(p.icon_muted),
    };
    let tag = g.server.as_ref().filter(|_| g.spans_servers).and_then(crate::servers::runtime::tag);
    let dot = |color: Color| el().key("mark").size(6.0, 6.0).radius(3.0).fill(color).a11y_hidden();
    let mark = auto_groups::mark(s, g).map(|m| match m {
        Mark::Busy => activity_ring(p).key("ring"),
        Mark::Attention => dot(p.warning),
        Mark::Unread => dot(p.bg_accent),
    });
    let key = g.key.clone();
    row()
        .key(format!("auto-group-{}", g.key))
        .role("button")
        .label(label.clone())
        .expanded(!g.collapsed)
        .w_full()
        .h(28.0)
        // `[&:not(:first-child)]:mt-2`.
        .mt(if first { 0.0 } else { 8.0 })
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .focus_inset(p)
        .hover(p.hover)
        // The header is the chevron's hover group, which the keyboard's focus on it lights too.
        .hover_group()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            auto_groups::toggle(s, &key);
        })
        .child(el().key("lead").size(16.0, 16.0).center().child(lead))
        .child(text(label).key("name").font(HEADER).color(p.muted).group_color(p.text).shrink().min_w(0.0).truncate().dir(rune::Direction::Auto))
        .children(tag.map(|tag| text(tag).key("server").font(HEADER).color(p.faint).shrink().min_w(0.0).truncate()))
        .children(mark)
        .child(session_view::style::hover_reveal(
            icon(&ic::FILL_TRIANGLE_DOWN, 14.0).color(p.icon_muted).rotate(if g.collapsed { -std::f32::consts::FRAC_PI_2 } else { 0.0 }).mirror_in_rtl(),
        ))
}

/// Consolidated tabs grouped by project over the fixture's tabs: one session pinned, which no
/// group holds, and one tab with no project, which comes last.
fn grouped_scene(s: &mut App, layout: crate::layout_state::TabLayout) {
    crate::scenes::open_scene(s);
    s.finish_stream();
    s.settings.sidebar_organize = crate::settings::Organize::Consolidated;
    s.settings.navigation = crate::settings::Navigation::Tabs;
    s.settings.tab_layout = layout;
    s.settings.tab_grouping = crate::settings::TabGrouping::Project;
    *s.layout = crate::layout_state::Shell::at_rest(s.settings.effective_tab_layout());
    if let Some(pin) = s.tabs.get(1).map(|t| crate::tabs::pins::PinRef::new(t.server.clone(), t.id.clone())) {
        crate::tabs::pins::update(s, |d| d.pin(pin));
    }
    s.tabs.push(crate::tabs::Tab::session("ses_grouped_other", "A session with no project", "/Users/demo/notes"));
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    // Settings › Group tabs › By project, side tabs: a header over each project's tabs.
    ("sidebar-grouped", |s| grouped_scene(s, crate::layout_state::TabLayout::Vertical)),
    // The same with its first three groups collapsed: the open tab's group keeps showing it, and
    // the others say what their hidden tabs need.
    ("sidebar-grouped-collapsed", |s| {
        grouped_scene(s, crate::layout_state::TabLayout::Vertical);
        let keys: Vec<String> = auto_groups::groups(s).into_iter().flatten().filter(|g| g.kind != Kind::Other).take(3).map(|g| g.key).collect();
        for key in keys {
            auto_groups::toggle(s, &key);
        }
    }),
    // By project in top tabs: each project's tabs together, nothing to collapse.
    ("top-grouped", |s| grouped_scene(s, crate::layout_state::TabLayout::Horizontal)),
]);
