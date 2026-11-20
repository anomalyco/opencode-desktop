//! Side tabs organized by project (Settings › Experimental › Organize vertical sidebar, Projects;
//! `sidebar-organized.tsx`): a Projects section, each used project a row that opens to its latest
//! sessions, then a divider and the side tabs. The projects are one server's, the one the current
//! tab is on (else Home's), which the window works against (`servers::runtime`); the tabs are
//! every server's.
use std::rc::Rc;

use rune_kit::ScrollBar;

use super::*;
use opencode_i18n::t;

/// A project's key in the list's state (`${server}:${project.id}`): each server's list opens and
/// pages its projects apart.
pub(crate) fn project_key(s: &App, id: &str) -> String {
    format!("{}:{id}", crate::servers::runtime::active(s).as_str())
}

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
        section = section.child(if s.project_navigation() && s.kernel.slots.get(&crate::projects::HOME_STATE).servers.len() > 1 { server_groups(s, p) } else { projects(s, p) });
    }
    // The projects, the divider and the side tabs scroll together (`flex flex-col gap-3` in one
    // `ScrollView`, `sidebar-organized.tsx:118-158`).
    let mut content = col().key("organize-content").w_full().gap(12.0).child(section);
    let (pins, recent) = sidebar::pins::section(s, p);
    content = content.children(pins);
    if !recent.is_empty() {
        let mut tabs = col().key("organize-tabs-section").w_full().gap(12.0);
        if !st.organize_collapsed {
            tabs = tabs.child(el().key("organize-divider").mx(8.0).h(1.0).fill(p.border_muted));
        }
        // Desktop's strip here is as tall as its tabs (its section has no height of its own), so
        // this one list scrolls them. The strip scrolls itself and, with no height given, lays
        // out at none: it takes its tabs' height (`tabs-list`, as of their last layout).
        let tabs_h = rune::memo::layout_with("tabs-list", |r| r[3]).unwrap_or(recent.len() as f32 * 30.0);
        content = content.child(tabs.child(sidebar::rows::section_title(p, "organize-recent-title", t!("sidebar.recent.section.recent")))
            .child(col().key("organize-tabs").w_full().h(tabs_h).child(super::tab_strip::section_strip(s, p, "tabs", &recent, false))));
    }
    // Its thumb runs in a 12 pt track past the list's end edge, across the sidebar's end inset and
    // the gap to the panel (`inset-inline-end: calc(-1 * (3px + 8px))`), and the pointer anywhere
    // over the list shows it (`thumbContainer`, `thumbHoverTarget`, `:107-118`).
    // The scroll reaches that far past the list, so its track is inside it; the rows keep to the list.
    let bar = rune_kit::ScrollBarStyle::scroll_view(crate::palette::kit(p)).hover_target();
    let viewport = col().grow(1.0).shrink().min_h(0.0).mr(-TRACK_PAST_END);
    let scroll = session_view::scroller::scroller("organize-scroll").bar(session_view::scroller::Bar::Styled(bar)).build::<App>(p, viewport, col().w_full().pr(TRACK_PAST_END).child(content));
    col().key("organize").w_full().grow(1.0).shrink().min_h(0.0).child(scroll).scroll_bar_hover("organize-scroll")
}

/// How far past the list's end edge the thumb's track ends: the sidebar's end inset and the
/// shell's inline inset (`--sidebar-inset-end`, `--shell-inline-inset`).
const TRACK_PAST_END: f32 = 3.0 + 8.0;

/// Only label the server groups when more than one is visible (`sidebar-organized.tsx`, #64220).
fn server_groups(s: &App, p: &Pal) -> El {
    use crate::servers::runtime;
    let mut groups = col().w_full().gap(4.0);
    for server in &s.kernel.slots.get(&crate::projects::HOME_STATE).servers {
        let key = server.key.clone();
        let collapsed = crate::projects::home::collapsed(&key);
        let mut group = col().w_full().gap(4.0)
            .child(small_button(p, &format!("server-{key}"), &server.label).expanded(!collapsed)
                .on_press_tx(move |_: &mut App, tx| crate::projects::home::toggle_collapsed(&key, tx)));
        if !collapsed && server.key == *runtime::active(s) { group = group.child(projects(s, p)); }
        if !collapsed && server.key != *runtime::active(s) {
            for proj in &server.projects {
                let key = format!("{}:{}", server.key, proj.id);
                let open = s.project_state.organize_expanded.contains(&key);
                let (on, id, anchor) = (server.key.clone(), proj.id.clone(), key.clone());
                group = group.child(small_button(p, &key, &proj.name).expanded(open).on_press_tx(move |s: &mut App, tx| {
                    tx.instant();
                    if !s.project_state.organize_expanded.remove(&key) { s.project_state.organize_expanded.insert(key.clone()); }
                }).on_secondary_press(move |s: &mut App, _, tx| {
                    runtime::with_server(s, &on, |s| home::projects::open_project_menu(s, &id, anchor.clone(), tx));
                }));
                if open && let Some(rows) = server.sessions.get(&proj.id) {
                    for (id, title) in rows.iter().take(SESSIONS) {
                        let (server, id) = (server.key.clone(), id.clone());
                        group = group.child(small_button(p, &format!("so-{server}-{id}"), title).pl(32.0).on_press_tx(move |s: &mut App, tx| {
                            runtime::switch(s, &server); s.open(&id, tx);
                        }));
                    }
                }
            }
        }
        groups = groups.child(group);
    }
    groups
}

