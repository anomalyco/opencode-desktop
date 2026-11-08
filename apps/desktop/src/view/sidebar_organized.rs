//! Side tabs organized by project (Settings › Experimental › Organize vertical sidebar, Projects;
//! `sidebar-organized.tsx`): a Projects section, each used project a row that opens to its latest
//! sessions, then a divider and the side tabs.
use rune_kit::shell::TabOrientation;

use super::*;
use opencode_i18n::t;

/// Projects shown before Show more, and each project's sessions before its own.
const PROJECTS: usize = 7;
const SESSIONS: usize = 5;
/// The Projects header (`text-[13px] leading-4 font-medium`).
const HEADER: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// A small ghost-muted Button's label.
const BUTTON: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);

/// The organized list in the sidebar's list slot.
pub(super) fn organized(s: &App, p: &Pal) -> El {
    let st = &s.project_state;
    let mut section = col().key("organize-projects").w_full().gap(8.0).child(header(p, st.organize_collapsed));
    if !st.organize_collapsed {
        section = section.child(col().key("organize-project-list").w_full().gap(4.0).children(projects(s, p)));
    }
    // The projects scroll in what the side tabs leave them; the side tabs scroll in their own strip.
    let projects = col()
        .key("organize-scroll")
        .w_full()
        .shrink()
        .min_h(0.0)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .child(section);
    let mut list = col().key("organize").w_full().grow(1.0).shrink().min_h(0.0).gap(12.0).child(projects);
    if !s.tabs.is_empty() {
        if !st.organize_collapsed {
            list = list.child(el().key("organize-divider").mx(8.0).h(1.0).fill(p.border_muted));
        }
        list = list.child(col().key("organize-tabs").w_full().grow(1.0).min_h(0.0).child(tab_strip(s, p, "tabs", TabOrientation::Vertical, None, None)));
    }
    list
}

/// The Projects toggle: its label and a triangle that turns shut.
fn header(p: &Pal, collapsed: bool) -> El {
    row()
        .key("organize-header")
        .role("button")
        .label(t!("sidebar.organize.projects"))
        .expanded(!collapsed)
        .w_full()
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.project_state.organize_collapsed = !s.project_state.organize_collapsed;
        })
        .child(text(t!("sidebar.organize.projects")).font(HEADER).color(p.muted).group_color(p.text).shrink().truncate())
        .child(icon(&ic::FILL_TRIANGLE_DOWN, 14.0).color(p.icon_muted).rotate(if collapsed { -std::f32::consts::FRAC_PI_2 } else { 0.0 }))
}

/// The used projects (the first seven, or all after Show more), each with its sessions while open.
fn projects(s: &App, p: &Pal) -> Vec<El> {
    let st = &s.project_state;
    let used = s.used_projects();
    let shown = if st.organize_all { used.len() } else { used.len().min(PROJECTS) };
    let mut rows: Vec<El> = used[..shown].iter().map(|proj| project(s, p, proj)).collect();
    if used.len() > PROJECTS {
        let label = if st.organize_all { t!("sidebar.organize.showLess") } else { t!("sidebar.organize.showMore") };
        rows.push(small_button(p, "organize-more", label).on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.project_state.organize_all = !s.project_state.organize_all;
        }));
    }
    rows
}

/// A project's row (`ProjectRow`): its icon and name toggle its sessions; More options and New
/// session show at its end while hovered or its menu is open.
fn project(s: &App, p: &Pal, proj: &model::Project) -> El {
    let open = s.project_state.organize_expanded.contains(&proj.id);
    let menu = format!("so-menu-{}", proj.id);
    let menu_open = s.popup.open_at(&menu);
    let (id, menu_id, ctx_id, new_dir) = (proj.id.clone(), proj.id.clone(), proj.id.clone(), proj.directory.clone());
    let (anchor, ctx_anchor) = (menu.clone(), menu.clone());
    let actions = row()
        .key("actions")
        .gap(4.0)
        .center_y()
        .child(
            home::projects::icon_button(p, &menu, t!("common.moreOptions"), &ic::OUTLINE_DOTS, menu_open)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| home::projects::open_project_menu(s, &menu_id, anchor.clone(), tx)),
        )
        .child(home::projects::icon_button(p, &format!("so-new-{}", proj.id), t!("command.session.new"), &ic::EDIT, false).on_press_tx(move |s: &mut App, tx: &mut Tx| s.new_session_in(&new_dir, tx)));
    let nav = row()
        .key("nav")
        .role("button")
        .label(proj.name.clone())
        .expanded(open)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h_full()
        .px(8.0)
        .gap(8.0)
        .center_y()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let st = &mut s.project_state;
            if !st.organize_expanded.remove(&id) {
                st.organize_expanded.insert(id.clone());
            }
            st.organize_limits.remove(&id);
        })
        .child(el().size(16.0, 16.0).center().child(crate::icons::badge(s, p, Some(proj), &proj.name)))
        .child(text(proj.name.clone()).key("label").font(UI).color(p.muted).group_color(p.text).grow(1.0).shrink().truncate());
    let head = row()
        .key("head")
        .w_full()
        .h(28.0)
        .pr(4.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .hover(p.hover)
        .on_secondary_press(move |s: &mut App, _: rune::PointerPress, tx: &mut Tx| home::projects::open_project_menu(s, &ctx_id, ctx_anchor.clone(), tx))
        .child(nav)
        .child(if menu_open { actions } else { actions.group_opacity(0.0, 1.0) });
    let mut r = col().key(format!("so-{}", proj.id)).w_full().child(head);
    if open {
        r = r.child(col().key("sessions").w_full().gap(3.0).pb(4.0).children(sessions(s, p, proj)));
    }
    r
}

