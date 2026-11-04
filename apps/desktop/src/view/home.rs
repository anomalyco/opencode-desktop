use super::*;
use crate::session_menu::{Spot, Target};

// ---------------------------------------------------------------------------------------------
// Home: projects (Desktop's `home/projects/view.tsx`) and recent sessions by day (`home/sessions`).

const WIDE_HOME_MIN_W: f32 = 860.0;

pub(crate) fn home(s: &App, p: &Pal, panel_w: f32) -> El {
    if s.launch.offline {
        return no_server_home(s, p);
    }
    let selected_id = s.project_state.home_selected.as_deref();
    let selected = selected_id.and_then(|id| s.project(id));
    let show_project_name = selected.is_none();

    let q = s.search.text.trim().to_lowercase();
    let matched = s.home_sessions(selected);
    let mut groups: Vec<(&str, Vec<&SessionInfo>)> = vec![("Today", vec![]), ("Yesterday", vec![]), ("Older", vec![])];
    let day = |ms: i64| ms.div_euclid(86_400_000);
    for ses in matched {
        let d = day(s.now_ms) - day(ses.updated);
        groups[if d <= 0 { 0 } else if d == 1 { 1 } else { 2 }].1.push(ses);
    }
    let nav_ids = std::rc::Rc::new(groups.iter().flat_map(|(_, items)| items.iter().map(|s| s.id.clone())).collect::<Vec<_>>());
    let active_id = s.home_search_active(&nav_ids);
    let mut list = col().key("groups").role("list").focus_group().w_full().pr(12.0).pb(64.0);
    let mut first = true;
    for (title, items) in groups.into_iter().filter(|g| !g.1.is_empty()) {
        let mut header = row().key(format!("h-{title}")).h(28.0).pl(12.0).center_y().child(text(title).font(UI).color(p.muted));
        if first {
            header = header.child(spacer()).child(new_session_button(p));
        }
        let mut rows = col().key(format!("g-{title}")).gap(1.0).pt(16.0).mb(24.0);
        for ses in items {
            let index = nav_ids.iter().position(|x| x == &ses.id).unwrap_or(0);
            let active = active_id.as_deref() == Some(ses.id.as_str());
            rows = rows.child(session_row_inner(s, p, ses, &nav_ids, index, show_project_name, active, &q));
        }
        list = list.child(header).child(rows);
        first = false;
    }
    if first {
        if !q.is_empty() {
            list = list.child(
                row()
                    .key("no-results")
                    .h(40.0)
                    .pl(12.0)
                    .center_y()
                    .child(text(format!("No sessions found for {}", s.search.text)).font(UI).color(p.muted)),
            );
        } else if let Some(proj) = selected {
            list = list.child(
                row()
                    .key("no-project-sessions")
                    .h(40.0)
                    .pl(12.0)
                    .center_y()
                    .child(text(format!("No sessions in {} yet", proj.name)).font(UI).color(p.muted))
                    .child(spacer())
                    .child(new_session_button(p)),
            );
        } else if !s.launch.waiting {
            list = list.child(
                row()
                    .key("no-sessions-yet")
                    .h(40.0)
                    .pl(12.0)
                    .center_y()
                    .child(text("No sessions yet").font(UI).color(p.muted))
                    .child(spacer())
                    .child(new_session_button(p)),
            );
        }
    }

    let placeholder = selected.map_or_else(|| "Search sessions".into(), |proj| format!("Search sessions in {}", proj.name));
    let mut search = row()
        .key("search")
        .grow(1.0)
        .shrink()
        .h(36.0)
        .pl(12.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.search_bg)
        .hover(p.layer2)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            text_input(&s.search, UI.lh(20.0))
                .key("search-input")
                .single_line()
                .grow(1.0)
                .shrink()
                .h(20.0)
                .placeholder(placeholder, p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.search)
                .on_input_key(|s: &mut App, ev, tx| s.home_search_key(ev, tx)),
        );
    if !s.search.text.is_empty() {
        search = search.child(
            row()
                .key("search-clear")
                .role("button")
                .label("Clear search")
                .help("Clear search")
                .size(22.0, 22.0)
                .center()
                .radius(4.0)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.clear_home_search(true, tx))
                .child(icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted)),
        );
    }

    let effective_w = if panel_w > 0.0 {
        panel_w
    } else {
        1280.0 / s.zoom.max(1e-3) - if s.sidebar_open { s.sidebar_w } else { 0.0 } - 16.0
    };
    let wide = effective_w >= WIDE_HOME_MIN_W;

    let body = if wide {
        row()
            .key("home-grid")
            .w_full()
            .max_w(1000.0)
            .gap(32.0)
            .child(projects_column(s, p, selected))
            .child(col().key("home-column").grow(1.0).shrink().min_w(0.0).max_w(720.0).child(search).child(list.mt(12.0)))
    } else {
        let top = row().key("home-bar").w_full().gap(8.0).center_y().child(projects_dropdown_trigger(s, p, selected)).child(search);
        col().key("home-column").w_full().max_w(720.0).child(top).child(list.mt(12.0))
    };

    col()
        .key("home")
        .grow(1.0)
        .w_full()
        .scroll(ScrollSpec { stick_bottom: false })
        .clip_soft(0.0)
        .child(col().key("home-content").w_full().center().px(24.0).pt(48.0).child(body))
}