/// The Projects toggle: its label and a triangle that turns shut.
fn header(p: &Pal, collapsed: bool) -> El {
    row()
        .key("organize-header")
        .role("button")
        .label_static(t!("sidebar.organize.projects"))
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
        .child(icon(&ic::FILL_TRIANGLE_DOWN, 14.0).color(p.icon_muted).rotate(if collapsed { -std::f32::consts::FRAC_PI_2 } else { 0.0 }).mirror_in_rtl())
}

/// The used projects in the list's order (the first seven, or all after Show more), each with its
/// sessions while open. Only the blocks in sight are built (a virtual list): Show more over
/// hundreds of projects builds a screenful.
fn projects(s: &App, p: &Pal) -> El {
    let st = &s.project_state;
    let used = s.organized_projects();
    let shown = if st.organize_all { used.len() } else { used.len().min(PROJECTS) };
    let more = used.len() > PROJECTS;
    let ids: Rc<Vec<String>> = Rc::new(used[..shown].iter().map(|proj| proj.id.clone()).collect());
    let n = ids.len() + usize::from(more);
    let version = (ids.clone(), more);
    let (keys, built, pal) = (ids.clone(), ids, *p);
    col()
        .key("organize-project-list")
        .w_full()
        .gap(4.0)
        .growth(rune::GrowthSpec::default())
        .virtual_rows(
            n,
            // A shut project's row.
            28.0,
            // The rows' keys are their blocks' own (a row takes its key), so paths stay as they were.
            move |_: &App, i| keys.get(i).map_or_else(|| "organize-more".to_owned(), |id| format!("so-{id}")).into(),
            move |s: &App, i| match built.get(i) {
                Some(id) => match s.projects.iter().find(|proj| proj.id == *id) {
                    Some(proj) => project(s, &pal, proj, i),
                    None => el().key(format!("so-{id}")).h(28.0),
                },
                None => {
                    let label = if s.project_state.organize_all { t!("sidebar.organize.showLess") } else { t!("sidebar.organize.showMore") };
                    small_button(&pal, "organize-more", label).on_press_tx(|s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        s.project_state.organize_all = !s.project_state.organize_all;
                    })
                }
            },
        )
        .virtual_rows_version(version)
}

/// A project's row (`ProjectRow`): its icon and name toggle its sessions, or drag the project and
/// its sessions to a new place; More options and New session show at its end while hovered or its
/// menu is open.
fn project(s: &App, p: &Pal, proj: &model::Project, index: usize) -> El {
    let key = project_key(s, &proj.id);
    let open = s.project_state.organize_expanded.contains(&key);
    let menu = format!("so-menu-{}", proj.id);
    let menu_open = s.popup.open_at(&menu);
    let (id, menu_id, ctx_id, new_dir, drag_id, layout_id) = (key, proj.id.clone(), proj.id.clone(), proj.directory.clone(), proj.id.clone(), proj.id.clone());
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
            let st = &mut *s.project_state;
            // A release that ends a drag isn't a press on the row it dropped.
            match st.organize_drag {
                OrganizeDrag::JustDropped => {
                    st.organize_drag = OrganizeDrag::Idle;
                    return;
                }
                OrganizeDrag::Idle | OrganizeDrag::Dragging(_) => {}
            }
            // Closing it pages its sessions back to the first five (`onToggle`).
            if !st.organize_expanded.remove(&id) {
                st.organize_expanded.insert(id.clone());
            }
            st.organize_limits.remove(&id);
        })
        .on_drag(move |s: &mut App, d: &mut rune::Drag, tx: &mut Tx| drag(s, &drag_id, index, d, tx))
        .child(el().size(16.0, 16.0).center().child(crate::icons::badge(s, p, Some(proj), &proj.name)))
        .child(text(proj.name.clone()).key("label").font(UI).color(p.muted).group_color(p.text).grow(1.0).shrink().truncate());
    // The dragged row takes no hover: its wash and its buttons stay off while it follows the pointer.
    let lifted = match &s.project_state.organize_drag {
        OrganizeDrag::Dragging(d) => d.from == index,
        OrganizeDrag::Idle | OrganizeDrag::JustDropped => false,
    };
    let head = row()
        .key("head")
        .w_full()
        .h(28.0)
        .pr(4.0)
        .center_y()
        .radius(6.0)
        .on_secondary_press(move |s: &mut App, _: rune::PointerPress, tx: &mut Tx| home::projects::open_project_menu(s, &ctx_id, ctx_anchor.clone(), tx))
        .child(nav)
        .child(match (menu_open, lifted) {
            (true, _) => actions,
            (false, true) => actions.opacity(0.0),
            (false, false) => actions.group_opacity(0.0, 1.0),
        });
    let head = if lifted { head } else { head.hover_group().hover(p.hover) };
    let mut r = col()
        .key(format!("so-{}", proj.id))
        .w_full()
        .on_layout(move |s: &mut App, rect| _ = s.project_state.organize_heights.insert(layout_id.clone(), rect[3]))
        .child(head);
    if open {
        r = r.child(col().key("sessions").w_full().gap(3.0).pb(4.0).children(sessions(s, p, proj)));
    }
    match &s.project_state.organize_drag {
        // It follows the pointer over the rows, which make room for it as it passes them.
        OrganizeDrag::Dragging(d) if d.from == index => r.raise().offset(0.0, d.dy).offset_motion(rune::Anim::INSTANT),
        OrganizeDrag::Dragging(d) => r.offset(0.0, drag_shift(d, s, index)),
        OrganizeDrag::Idle | OrganizeDrag::JustDropped => r,
    }
}