/// A project's latest root sessions, five more per Show more, or a line when it has none.
fn sessions(s: &App, p: &Pal, proj: &model::Project) -> Vec<El> {
    let dir = crate::projects::clean_dir(&proj.directory);
    let mut all: Vec<&SessionInfo> = s
        .sessions
        .iter()
        .filter(|i| i.parent.is_none() && i.archived.is_none() && crate::projects::clean_dir(&i.directory) == dir)
        .collect();
    all.sort_by_key(|i| std::cmp::Reverse(i.updated));
    if all.is_empty() {
        return vec![text(t!("sidebar.organize.empty")).key("organize-empty").font(UI_SMALL).color(p.faint).pl(32.0).pr(8.0).py(8.0)];
    }
    let limit = s.project_state.organize_limits.get(&proj.id).copied().unwrap_or(SESSIONS);
    let mut rows: Vec<El> = all.iter().take(limit).map(|ses| session(s, p, ses)).collect();
    if all.len() > limit {
        let id = proj.id.clone();
        rows.push(small_button(p, &format!("so-more-{}", proj.id), t!("sidebar.organize.showMore")).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            *s.project_state.organize_limits.entry(id.clone()).or_insert(SESSIONS) += SESSIONS;
        }));
    }
    rows
}

/// A session under its project (`ProjectSessionRow`): a dot at its start when it has a tab open,
/// its title (the text colour when it is the current session opened from this list, faint
/// otherwise), and a ring while it works or a dot while it is unread.
fn session(s: &App, p: &Pal, ses: &SessionInfo) -> El {
    let active = s.route == Route::Session(ses.id.clone()) && s.project_state.organize_selected.as_deref() == Some(ses.id.as_str());
    let open_tab = s.tabs.iter().any(|t| t.route() == Route::Session(ses.id.clone()));
    let id = ses.id.clone();
    let mark = match s.tab_status(&ses.id) {
        Some(crate::TabStatus::Busy) => Some(activity_ring(p).key("ring")),
        Some(_) if !active && ses.unread() => Some(el().key("unread").size(6.0, 6.0).radius(3.0).fill(p.text)),
        _ => None,
    };
    let mut r = row()
        .key(format!("so-session-{}", ses.id))
        .role("button")
        .label(ses.title.clone())
        .w_full()
        .h(28.0)
        .pl(32.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            s.project_state.organize_selected = Some(id.clone());
            s.open(&id, tx);
        })
        .child(text(ses.title.clone()).key("title").font(UI_MED).color(if active { p.text } else { p.faint }).grow(1.0).shrink().truncate())
        .child(el().key("status").size(16.0, 16.0).center().children(mark));
    if open_tab {
        r = r.child(el().key("open-tab").label(t!("sidebar.organize.openTab")).pin(Some(8.0), None, None, None).size(16.0, 16.0).center().child(el().size(4.0, 4.0).radius(2.0).fill(p.icon_faint)));
    }
    if active {
        r = r.fill(p.pressed);
    }
    r
}

/// A small ghost-muted Button, 23 in from the list's start (`ms-[23px] self-start`).
fn small_button(p: &Pal, key: &str, label: &str) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .ml(23.0)
        .self_start()
        .h(24.0)
        .px(8.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(BUTTON).color(p.muted))
}
