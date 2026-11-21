//! Project navigation on desktop (`home/projects/{landing,start,browse-view}.tsx`, e85035a36c).
use super::*;
use crate::{servers::runtime, widgets};
use opencode_kit::servers::ServerKey;
use rune::{Key, KeyEvent, TextEdit};
use rune_kit::{ButtonVariant, TextInput};

pub const OVERLAY_ID: &str = "dialog.home_browse";

pub(crate) use crate::projects::home::Browser;

enum Event {
    Browse { restore_focus: Option<String>, restore_composer: bool },
    Project {
        server: ServerKey,
        project: model::Project,
        sessions: Vec<(String, String)>,
        restore_focus: Option<String>,
        restore_composer: bool,
    },
    Close,
}

fn step(state: &mut Browser, event: Event) {
    *state = match event {
        Event::Close => Browser::Closed,
        Event::Browse { restore_focus, restore_composer } => Browser::Projects {
            search: TextEdit::default(),
            selected: 0,
            restore_focus,
            restore_composer,
        },
        Event::Project { server, project, sessions, restore_focus, restore_composer } => Browser::History {
            search: TextEdit::default(),
            server,
            project,
            sessions,
            selected: 0,
            restore_focus,
            restore_composer,
        },
    };
}

fn capture_restore(s: &App, tx: &Tx) -> (Option<String>, bool) {
    match &s.kernel.slots.get(&crate::projects::HOME_STATE).browser {
        Browser::Projects { restore_focus, restore_composer, .. }
        | Browser::History { restore_focus, restore_composer, .. } => (restore_focus.clone(), *restore_composer),
        Browser::Closed => (tx.focused().map(str::to_owned), s.composer.focused),
    }
}

fn dismiss(s: &mut App, restore: bool, tx: &mut Tx) {
    let saved = match &s.kernel.slots.get(&crate::projects::HOME_STATE).browser {
        Browser::Projects { restore_focus, restore_composer, .. }
        | Browser::History { restore_focus, restore_composer, .. } => (restore_focus.clone(), *restore_composer),
        Browser::Closed => (None, false),
    };
    step(&mut s.kernel.slots.get_mut(&crate::projects::HOME_STATE).browser, Event::Close);
    opencode_kernel::AppOverlays::on_overlay_closed(s, OVERLAY_ID);
    tx.instant();
    if !restore {
        return;
    }
    let (return_focus, restore_composer) = saved;
    if restore_composer && s.has_composer() && crate::auto_focus_composer(tx) {
        s.composer.focused = true;
        tx.return_focus("prompt", tx.focus_visible());
    } else if let Some(key) = return_focus {
        tx.return_focus(key, tx.focus_visible());
    }
}

pub(crate) fn close(s: &mut App, tx: &mut Tx) {
    dismiss(s, true, tx);
}

pub(crate) fn open(s: &mut App, tx: &mut Tx) {
    let (restore_focus, restore_composer) = capture_restore(s, tx);
    s.composer.focused = false;
    step(&mut s.kernel.slots.get_mut(&crate::projects::HOME_STATE).browser, Event::Browse {
        restore_focus: restore_focus.clone(),
        restore_composer,
    });
    opencode_kernel::AppOverlays::on_overlay_opened(s, OVERLAY_ID, restore_focus);
    tx.instant();
    tx.focus("browse-query", false);
}

pub(crate) fn history(s: &mut App, key: &ServerKey, project: &model::Project, tx: &mut Tx) {
    let (restore_focus, restore_composer) = capture_restore(s, tx);
    let sessions = runtime::with_server(s, key, |s| {
        let mut rows: Vec<_> = s.sessions.iter().filter(|i| i.archived.is_none() && i.parent.is_none() && s.project_for_session(i).is_some_and(|p| p.id == project.id)).collect();
        rows.sort_by_key(|i| std::cmp::Reverse(s.recent_page.active_ms(i)));
        rows.into_iter().map(|i| (i.id.clone(), title_of(i).to_owned())).collect()
    }).unwrap_or_default();
    s.composer.focused = false;
    step(&mut s.kernel.slots.get_mut(&crate::projects::HOME_STATE).browser, Event::Project {
        server: key.clone(),
        project: project.clone(),
        sessions,
        restore_focus: restore_focus.clone(),
        restore_composer,
    });
    opencode_kernel::AppOverlays::on_overlay_opened(s, OVERLAY_ID, restore_focus);
    tx.instant();
    tx.focus("browse-query", false);
}