pub use crate::projects::OrganizeDrag;
pub use crate::projects::sidebar::ProjectDrag;

/// Points the pointer travels before a press on a row becomes a drag (`Distance({ value: 4 })`).
const DRAG_SLOP: f32 = 4.0;
/// The list's gap between project blocks (`gap-1`).
const BLOCK_GAP: f32 = 4.0;

/// How far block `index` sits from its place while another is dragged: a step toward the
/// dragged block's place when it's between that and where the block would land.
fn drag_shift(drag: &ProjectDrag, s: &App, index: usize) -> f32 {
    let pitch = height(s, &drag.ids[drag.from]) + BLOCK_GAP;
    if drag.from < index && index <= drag.to {
        -pitch
    } else if drag.to <= index && index < drag.from {
        pitch
    } else {
        0.0
    }
}

fn height(s: &App, id: &str) -> f32 {
    s.project_state.organize_heights.get(id).copied().unwrap_or(28.0)
}

fn drag(s: &mut App, id: &str, index: usize, d: &mut rune::Drag, tx: &mut Tx) {
    match d.phase {
        rune::Phase::Down => match s.project_state.organize_drag {
            OrganizeDrag::JustDropped => s.project_state.organize_drag = OrganizeDrag::Idle,
            OrganizeDrag::Idle | OrganizeDrag::Dragging(_) => {}
        },
        rune::Phase::Move => {
            let mut drag = match &s.project_state.organize_drag {
                OrganizeDrag::Dragging(_) => match std::mem::take(&mut s.project_state.organize_drag) {
                    OrganizeDrag::Dragging(drag) => drag,
                    OrganizeDrag::Idle | OrganizeDrag::JustDropped => return,
                },
                OrganizeDrag::Idle | OrganizeDrag::JustDropped => {
                    if d.dx.hypot(d.dy) < DRAG_SLOP {
                        return;
                    }
                    let ids: Vec<String> = s.organized_projects().iter().map(|p| p.id.clone()).collect();
                    if ids.get(index).map(String::as_str) != Some(id) {
                        return;
                    }
                    ProjectDrag { from: index, to: index, dy: 0.0, ids }
                }
            };
            tx.animated();
            let shown = if s.project_state.organize_all { usize::MAX } else { PROJECTS };
            drag.dy = d.dy;
            drag.to = target(s, &drag, shown);
            s.project_state.organize_drag = OrganizeDrag::Dragging(drag);
        }
        rune::Phase::Up => {
            let drag = match &s.project_state.organize_drag {
                OrganizeDrag::Dragging(_) => match std::mem::replace(&mut s.project_state.organize_drag, OrganizeDrag::JustDropped) {
                    OrganizeDrag::Dragging(drag) => drag,
                    OrganizeDrag::Idle | OrganizeDrag::JustDropped => return,
                },
                OrganizeDrag::Idle | OrganizeDrag::JustDropped => return,
            };
            tx.instant();
            if d.cancelled() || drag.from == drag.to {
                return;
            }
            let mut ids = drag.ids;
            let moved = ids.remove(drag.from);
            ids.insert(drag.to, moved);
            let server = crate::servers::runtime::active(s).as_str().to_owned();
            s.project_state.organize_order.insert(server, ids);
        }
    }
}

