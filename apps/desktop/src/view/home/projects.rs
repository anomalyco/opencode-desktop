use super::*;
use crate::popup::{self, Item, Menu};
use opencode_i18n::t;

// ---------------------------------------------------------------------------------------------
// Home's projects (`home/projects/view.tsx`, `view.css`, `menu.tsx`): the column beside the
// sessions, its rows with their hover actions and menu, and Settings and Help under them; on a
// narrow window the same rows in a dropdown.

/// `HomeProjectsPanel`. Wide: 280 pt, 56 down and sticking there (`lg:sticky lg:top-14 lg:mt-14`),
/// its header 52 below that, on the first group's line (`lg:pt-[52px]`), and Settings and Help
/// under the list; as tall as the page leaves under those 56 (`lg:h-[calc(100cqh-56px)]`, the page
/// `page_h` tall when known), its rows scrolling inside it, so it stays put at any height.
/// Stacked above the sessions: 10 down (`mt-2.5`), Settings and Help at the foot.
pub(super) fn column(s: &App, p: &Pal, selected: Option<&model::Project>, wide: bool, page_h: f32) -> El {
    let used = s.used_projects();
    // `h-7 pl-1.5 pr-3`: Projects in muted 530, and the large ghost-muted folder-add button.
    let header = row()
        .key("home-projects-header")
        .w_full()
        .h(28.0)
        .pl(6.0)
        .pr(12.0)
        .center_y()
        .child(text(t!("home.projects")).font(UI_MED).color(p.muted))
        .child(spacer())
        .child(
            el().key("home-add-project")
                .role("button")
                .label(t!("home.project.add"))
                .help(t!("home.project.add"))
                .size(28.0, 28.0)
                .radius(6.0)
                .center()
                .hover(p.hover)
                .press(p.pressed)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.open_project_dialog(tx))
                .child(icon(&ic::FOLDER_ADD_LEFT, 16.0).color(p.icon_muted)),
        );
    // `pr-3`, rows `gap-1`.
    let mut rows = col().key("home-projects-list").w_full().pr(12.0).gap(4.0);
    if used.is_empty() {
        rows = rows.child(nav_row(p, "home-add-project-empty", &ic::FOLDER_ADD_LEFT, t!("home.project.add")).w_full().on_press_tx(|s: &mut App, tx: &mut Tx| s.open_project_dialog(tx)));
    } else {
        for proj in used {
            rows = rows.child(project_row(s, p, proj, selected.is_some_and(|sel| sel.id == proj.id), PROJECTS_W - 12.0));
        }
    }
    if !wide {
        return col().key("home-projects").w_full().mt(10.0).gap(16.0).child(header).child(rows);
    }
    // `home-projects-viewport`: the rows scroll in what the column leaves them, their edges
    // masked 16 pt while rows hide past them (`index.css:353-376`).
    let rows = col().key("home-projects-scroll").w_full().shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false })
        .scroll_fade(rune::ScrollFade::strength(16.0).linear().full_at(0.01)).child(rows);
    let c = col().key("home-projects").w(PROJECTS_W).gap(16.0).child(header).child(rows);
    let c = if page_h > 56.0 { c.h(page_h - 56.0).self_start() } else { c };
    // `HomeUtilityNav` `mb-8 mt-4`, after the panel's `gap-4`.
    c.mt(56.0).pt(52.0).sticky_top(56.0).child(utility(p, true).mt(16.0).mb(32.0))
}

/// `HomeUtilityNav`: Settings and Help. Under the projects (`lg:flex-col lg:pr-3`, full-width
/// rows `gap-1`), or along Home's foot, one at each end.
pub(super) fn utility(p: &Pal, stacked: bool) -> El {
    let settings = nav_row(p, "home-settings", &ic::SETTINGS_GEAR, t!("sidebar.settings")).on_press_tx(|s: &mut App, tx: &mut Tx| {
        if !s.settings.open {
            s.toggle_settings(tx);
        }
    });
    let help = nav_row(p, "home-help", &ic::HELP, t!("sidebar.help")).on_press_tx(|_: &mut App, tx: &mut Tx| {
        let _ = tx.open_url("https://opencode.ai/desktop-feedback");
    });
    if stacked {
        return col().key("home-utility").w_full().pr(12.0).gap(4.0).child(settings.w_full()).child(help.w_full());
    }
    // `[&>button]:w-auto`: each as wide as its icon and label.
    row().w_full().gap(4.0).justify_between().child(settings).child(help)
}