enum BrowseRow {
    Project { key: String, label: String, server: ServerKey, project: model::Project },
    Back { key: String, label: String },
    NewSession { key: String, label: String, server: ServerKey, directory: String },
    Session { key: String, label: String, server: ServerKey, id: String },
}

impl BrowseRow {
    fn key(&self) -> &str {
        match self {
            Self::Project { key, .. } | Self::Back { key, .. } | Self::NewSession { key, .. } | Self::Session { key, .. } => key,
        }
    }
}

enum BrowseItem {
    Header(String),
    Row(BrowseRow),
}

fn collect_items(s: &App) -> (Vec<BrowseItem>, usize) {
    let home = s.kernel.slots.get(&crate::projects::HOME_STATE);
    match &home.browser {
        Browser::Closed => (Vec::new(), 0),
        Browser::Projects { search, selected, .. } => {
            let query = search.text.to_lowercase();
            let mut items = Vec::new();
            let push_projects = |items: &mut Vec<BrowseItem>, key: &ServerKey, projects: Vec<&model::Project>| {
                for project in projects {
                    if !project.name.to_lowercase().contains(&query) && !project.directory.to_lowercase().contains(&query) {
                        continue;
                    }
                    items.push(BrowseItem::Row(BrowseRow::Project {
                        key: format!("browse-{key}-{}", project.id),
                        label: project.name.clone(),
                        server: key.clone(),
                        project: project.clone(),
                    }));
                }
            };
            if home.servers.is_empty() {
                push_projects(&mut items, runtime::active(s), s.used_projects());
            } else {
                for server in &home.servers {
                    if home.servers.len() > 1 {
                        items.push(BrowseItem::Header(server.label.clone()));
                    }
                    push_projects(&mut items, &server.key, server.projects.iter().collect());
                }
            }
            (items, *selected)
        }
        Browser::History { search, server, project, sessions, selected, .. } => {
            let query = search.text.to_lowercase();
            let mut items = vec![
                BrowseItem::Row(BrowseRow::Back { key: "browse-back".into(), label: t!("project.browse.back").into() }),
                BrowseItem::Row(BrowseRow::NewSession {
                    key: "browse-new".into(),
                    label: t!("project.browse.newSession").into(),
                    server: server.clone(),
                    directory: project.directory.clone(),
                }),
            ];
            for (id, title) in sessions.iter().filter(|(_, title)| title.to_lowercase().contains(&query)) {
                items.push(BrowseItem::Row(BrowseRow::Session {
                    key: format!("browse-{id}"),
                    label: title.clone(),
                    server: server.clone(),
                    id: id.clone(),
                }));
            }
            (items, *selected)
        }
    }
}

fn activate_row(s: &mut App, row: BrowseRow, tx: &mut Tx) {
    match row {
        BrowseRow::Project { server, project, .. } => history(s, &server, &project, tx),
        BrowseRow::Back { .. } => open(s, tx),
        BrowseRow::NewSession { server, directory, .. } => {
            dismiss(s, false, tx);
            runtime::switch(s, &server);
            s.new_session_in(&directory, tx);
        }
        BrowseRow::Session { server, id, .. } => {
            dismiss(s, false, tx);
            runtime::switch(s, &server);
            s.open(&id, tx);
        }
    }
}

