//! Project navigation on desktop (`home/projects/{landing,start,browse-view}.tsx`, e85035a36c).
use super::*;
use crate::{servers::runtime, widgets};
use opencode_kit::servers::ServerKey;
use rune::TextEdit;
use rune_kit::{ButtonVariant, TextInput};

pub(crate) use crate::projects::home::Browser;

enum Event {
    Browse,
    Project(ServerKey, model::Project, Vec<(String, String)>),
    Close,
}

fn step(state: &mut Browser, event: Event) {
    *state = match event {
        Event::Close => Browser::Closed,
        Event::Browse => Browser::Projects(TextEdit::default()),
        Event::Project(server, project, sessions) => Browser::History { search: TextEdit::default(), server, project, sessions },
    };
}

pub(crate) fn open(s: &mut App, tx: &mut Tx) {
    step(&mut s.home_state.browser, Event::Browse);
    tx.instant();
    tx.focus("browse-query", false);
}

pub(crate) fn history(s: &mut App, key: &ServerKey, project: &model::Project, tx: &mut Tx) {
    let sessions = runtime::with_server(s, key, |s| {
        let mut rows: Vec<_> = s.sessions.iter().filter(|i| i.archived.is_none() && i.parent.is_none() && s.project_for_session(i).is_some_and(|p| p.id == project.id)).collect();
        rows.sort_by_key(|i| std::cmp::Reverse(s.recent_page.active_ms(i)));
        rows.into_iter().map(|i| (i.id.clone(), title_of(i).to_owned())).collect()
    }).unwrap_or_default();
    step(&mut s.home_state.browser, Event::Project(key.clone(), project.clone(), sessions));
    tx.instant();
    tx.focus("browse-query", false);
}

fn option(p: &Pal, key: String, label: String) -> El {
    row().key(key).role("option").label(label.clone()).w_full().h(32.0).px(8.0).center_y().radius(6.0).hover(p.hover)
        .child(text(label).font(UI).color(p.text).truncate())
}

pub(crate) fn view(s: &App, p: &Pal, cx: &rune::Cx<App>) -> Vec<El> {
    let kit = crate::palette::kit(p);
    let mut list = col().w_full().gap(2.0).role("listbox");
    let (edit, hint) = match &s.home_state.browser {
        Browser::Closed => return vec![],
        Browser::Projects(edit) => {
            let query = edit.text.to_lowercase();
            let add = |mut list: El, key: &ServerKey, projects: Vec<&model::Project>| {
                for project in projects {
                    if !project.name.to_lowercase().contains(&query) && !project.directory.to_lowercase().contains(&query) { continue; }
                    let (server, proj) = (key.clone(), project.clone());
                    list = list.child(option(p, format!("browse-{key}-{}", proj.id), proj.name.clone()).on_press_tx(move |s: &mut App, tx| history(s, &server, &proj, tx)));
                }
                list
            };
            if s.home_state.servers.is_empty() {
                list = add(list, runtime::active(s), s.used_projects());
            } else {
                for server in &s.home_state.servers {
                    if s.home_state.servers.len() > 1 { list = list.child(text(server.label.clone()).font(UI_SMALL).color(p.muted)); }
                    list = add(list, &server.key, server.projects.iter().collect());
                }
            }
            (edit, t!("session.new.project.search").to_owned())
        }
        Browser::History { search, server, project, sessions } => {
            let query = search.text.to_lowercase();
            let (key, dir) = (server.clone(), project.directory.clone());
            list = list.child(option(p, "browse-back".into(), t!("project.browse.back").into()).on_press_tx(open))
                .child(option(p, "browse-new".into(), t!("project.browse.newSession").into()).on_press_tx(move |s: &mut App, tx| {
                    step(&mut s.home_state.browser, Event::Close);
                    runtime::switch(s, &key);
                    s.new_session_in(&dir, tx);
                }));
            for (id, title) in sessions.iter().filter(|(_, title)| title.to_lowercase().contains(&query)) {
                let (server, id) = (server.clone(), id.clone());
                list = list.child(option(p, format!("browse-{id}"), title.clone()).on_press_tx(move |s: &mut App, tx| {
                    step(&mut s.home_state.browser, Event::Close);
                    runtime::switch(s, &server);
                    s.open(&id, tx);
                }));
            }
            (search, t!("home.sessions.search.placeholder.scoped", scope = &project.name))
        }
    };
    let field = TextInput::new(edit).width(None).placeholder(hint.clone()).label(hint.clone()).on_edit(|s: &mut App| match &mut s.home_state.browser {
        Browser::Projects(edit) | Browser::History { search: edit, .. } => edit,
        Browser::Closed => unreachable!("browser field is mounted while open"),
    }).build(kit).key("browse-query");
    let viewport = col().w_full().min_h(0.0).shrink().clip_soft(0.0);
    let scroll = session_view::scroller::scroller("browse-projects-scroll").build::<App>(p, viewport, list.label(hint));
    let card = col().role("dialog").label_static(t!("project.browse.title")).padding(16.0).gap(10.0).radius(8.0).fill(p.layer1).elevate(p.overlay)
        .modal_tx(true, |s: &mut App, tx| { step(&mut s.home_state.browser, Event::Close); tx.instant(); })
        .child(text(t!("project.browse.title")).font(UI_MED).color(p.text)).child(field).child(scroll);
    widgets::sized_modal("browse-projects", p, widgets::DialogSize::Normal, widgets::Fit::Anchored, widgets::dialog_window(cx, *s.zoom), card)
}

enum Start<'a> { Loading, Add, Reconnect, New(&'a model::Project) }

fn target(s: &App) -> Start<'_> {
    let server = runtime::active(s);
    let view = s.home_state.servers.iter().find(|v| v.key == *server);
    if view.is_some_and(|v| v.failed() || v.sign_in_required()) { return Start::Reconnect; }
    let preferred = s.home_state.last_project.as_ref().filter(|(key, _)| key == server);
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
    let server = s.home_state.servers.iter().find(|v| v.key == *key).map(|v| v.label.as_str()).unwrap_or(key.as_str());
    let caption = match target { Start::New(p) => format!("{} · {server}", p.name), _ => server.to_owned() };
    col().key("project-landing").w_full().h_full().grow(1.0).center().gap(12.0)
        .child(widgets::button(label, ButtonVariant::Neutral).disabled(matches!(target, Start::Loading)).on_press_tx(start).build(kit))
        .child(text(caption).font(UI).color(p.muted))
        .child(widgets::button(t!("project.browse.title"), ButtonVariant::Ghost).on_press_tx(open).build(kit))
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("sidebar-project-landing", |s| {
        crate::scenes::side_tabs(s);
        s.settings.sidebar_organize = crate::settings::Organize::Projects;
        s.tabs.clear();
        *s.route = Route::Home;
    }),
    ("sidebar-customize-landing", |s| {
        crate::scenes::side_tabs(s);
        s.settings.sidebar_organize = crate::settings::Organize::Customize;
        s.tabs.clear();
        *s.route = Route::Home;
    }),
]);