/// `HomeProjectNavButton`: 28 tall, `px-1.5 gap-2`, the small (14 pt) icon in icon-muted, the
/// label muted at 440 going to the text colour over the layer-01 hover.
fn nav_row(p: &Pal, key: &str, glyph: &'static Icon, label: &str) -> El {
    row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .hover(p.layer1)
        .child(icon(glyph, 14.0).key("icon").color(p.icon_muted))
        .child(text(label).font(UI).color(p.muted).group_color(p.text).shrink().truncate())
}

/// The project menu's trigger: the menu opens below its end, 6 away (`gutter={6}`).
fn menu_key(id: &str) -> String {
    format!("hp-menu-{id}")
}

/// `HomeProjectRow` in a column `width` wide: the avatar and name as the nav button (layer-01
/// hovered, layer-03 selected), and at its end, while hovered or its menu is open, More options
/// and New session (small ghost-muted IconButtons, `right-1 gap-1 pl-2`) over an 8 pt fade into
/// the row's surface. A click scopes Home to it; a right-click opens its menu.
fn project_row(s: &App, p: &Pal, proj: &model::Project, active: bool, width: f32) -> El {
    let menu = menu_key(&proj.id);
    let menu_open = s.popup.open_at(&menu);
    let (id, menu_id, ctx_id, new_dir) = (proj.id.clone(), proj.id.clone(), proj.id.clone(), proj.directory.clone());
    let surface = if active { p.layer3 } else { p.layer1 };
    // `pl-2`, two 20 pt buttons `gap-1`: the fade runs over the first 8.
    let actions_w = 8.0 + 20.0 + 4.0 + 20.0;
    let actions = row()
        .key("actions")
        .pin(None, Some(0.0), Some(4.0), Some(0.0))
        .w(actions_w)
        .pl(8.0)
        .gap(4.0)
        .center_y()
        .linear_gradient(std::f32::consts::FRAC_PI_2, [(0.0, surface.alpha(0.0)), (8.0 / actions_w, surface), (1.0, surface)])
        .child(
            icon_button(p, &menu, t!("common.moreOptions"), &ic::OUTLINE_DOTS, menu_open)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| open_menu(s, &menu_id, tx)),
        )
        .child(icon_button(p, &format!("hp-new-{}", proj.id), t!("command.session.new"), &ic::EDIT, false).on_press_tx(move |s: &mut App, tx: &mut Tx| s.new_session_in(&new_dir, tx)));
    // The nav button: hovered itself, its label goes to the text colour; the pointer on the
    // actions leaves it muted while the row keeps its hover surface.
    let nav = row()
        .key("nav")
        .role("button")
        .label(proj.name.clone())
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h(28.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .hover_group()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.toggle_home_project(&id, tx))
        .child(crate::icons::badge(s, p, Some(proj), &proj.name))
        .child(text(proj.name.clone()).key("label").font(UI).color(if active { p.text } else { p.muted }).group_color(p.text).grow(1.0).shrink().truncate());
    let mut r = row()
        .key(format!("hp-{}", proj.id))
        .w(width)
        .h(28.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .hover(surface)
        .on_secondary_press(move |s: &mut App, _: rune::PointerPress, tx: &mut Tx| open_menu(s, &ctx_id, tx))
        .child(nav)
        .child(if menu_open { actions } else { actions.group_opacity(0.0, 1.0) });
    if active || menu_open {
        r = r.fill(surface);
    }
    r
}

/// A small ghost-muted IconButton: 20 pt, `rounded-[4px]`, the 16 pt icon in icon-muted, the
/// overlay hovered and the pressed overlay (with icon-base) pressed or while its menu is open.
pub(crate) fn icon_button(p: &Pal, key: &str, label: &str, glyph: &'static Icon, expanded: bool) -> El {
    let b = el()
        .key(key.to_owned())
        .role("button")
        .label(label)
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .child(icon(glyph, 16.0).color(if expanded { p.icon } else { p.icon_muted }));
    if expanded { b.fill(p.pressed) } else { b }
}

/// `ProjectMenuItems`: New session, Import session, Edit project, Reveal in File Explorer (a
/// local server's), Clear notifications (while any of its sessions is unread), then Close.
fn open_menu(s: &mut App, id: &str, tx: &mut Tx) {
    open_project_menu(s, id, menu_key(id), tx);
}