pub(crate) fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if matches!(s.kernel.slots.get(&crate::projects::HOME_STATE).browser, Browser::Closed) {
        return false;
    }
    if ev.key == Key::Escape {
        close(s, tx);
        return true;
    }
    let (items, selected) = collect_items(s);
    let rows: Vec<BrowseRow> = items
        .into_iter()
        .filter_map(|item| match item {
            BrowseItem::Row(r) => Some(r),
            BrowseItem::Header(_) => None,
        })
        .collect();
    let count = rows.len();
    let step_dir = match ev.key {
        Key::Down => Some(1isize),
        Key::Up => Some(-1isize),
        Key::Text(ref c) if ev.mods.ctrl && !ev.mods.alt && !ev.mods.cmd && c == "n" => Some(1),
        Key::Text(ref c) if ev.mods.ctrl && !ev.mods.alt && !ev.mods.cmd && c == "p" => Some(-1),
        _ => None,
    };
    if let Some(delta) = step_dir {
        if count > 0 {
            let cur = selected.min(count - 1);
            let next = (cur as isize + delta).rem_euclid(count as isize) as usize;
            let row_key = rows[next].key().to_owned();
            match &mut s.kernel.slots.get_mut(&crate::projects::HOME_STATE).browser {
                Browser::Projects { selected, .. } | Browser::History { selected, .. } => *selected = next,
                Browser::Closed => {}
            }
            tx.instant();
            tx.scroll_into_view_in("browse-projects-scroll", row_key);
        }
        return true;
    }
    if ev.key == Key::Enter {
        if let Some(chosen) = rows.into_iter().nth(selected.min(count.saturating_sub(1))) {
            activate_row(s, chosen, tx);
        }
        return true;
    }
    false
}

fn option(p: &Pal, key: String, label: String, active: bool) -> El {
    let mut r = row()
        .key(key)
        .role("option")
        .label(label.clone())
        .focus_selected(active)
        .w_full()
        .h(32.0)
        .px(8.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .hover(if active { rune::Color::TRANSPARENT } else { p.hover })
        .child(text(label).font(UI).color(p.text).truncate());
    if active {
        r = r.fill(p.hover);
    }
    r
}

pub(crate) fn view(s: &App, p: &Pal, cx: &rune::Cx<App>) -> Vec<El> {
    let kit = crate::palette::kit(p);
    let home = s.kernel.slots.get(&crate::projects::HOME_STATE);
    let (edit, hint) = match &home.browser {
        Browser::Closed => return vec![],
        Browser::Projects { search, .. } => (search, t!("session.new.project.search").to_owned()),
        Browser::History { search, project, .. } => (search, t!("home.sessions.search.placeholder.scoped", scope = &project.name)),
    };
    let (items, selected) = collect_items(s);
    let row_count = items.iter().filter(|i| matches!(i, BrowseItem::Row(_))).count();
    let active_index = (row_count > 0).then(|| selected.min(row_count - 1));
    let mut active_key = String::new();
    let mut list = col().w_full().gap(2.0).role("listbox");
    let mut row_idx = 0usize;
    for item in items {
        match item {
            BrowseItem::Header(label) => {
                list = list.child(text(label).font(UI_SMALL).color(p.muted));
            }
            BrowseItem::Row(row_item) => {
                let is_active = active_index == Some(row_idx);
                if is_active {
                    active_key = row_item.key().to_owned();
                }
                let el = match row_item {
                    BrowseRow::Project { key, label, server, project } => option(p, key, label, is_active)
                        .on_press_tx(move |s: &mut App, tx| history(s, &server, &project, tx)),
                    BrowseRow::Back { key, label } => option(p, key, label, is_active).on_press_tx(open),
                    BrowseRow::NewSession { key, label, server, directory } => option(p, key, label, is_active)
                        .on_press_tx(move |s: &mut App, tx| {
                            dismiss(s, false, tx);
                            runtime::switch(s, &server);
                            s.new_session_in(&directory, tx);
                        }),
                    BrowseRow::Session { key, label, server, id } => option(p, key, label, is_active)
                        .on_press_tx(move |s: &mut App, tx| {
                            dismiss(s, false, tx);
                            runtime::switch(s, &server);
                            s.open(&id, tx);
                        }),
                };
                list = list.child(el);
                row_idx += 1;
            }
        }
    }
    let field = TextInput::new(edit)
        .width(None)
        .placeholder(hint.clone())
        .label(hint.clone())
        .active_descendant((!active_key.is_empty()).then_some(active_key))
        .on_edit(|s: &mut App| match &mut s.kernel.slots.get_mut(&crate::projects::HOME_STATE).browser {
            Browser::Projects { search, .. } | Browser::History { search, .. } => search,
            Browser::Closed => unreachable!("browser field is mounted while open"),
        })
        .on_change(|s: &mut App, _: &mut Tx| match &mut s.kernel.slots.get_mut(&crate::projects::HOME_STATE).browser {
            Browser::Projects { selected, .. } | Browser::History { selected, .. } => *selected = 0,
            Browser::Closed => {}
        })
        .on_key(key)
        .build(kit)
        .key("browse-query");
    let viewport = col().w_full().min_h(0.0).shrink().clip_soft(0.0);
    let scroll = session_view::scroller::scroller("browse-projects-scroll").build::<App>(p, viewport, list.label(hint));
    let card = col().role("dialog").label_static(t!("project.browse.title")).padding(16.0).gap(10.0).radius(8.0).fill(p.layer1).elevate(p.overlay)
        .modal_tx(true, close)
        .child(text(t!("project.browse.title")).font(UI_MED).color(p.text)).child(field).child(scroll);
    widgets::sized_modal("browse-projects", p, widgets::DialogSize::Normal, widgets::Fit::Anchored, widgets::dialog_window(cx, *s.zoom), card)
}

enum Start<'a> { Loading, Add, Reconnect, New(&'a model::Project) }