fn new_session_button(p: &Pal) -> El {
    row()
        .key("new-session")
        .role("button")
        .h(28.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.new_session(tx))
        .child(icon(&ic::EDIT, 16.0).color(p.icon_muted))
        .child(text("New session").font(UI_MED).color(p.muted))
}

fn projects_column(s: &App, p: &Pal, selected: Option<&model::Project>) -> El {
    let used = s.used_projects();
    let header = row()
        .key("home-projects-header")
        .w_full()
        .h(36.0)
        .pl(6.0)
        .pr(2.0)
        .center_y()
        .child(text("Projects").font(UI_MED).color(p.muted))
        .child(spacer())
        .child(
            el().key("home-add-project")
                .role("button")
                .label("Add project")
                .size(28.0, 28.0)
                .radius(6.0)
                .center()
                .hover(p.hover)
                .press(p.pressed)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.open_project_dialog(tx))
                .child(icon(&ic::PLUS, 16.0).color(p.icon_muted)),
        );

    let mut rows = col().key("home-projects-list").w_full().gap(2.0).mt(12.0);
    if used.is_empty() {
        rows = rows.child(
            row()
                .key("home-add-project-empty")
                .role("button")
                .label("Add project")
                .w_full()
                .h(28.0)
                .px(6.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .hover(p.hover)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.open_project_dialog(tx))
                .child(icon(&ic::PLUS, 16.0).color(p.icon_muted))
                .child(text("Add project").font(UI).color(p.muted)),
        );
    } else {
        for proj in used {
            rows = rows.child(home_project_row(s, p, proj, selected.is_some_and(|sel| sel.id == proj.id)));
        }
    }
    col().key("home-projects").w(216.0).child(header).child(rows)
}

fn home_project_row(s: &App, p: &Pal, proj: &model::Project, active: bool) -> El {
    let id = proj.id.clone();
    let menu_id = proj.id.clone();
    let menu_dir = proj.directory.clone();
    let new_dir = proj.directory.clone();
    let mut r = row()
        .key(format!("hp-{}", proj.id))
        .role("button")
        .label(proj.name.clone())
        .w_full()
        .h(28.0)
        .pl(6.0)
        .pr(4.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .hover(if active { p.pressed } else { p.hover })
        .press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.toggle_home_project(&id, tx))
        .on_secondary_press(move |_: &mut App, press: rune::PointerPress, tx: &mut Tx| {
            let menu = rune::PopupMenu::new()
                .item(rune::PopupItem::new("new-session", "New session"))
                .separator()
                .item(rune::PopupItem::new("close", "Close"));
            let pid = menu_id.clone();
            let dir = menu_dir.clone();
            tx.popup_menu(menu, press, move |s: &mut App, picked, tx| match picked.as_deref() {
                Some("new-session") => s.open_project_dir(&dir, true, tx),
                Some("close") => s.hide_project(&pid, tx),
                _ => {}
            });
        })
        .child(crate::icons::badge(s, p, Some(proj), &proj.name))
        .child(
            text(proj.name.clone())
                .font(if active { UI_MED } else { UI })
                .color(if active { p.text } else { p.muted })
                .grow(1.0)
                .shrink()
                .truncate_fade(),
        )
        .child(
            el().key(format!("hp-new-{}", proj.id))
                .role("button")
                .label("New session")
                .size(22.0, 22.0)
                .radius(4.0)
                .center()
                .hover(p.hover)
                .group_opacity(0.0, 1.0)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_project_dir(&new_dir, true, tx))
                .child(icon(&ic::EDIT, 14.0).color(p.icon_muted)),
        );
    if active {
        r = r.fill(p.pressed);
    }
    r
}