/// `open_menu` under the element keyed `anchor`.
pub(crate) fn open_project_menu(s: &mut App, id: &str, anchor: String, tx: &mut Tx) {
    let Some(proj) = s.project(id).cloned() else { return };
    let unread = project_unread(s, &proj).next().is_some();
    let local = matches!(&s.source, crate::Source::Live { server, .. } if server.is_local());
    let mut menu = Menu::new(t!("command.category.project"))
        .min_width(160.0)
        .align(Align::End)
        .gap(6.0)
        .item(Item::new("new-session", t!("command.session.new")))
        .item(Item::new("import", t!("command.session.import")).enabled(!s.project_state.importing))
        .item(Item::new("edit", t!("dialog.project.edit.title")));
    if local {
        menu = menu.item(Item::new("reveal", super::super::location_card::reveal_label()));
    }
    menu = menu.item(Item::new("clear", t!("sidebar.project.clearNotifications")).enabled(unread)).separator().item(Item::new("close", t!("common.close")));
    tx.instant();
    popup::open_drawn(s, menu, anchor, tx, move |s: &mut App, picked, tx| match picked {
        Some("new-session") => s.new_session_in(&proj.directory, tx),
        Some("import") => s.import_session(&proj.directory, tx),
        Some("edit") => s.edit_project(&proj.directory, tx),
        Some("reveal") => {
            if let Err(e) = super::super::location_card::reveal(&proj.directory) {
                crate::status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), e);
            }
        }
        Some("clear") => s.clear_project_notifications(&proj, tx),
        Some("close") => s.hide_project(&proj.id, tx),
        _ => {}
    });
}

/// `proj`'s unread root sessions among those this window has loaded, in its folder
/// (`notification.project.hasUnread`: the data store's sessions, not Home's index).
fn project_unread<'a>(s: &'a App, proj: &'a model::Project) -> impl Iterator<Item = &'a SessionInfo> + 'a {
    let dir = crate::projects::clean_dir(&proj.directory);
    s.sessions.iter().filter(move |i| {
        i.parent.is_none() && i.archived.is_none() && i.unread() && s.transcripts.contains_key(&i.id) && crate::projects::clean_dir(&i.directory) == dir
    })
}

impl App {
    /// A project's New session: a draft in its folder (`openProjectNewSession`).
    pub(crate) fn new_session_in(&mut self, dir: &str, tx: &mut Tx) {
        self.close_settings();
        self.project_state.home_menu_open = false;
        self.default_dir = dir.to_owned();
        self.opening = None;
        self.open_draft_tab();
        tx.settle();
    }

    /// Whether any of `proj`'s sessions is unread (`notification.project.hasUnread`).
    pub(crate) fn project_has_unread(&self, proj: &model::Project) -> bool {
        project_unread(self, proj).next().is_some()
    }

    /// Clear notifications: its sessions read, here and on the server.
    pub(crate) fn clear_project_notifications(&mut self, proj: &model::Project, tx: &mut Tx) {
        let ids: Vec<String> = project_unread(self, proj).map(|i| i.id.clone()).collect();
        tx.instant();
        for id in ids {
            let Some(info) = self.sessions.iter_mut().find(|i| i.id == id) else { continue };
            info.viewed = info.idle.max(info.viewed);
            let idle = info.idle.unwrap_or(self.now_ms);
            self.write(opencode_kit::client::Write::View { session: id, idle });
        }
    }

    /// Import session: a session export (JSON) picked from disk, imported into `dir` through
    /// `experimental/session/import`, then opened (`home/projects/actions.tsx`).
    fn import_session(&mut self, dir: &str, tx: &mut Tx) {
        let options = rune::FileDialogOptions { title: t!("command.session.import").into(), ..Default::default() };
        let dir = dir.to_owned();
        tx.open_file(options, move |s: &mut App, result, tx: &mut Tx| match result {
            Ok(Some(path)) => s.upload_session(&path, dir.clone(), tx),
            Ok(None) => {}
            Err(e) => crate::status_toast::fail(s, t!("rune.status.filePickerFailed"), e.to_string()),
        });
    }