fn target(s: &App) -> Start<'_> {
    let server = runtime::active(s);
    let home = s.kernel.slots.get(&crate::projects::HOME_STATE);
    let view = home.servers.iter().find(|v| v.key == *server);
    if view.is_some_and(|v| v.failed() || v.sign_in_required()) { return Start::Reconnect; }
    let preferred = home.last_project.as_ref().filter(|(key, _)| key == server);
    if let Some(p) = preferred.and_then(|(_, dir)| s.used_projects().into_iter().find(|p| p.directory == *dir)).or_else(|| s.home_selected()).or_else(|| s.last_used_project()).or_else(|| s.used_projects().into_iter().next()) {
        return Start::New(p);
    }
    if s.live() && !view.is_some_and(|v| v.loaded) { Start::Loading } else { Start::Add }
}

pub(crate) fn start(s: &mut App, tx: &mut Tx) {
    match target(s) {
        Start::Loading => {}
        Start::Add => super::servers::choose_project(s, &runtime::active(s).clone(), tx),
        Start::Reconnect => s.open_settings_page(crate::settings::pages::Page::Servers, tx),
        Start::New(project) => { let dir = project.directory.clone(); s.new_session_in(&dir, tx); }
    }
}

pub(super) fn landing(s: &App, p: &Pal) -> El {
    let target = target(s);
    let label = match target { Start::Loading => t!("common.loading"), Start::Add => t!("session.new.project.add"), Start::Reconnect => t!("project.start.reconnect"), Start::New(_) => t!("command.session.new") };
    let kit = crate::palette::kit(p);
    let key = runtime::active(s);
    let server = s.kernel.slots.get(&crate::projects::HOME_STATE).servers.iter().find(|v| v.key == *key).map(|v| v.label.as_str()).unwrap_or(key.as_str());
    let caption = match target { Start::New(p) => format!("{} · {server}", p.name), _ => server.to_owned() };
    col().key("project-landing").w_full().h_full().grow(1.0).center().gap(12.0)
        .child(widgets::button(label, ButtonVariant::Neutral).disabled(matches!(target, Start::Loading)).on_press_tx(start).build(kit))
        .child(text(caption).font(UI).color(p.muted))
        .child(widgets::button(t!("project.browse.title"), ButtonVariant::Ghost).on_press_tx(open).build(kit))
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("sidebar-customize-landing", |s| {
        crate::scenes::side_tabs(s);
        s.settings.sidebar_organize = crate::settings::Organize::Customize;
        s.tabs.clear();
        *s.route = Route::Home;
    }),
]);