/// Where the dragged block lands: past each block whose middle its leading edge has crossed, as
/// dnd-kit's sorting by the most overlapped slot does.
fn target(s: &App, drag: &ProjectDrag, shown: usize) -> usize {
    let count = drag.ids.len().min(shown);
    let tops: Vec<f32> = drag.ids[..count].iter().scan(0.0, |y, id| {
        let top = *y;
        *y += height(s, id) + BLOCK_GAP;
        Some(top)
    }).collect();
    let top = tops[drag.from] + drag.dy;
    let bottom = top + height(s, &drag.ids[drag.from]);
    let middle = |j: usize| tops[j] + height(s, &drag.ids[j]) / 2.0;
    let below = (drag.from + 1..count).take_while(|&j| bottom > middle(j)).count();
    let above = (0..drag.from).rev().take_while(|&j| top < middle(j)).count();
    drag.from + below - above
}

/// A project's latest root sessions, five more per Show more, or a line when it has none.
fn sessions(s: &App, p: &Pal, proj: &model::Project) -> Vec<El> {
    let mut all: Vec<&SessionInfo> = s
        .sessions
        .iter()
        .filter(|i| i.parent.is_none() && i.archived.is_none() && !crate::view::is_automation(&i.title) && s.project_for_session(i).is_some_and(|p| p.id == proj.id))
        .collect();
    all.sort_by_key(|i| std::cmp::Reverse(s.recent_page.active_ms(i)));
    if all.is_empty() {
        return vec![text(t!("sidebar.organize.empty")).key("organize-empty").font(UI_SMALL).color(p.faint).pl(32.0).pr(8.0).py(8.0)];
    }
    let key = project_key(s, &proj.id);
    let limit = s.project_state.organize_limits.get(&key).copied().unwrap_or(SESSIONS);
    let mut rows: Vec<El> = all.iter().take(limit).map(|ses| session(s, p, ses)).collect();
    if all.len() > limit {
        let id = key;
        rows.push(small_button(p, &format!("so-more-{}", proj.id), t!("sidebar.organize.showMore")).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            *s.project_state.organize_limits.entry(id.clone()).or_insert(SESSIONS) += SESSIONS;
        }));
    }
    rows
}

/// A session under its project (`ProjectSessionRow`): a dot at its start when it has a tab open,
/// its title (the text colour when it is the current session opened from this list, faint
/// otherwise), and a ring while it works or a dot while it is unread. A right-click opens its
/// menu: Rename, Export…, Mark as read or unread, then Delete….
fn session(s: &App, p: &Pal, ses: &SessionInfo) -> El {
    // The list's server is the one the window works against, so its sessions route `here`.
    let uid = crate::tabs::session_uid(crate::servers::runtime::active(s), &ses.id);
    let active = s.route == Route::here(ses.id.clone()) && s.project_state.organize_selected.as_deref() == Some(uid.as_str());
    let open_tab = s.tabs.iter().any(|t| t.route() == Route::here(ses.id.clone()));
    let id = ses.id.clone();
    let target = crate::session_menu::Target::on(crate::servers::runtime::active(s), &ses.id, crate::session_menu::Spot::Row);
    let mark = match s.tab_status(&ses.id) {
        Some(crate::TabStatus::Busy) => Some(activity_ring(p).key("ring")),
        _ if crate::session_menu::preferences::muted(&target.server, &target.id) => Some(sidebar::pins::muted_mark(p)),
        Some(crate::TabStatus::Unread | crate::TabStatus::Failed) if !active => Some(el().key("unread").size(6.0, 6.0).radius(3.0).fill(p.text)),
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
        .focus_inset(p)
        .hover(p.hover)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            s.project_state.organize_selected = Some(crate::tabs::session_uid(crate::servers::runtime::active(s), &id));
            s.open(&id, tx);
        })
        .child(text(ses.title.clone()).key("title").font(UI_MED).color(if active { p.text } else { p.faint }).grow(1.0).shrink().truncate().dir(rune::Direction::Auto))
        .child(el().key("status").size(16.0, 16.0).center().children(mark));
    if open_tab {
        r = r.child(el().key("open-tab").label_static(t!("sidebar.organize.openTab")).pin(Some(8.0), None, None, None).size(16.0, 16.0).center().child(el().size(4.0, 4.0).radius(2.0).fill(p.icon_faint)));
    }
    if active {
        r = r.fill(p.pressed);
    }
    // A right-click opens the row's menu (`ProjectSessionRow`'s `Menu.Context`); its Rename opens
    // the Rename dialog.
    crate::session_menu::on_dialog(r, target)
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