    fn upload_session(&mut self, path: &std::path::Path, dir: String, tx: &mut Tx) {
        let data = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()));
        let mut data = match data {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => return crate::status_toast::fail(self, t!("common.requestFailed"), t!("rune.home.import.notExport").to_owned()),
            Err(e) => return crate::status_toast::fail(self, t!("common.requestFailed"), e),
        };
        if !self.can_write() {
            return crate::status_toast::fail(self, t!("common.requestFailed"), crate::jobs::read_only().to_owned());
        }
        data.insert("location".into(), serde_json::json!({ "directory": dir }));
        self.project_state.importing = true;
        tx.instant();
        self.job(move |srv| {
            let made = srv.write("POST", "/api/experimental/session/import", Some(serde_json::Value::Object(data)));
            Box::new(move |s: &mut App, tx: &mut Tx| {
                s.project_state.importing = false;
                tx.instant();
                match made {
                    Ok(v) => {
                        let info = opencode_kit::model::parse_session(v.get("data").unwrap_or(&v));
                        let id = info.id.clone();
                        if !s.sessions.iter().any(|x| x.id == id) {
                            s.sessions.insert(0, info);
                        }
                        s.open(&id, tx);
                    }
                    Err(e) => crate::status_toast::fail(s, t!("common.requestFailed"), e),
                }
            }) as crate::jobs::Apply
        });
    }
}

/// The narrow window's projects dropdown (`home-projects-dropdown`): 40 tall and full width,
/// `px-1.5 gap-2`, bg-base going to layer-01 hovered; the selected project's avatar (a small
/// folder for all of them), its name, and a small chevron in icon-muted.
pub(super) fn dropdown_trigger(s: &App, p: &Pal, selected: Option<&model::Project>) -> El {
    let open = s.project_state.home_menu_open;
    let lead = match selected {
        Some(proj) => crate::icons::badge(s, p, Some(proj), &proj.name),
        None => icon(&ic::FOLDER, 14.0).color(p.text),
    };
    let label = selected.map_or(t!("home.projects.all"), |proj| proj.name.as_str());
    row()
        .key("home-projects-trigger")
        .role("button")
        .label(t!("home.projects"))
        .w_full()
        .h(40.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(if open { p.layer1 } else { p.bg_base })
        .hover(p.layer1)
        .on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
            if (s.project_state.trigger_w - r[2]).abs() > 0.5 {
                s.project_state.trigger_w = r[2];
            } else {
                tx.live_only();
            }
        })
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.project_state.home_menu_open = !s.project_state.home_menu_open;
        })
        .child(lead)
        .child(text(label.to_owned()).font(UI).color(p.text).grow(1.0).shrink().truncate())
        .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted))
}

/// The dropdown's popover: as wide as its trigger and 6 below it, `rounded-[10px] bg-base p-1.5`,
/// the floating elevation; All projects, the project rows, then Add project.
pub(crate) fn home_projects_popover(s: &App, p: &Pal) -> Option<[El; 2]> {
    if s.route != Route::Home || !s.project_state.home_menu_open {
        return None;
    }
    let selected_id = s.project_state.home_selected.as_deref();
    let width = s.project_state.trigger_w.max(160.0);
    let mut all = nav_row(p, "hpm-all", &ic::FOLDER, t!("home.projects.all")).w_full().mb(4.0).on_press_tx(|s: &mut App, tx: &mut Tx| s.clear_home_project(tx));
    if selected_id.is_none() {
        all = all.fill(p.layer3);
    }
    let mut list = col().key("home-projects-options").w_full().gap(4.0);
    for proj in s.used_projects() {
        list = list.child(project_row(s, p, proj, selected_id == Some(proj.id.as_str()), width - 12.0));
    }
    let add = nav_row(p, "hpm-add", &ic::FOLDER_ADD_LEFT, t!("home.project.add")).w_full().mt(4.0).on_press_tx(|s: &mut App, tx: &mut Tx| s.open_project_dialog(tx));
    let menu = col()
        .key("home-projects-popover")
        .role("menu")
        .label(t!("home.projects"))
        .anchored("home-projects-trigger", Side::Bottom, Align::Start, 6.0)
        .w(width)
        .padding(6.0)
        .radius(10.0)
        .fill(p.bg_base)
        .elevate(p.floating)
        .transition(rune::Transition::none())
        .motion(Spring::NONE)
        .on_press(|_: &mut App| {})
        .child(all)
        .child(col().key("home-projects-scroll").w_full().max_h(320.0).scroll(ScrollSpec { stick_bottom: false }).scroll_fade(rune::ScrollFade::strength(16.0).linear().full_at(1.0)).child(list))
        .child(add);
    let dismiss = el().key("home-projects-dismiss").absolute(0.0, 0.0).w_full().h_full().on_press_tx(|s: &mut App, tx: &mut Tx| {
        tx.instant();
        s.project_state.home_menu_open = false;
    });
    Some([dismiss, menu])
}