fn projects_dropdown_trigger(s: &App, p: &Pal, selected: Option<&model::Project>) -> El {
    let open = s.project_state.home_menu_open;
    let lead = match selected {
        Some(proj) => crate::icons::badge(s, p, Some(proj), &proj.name),
        None => icon(&ic::FOLDER, 16.0).color(p.icon_muted),
    };
    let label = selected.map_or("All projects", |proj| proj.name.as_str());
    row()
        .key("home-projects-trigger")
        .role("button")
        .label("Projects")
        .h(36.0)
        .max_w(180.0)
        .pl(10.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(if open { p.layer2 } else { p.search_bg })
        .hover(p.layer2)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.project_state.home_menu_open = !s.project_state.home_menu_open;
        })
        .child(lead)
        .child(text(label.to_owned()).font(UI_MED).color(if selected.is_some() || open { p.text } else { p.muted }).truncate_fade())
        .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_faint))
}

pub(crate) fn home_projects_popover(s: &App, p: &Pal) -> Option<[El; 2]> {
    if s.route != Route::Home || !s.project_state.home_menu_open {
        return None;
    }
    let selected_id = s.project_state.home_selected.as_deref();
    let used = s.used_projects();

    let mut all_row = row()
        .key("hpm-all")
        .role("menuitemradio")
        .label("All projects")
        .h(28.0)
        .w_full()
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.clear_home_project(tx))
        .child(icon(&ic::FOLDER, 16.0).color(p.icon_muted))
        .child(text("All projects").font(UI).color(p.text))
        .child(spacer());
    if selected_id.is_none() {
        all_row = all_row.child(icon(&ic::CHECK_SMALL, 16.0).color(p.icon));
    }

    let mut list = col().key("home-projects-options").w_full();
    let mut height = 0.0_f32;
    for proj in &used {
        let id = proj.id.clone();
        let active = selected_id == Some(proj.id.as_str());
        let mut item = row()
            .key(format!("hpm-{}", proj.id))
            .role("menuitemradio")
            .label(proj.name.clone())
            .h(28.0)
            .w_full()
            .px(10.0)
            .gap(8.0)
            .center_y()
            .radius(4.0)
            .hover(p.hover)
            .press(p.pressed)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.toggle_home_project(&id, tx))
            .child(crate::icons::badge(s, p, Some(proj), &proj.name))
            .child(text(proj.name.clone()).font(UI).color(p.text).truncate_fade())
            .child(spacer());
        if active {
            item = item.child(icon(&ic::CHECK_SMALL, 16.0).color(p.icon));
        }
        list = list.child(item);
        height += 28.0;
    }

    let add_row = row()
        .key("hpm-add")
        .role("menuitem")
        .label("Add project")
        .h(28.0)
        .w_full()
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.open_project_dialog(tx))
        .child(icon(&ic::PLUS, 16.0).color(p.icon_muted))
        .child(text("Add project").font(UI).color(p.text))
        .child(spacer())
        .child(text("⌘O").font(UI).color(p.faint));

    let mut menu = col()
        .key("home-projects-popover")
        .role("menu")
        .label("Projects")
        .anchored("home-projects-trigger", Side::Bottom, Align::Start, 6.0)
        .w(232.0)
        .padding(2.0)
        .radius(6.0)
        .fill(p.layer1)
        .border(0.5, p.border_muted)
        .shadow(4.0, 12.0, p.shadow)
        .transition(rune::Transition::none())
        .motion(Spring::NONE)
        .on_press(|_: &mut App| {})
        .child(all_row);

    if height > 0.0 {
        menu = menu
            .child(el().h(0.5).w_full().my(2.0).fill(p.border_muted))
            .child(
                col()
                    .key("home-projects-scroll")
                    .w_full()
                    .h(height.min(196.0))
                    .scroll(ScrollSpec { stick_bottom: false })
                    .clip_fade(0.0, 8.0, 0.0, 8.0)
                    .child(list),
            );
    }
    menu = menu.child(el().h(0.5).w_full().my(2.0).fill(p.border_muted)).child(add_row);

    let dismiss = el()
        .key("home-projects-dismiss")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.project_state.home_menu_open = false;
        });

    Some([dismiss, menu])
}

