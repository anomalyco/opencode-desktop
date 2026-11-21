//! The rows the Customize and Recent sidebars share: a session under a group
//! (`ProjectSessionRow`, `project-session-row.tsx`), a group's header (`GroupHeaderView`,
//! `sidebar-customized.tsx:543-644`), a section's title and the small ghost buttons.

use super::*;
use crate::session_menu::{Spot, Target};
use opencode_i18n::t;

/// A collapsible section's title (`text-[12px] font-medium`) and a group's (`text-[13px]`, 440).
const SECTION: TextStyle = TextStyle::new(12.0, 530.0).lh(16.0);
const GROUP: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
/// The Sessions, Pinned and Recent headers (`text-[13px] leading-4 font-medium`).
pub(super) const HEADER: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// A row's faint trailing label and a collapsed group's count (`text-[11px]`).
const LABEL: TextStyle = TextStyle::new(11.0, 400.0).lh(16.0);
/// A ghost-muted Button's label.
const BUTTON: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);

/// What a session row shows (`ProjectSessionRow`'s props).
pub(super) struct SessionRow<'a> {
    pub server: &'a opencode_kit::servers::ServerKey,
    pub session: &'a SessionInfo,
    pub active: bool,
    pub open_tab: bool,
    /// The faint trailing label: the session's server where several mix.
    pub label: Option<String>,
    /// Extra 14 pt steps in, for subagents.
    pub depth: usize,
    /// The project avatar at its start, where no project header names it.
    pub project: Option<&'a model::Project>,
}

/// `ProjectSessionRow`: 28 tall, 32 in (14 more per step), its title in the text colour while
/// current and faint otherwise, a ring while it works or a dot while unread; its menu on a
/// right-click. A press opens it.
pub(super) fn session_row(s: &App, p: &Pal, key: String, r: SessionRow) -> El {
    let id = r.session.id.clone();
    let mark = match s.tab_status(&id) {
        Some(crate::TabStatus::Busy) => Some(activity_ring(p).key("ring")),
        _ if crate::session_menu::preferences::muted(r.server, &id) => Some(super::pins::muted_mark(p)),
        Some(crate::TabStatus::Unread | crate::TabStatus::Failed) if !r.active => Some(el().key("unread").size(6.0, 6.0).radius(3.0).fill(p.text)),
        _ => None,
    };
    let start = 32.0 + 14.0 * r.depth as f32;
    let open = id.clone();
    let mut row = row()
        .key(key)
        .role("button")
        .label(r.session.title.clone())
        .w_full()
        .h(28.0)
        .pl(start)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .focus_inset(p)
        .hover(if r.active { p.pressed } else { p.hover })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open(&open, tx));
    // Rename from its menu opens the Rename dialog (`project-session-row.tsx:185-197`).
    let target = Target::on(r.server, &id, Spot::Row);
    row = row.child(text(r.session.title.clone()).key("title").font(UI_MED).color(if r.active { p.text } else { p.faint }).grow(1.0).shrink().truncate().dir(rune::Direction::Auto));
    if let Some(label) = r.label {
        row = row.child(text(label).key("server").font(LABEL).color(p.faint).shrink().truncate());
    }
    row = row.child(el().key("status").size(16.0, 16.0).center().children(mark));
    if r.open_tab {
        row = row.child(el().key("open-tab").label(t!("sidebar.organize.openTab")).pin(Some(8.0), None, None, None).size(16.0, 16.0).center().child(el().size(4.0, 4.0).radius(2.0).fill(p.icon_faint)));
    } else if let Some(proj) = r.project {
        row = row.child(el().key("avatar").pin(Some(8.0), None, None, None).size(16.0, 16.0).center().child(crate::icons::badge(s, p, Some(proj), &proj.name)));
    }
    if r.active {
        row = row.fill(p.pressed);
    }
    crate::session_menu::on_dialog(row, target)
}

/// A group's lead: a project's avatar, a server's glyph, or Other's dots.
pub(super) enum Lead<'a> {
    Project(&'a model::Project),
    Server,
    Other,
}

/// `GroupHeaderView`: a leaf group's 28 pt row, or a Server → Project section's 24 pt title.
/// The label toggles the group; at its end a collapsed group counts its open tabs, and a
/// project's pencil (shown on hover, over the count) starts a session there.
#[allow(clippy::too_many_arguments)]
pub(super) fn group_header(s: &App, p: &Pal, key: &str, label: String, lead: Lead, section: bool, first: bool, collapsed: bool, open: usize, new_session: Option<(String, opencode_kit::servers::ServerKey)>) -> El {
    let toggle_key = key.to_owned();
    let glyph = match lead {
        Lead::Project(proj) => crate::icons::badge(s, p, Some(proj), &proj.name),
        Lead::Server => icon(&ic::SERVER, 14.0).color(p.icon_muted),
        Lead::Other => icon(&ic::OUTLINE_DOTS, 14.0).color(p.icon_muted),
    };
    let (font, ink, hover_ink) = if section { (SECTION, p.faint, p.muted) } else { (GROUP, p.muted, p.text) };
    let button = row()
        .key("toggle")
        .role("button")
        .label(label.clone())
        .expanded(!collapsed)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h_full()
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .focus_inset(p)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let c = &mut s.project_state.customize.collapsed;
            let next = !c.get(&toggle_key).copied().unwrap_or(false);
            c.insert(toggle_key.clone(), next);
        })
        .child(el().size(16.0, 16.0).center().child(glyph))
        .child(text(label).font(font).color(ink).group_color(hover_ink).grow(1.0).shrink().truncate().dir(rune::Direction::Auto));
    let mut end = el().key("end").size(20.0, 20.0).center();
    if collapsed && open > 0 {
        let count = text(open.to_string()).key("count").font(LABEL).color(p.faint);
        end = end.child(if new_session.is_some() { count.group_opacity(1.0, 0.0) } else { count });
    }
    if let Some((dir, server)) = new_session {
        let b = super::super::home::projects::icon_button(p, &format!("{key}-new"), t!("command.session.new"), &ic::EDIT, false)
            .absolute(0.0, 0.0)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                if server != *crate::servers::runtime::active(s) {
                    crate::servers::runtime::switch(s, &server);
                }
                s.new_session_in(&dir, tx)
            });
        end = end.child(b.group_opacity(0.0, 1.0));
    }
    let r = row().key(format!("head-{key}")).w_full().pr(4.0).center_y().radius(6.0).hover_group().child(button).child(end);
    if section {
        r.h(24.0).mt(if first { 0.0 } else { 8.0 })
    } else {
        r.h(28.0).hover(p.hover)
    }
}

/// A section's title (`SectionHeader`, `recent-sessions.tsx:651-660`): 24 tall, `px-2`.
pub(crate) fn section_title(p: &Pal, key: &str, label: &str) -> El {
    row().key(key.to_owned()).role("heading").label(label.to_owned()).w_full().h(24.0).px(8.0).center_y().child(text(label.to_owned()).font(HEADER).color(p.muted).shrink().truncate())
}

/// A ghost-muted Button: `small` 24 tall, 23 in from the list's start (`ms-[23px] self-start`),
/// or `normal` 28 tall across the list (`w-full justify-start px-2`).
pub(super) fn ghost_button(p: &Pal, key: &str, label: &str, small: bool) -> El {
    let b = row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .px(8.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .focus_inset(p)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(BUTTON).color(p.muted));
    if small { b.ml(23.0).self_start().h(24.0) } else { b.w_full().h(28.0) }
}