/// One session in Home's list: opens on a click, and has Desktop's right-click menu. While it's
/// being renamed the row is the field, and doesn't open.
#[allow(dead_code)]
pub(crate) fn session_row(s: &App, p: &Pal, ses: &SessionInfo, nav_ids: &std::rc::Rc<Vec<String>>, index: usize) -> El {
    session_row_inner(s, p, ses, nav_ids, index, true, false, "")
}

fn session_row_inner(
    s: &App,
    p: &Pal,
    ses: &SessionInfo,
    nav_ids: &std::rc::Rc<Vec<String>>,
    index: usize,
    show_project: bool,
    active: bool,
    query: &str,
) -> El {
    let project = s.project_for_session(ses);
    let proj_name = project_or_dir(project, &ses.directory);
    let id = ses.id.clone();
    let hover_id = id.clone();
    let nav = nav_ids.clone();
    let target = Target::new(&ses.id, Spot::Row);
    let editing = crate::session_menu_view::field(s, p, &target, UI_MED);
    let marked = s.menus.on.as_ref() == Some(&target);
    let mut r = row()
        .key(format!("r-{}", ses.id))
        .role("button")
        .label(ses.title.clone())
        .focusable(true)
        .focus_selected(index == 0)
        .h(40.0)
        .pl(12.0)
        .pr(16.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if !inside || s.search.text.trim().is_empty() {
                tx.live_only();
            }
            if inside {
                s.prefetch_session(&hover_id);
                if !s.search.text.trim().is_empty() {
                    tx.instant();
                    let q = s.search.text.clone();
                    s.home_search.activate(hover_id.clone(), &q);
                }
            }
        })
        .on_key(move |s: &mut App, ev, _: &mut Tx| s.prefetch_navigation(&nav, index, ev))
        .child(crate::icons::badge(s, p, project, &proj_name));
    if active {
        r = r.fill(p.row_active);
    } else if marked {
        r = r.fill(p.hover);
    }
    let title = title_of(ses);
    let title_el = if query.is_empty() {
        text(title).font(UI_MED).color(p.text).truncate_fade()
    } else {
        crate::switcher::highlighted(&title, query, UI_MED, p.text, p.accent)
    };
    r = match editing {
        Some(field) => r.child(field.grow(1.0)),
        None => r
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                if s.session(&id).is_none()
                    && let Some(ses) = s.home_search.loaded.iter().find(|x| x.id == id).cloned()
                {
                    s.sessions.push(ses);
                }
                s.clear_home_search(false, tx);
                s.open(&id, tx);
            })
            .child(title_el),
    };
    if show_project {
        let proj_el = if query.is_empty() {
            text(proj_name).font(UI).color(p.faint).truncate_fade()
        } else {
            crate::switcher::highlighted(&proj_name, query, UI, p.faint, p.accent)
        };
        r = r.child(proj_el);
    }
    crate::session_menu::on(
        r.child(spacer()).child(text(model::ago(s.now_ms, ses.updated)).font(UI).color(p.faint)),
        target,
    )
}

const SERVE_CMD: &str = "opencode serve --port 4096";

fn no_server_home(s: &App, p: &Pal) -> El {
    let addr = match &s.source {
        crate::Source::Live { server, .. } => server.base.as_str(),
        crate::Source::Fixture => opencode_kit::client::DEFAULT_SERVER,
    };
    let checking = s.launch.waiting;
    let copy_key = "no-server-copy";
    let cmd_row = row()
        .key("no-server-cmd")
        .w_full()
        .h(36.0)
        .pl(12.0)
        .pr(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .child(text(SERVE_CMD).font(CODE).color(p.text).grow(1.0).shrink().truncate_fade())
        .child(
            row()
                .key(copy_key)
                .role("button")
                .label("Copy command")
                .help("Copy command")
                .size(24.0, 24.0)
                .center()
                .radius(4.0)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.copy(SERVE_CMD);
                    s.mark_copied(copy_key, tx);
                })
                .child(crate::copied::glyph(s, copy_key, 14.0, p.icon_muted)),
        );

    let mut retry = row()
        .key("no-server-retry")
        .role("button")
        .label(if checking { "Checking for server" } else { "Retry connection" })
        .h(30.0)
        .px(12.0)
        .center()
        .radius(6.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .hover(if checking { p.bg_base } else { p.hover })
        .hover_motion(INSTANT, INSTANT)
        .child(text(if checking { "Checking…" } else { "Retry" }).font(UI_MED).color(if checking { p.faint } else { p.text }));
    if !checking {
        retry = retry.press(p.pressed).on_press_tx(|s: &mut App, tx: &mut Tx| crate::launch::retry(s, tx));
    }

    let card = col()
        .key("no-server")
        .w_full()
        .max_w(460.0)
        .padding(20.0)
        .gap(14.0)
        .radius(10.0)
        .fill(p.layer2)
        .border(0.5, p.ring)
        .child(
            col()
                .w_full()
                .gap(6.0)
                .child(text("No OpenCode server found").font(H2).color(p.text))
                .child(
                    text(format!("Start OpenCode Desktop or run a local server at {addr}, then retry."))
                        .font(PROMPT)
                        .color(p.muted)
                        .wrap(),
                ),
        )
        .child(cmd_row)
        .child(row().w_full().child(retry));

    col()
        .key("home")
        .grow(1.0)
        .w_full()
        .scroll(ScrollSpec { stick_bottom: false })
        .clip_soft(0.0)
        .child(col().key("home-content").w_full().center().px(24.0).pt(96.0).child(card))
}

mod no_server_story {
    use rune::{El, Play, col};

    use crate::App;
    use crate::palette::Pal;
    use crate::stories::{Story, Trigger, replay, stage};

    const VARIANTS: &[(&str, usize)] = &[("Disconnected", 0), ("Checking", 1)];
    const KEYS: &[Trigger] = &[
        replay(open),
        Trigger {
            key: 'c',
            label: "Toggle checking",
            run: |s, tx| {
                tx.instant();
                s.launch.waiting = !s.launch.waiting;
            },
        },
    ];

    fn setup(s: &mut App) {
        s.route = crate::Route::Home;
        s.sessions.clear();
        s.tabs.clear();
        s.projects.clear();
        s.launch.offline = true;
        s.launch.waiting = s.variant == 1;
    }

    fn open(s: &mut App, tx: &mut Play<App>) {
        setup(s);
        tx.instant();
    }

    fn view(s: &App, p: &Pal) -> El {
        col()
            .key("story-no-server")
            .w(760.0)
            .h(440.0)
            .radius(10.0)
            .fill(p.bg_base)
            .border(0.5, p.ring)
            .clip_soft(0.0)
            .child(super::home(s, p, 760.0))
    }

    rune_register::register!(crate::stories::STORY_LIST: Story = [
        Story { path: "Home/No server", scene: |s| { setup(s); stage(s, view); }, knobs: &[], keys: KEYS, variants: VARIANTS },
    ]);

    rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
        ("no-server", setup),
    ]);
}
