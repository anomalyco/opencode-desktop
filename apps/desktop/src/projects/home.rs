//! Home across servers (`home/model.ts`, `home/projects/controller.tsx`,
//! `home/sessions/load-machine.ts`):
//! - **Selection:** which server Home focuses and which of its used projects is selected, run by
//!   MS-1's [`opencode_kit::servers::home`] machine against the servers Home lists. The window
//!   works against the selected server while Home shows (`servers::runtime::desired`).
//! - **Servers:** each listed server's name, badge, health and used projects, read once a poll
//!   ([`Home::servers`]) for the projects column, and each one's collapse state and whether it's
//!   hidden from Home, kept for every window in the app's database (Desktop's global `home.servers` and
//!   `server.hidden`), as is the selection (`layout.home.selection`).
//! - **Session list:** the focused server's list phase, run by the [`home_load`] machine: its
//!   placeholder rows while the server connects, its Unreachable, Signed out and Incompatible
//!   states, and a list already shown staying while its server is away.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use opencode_kit::model::Project;
use opencode_kit::servers::health::{self as server_health, Health, ProbeResult};
use opencode_kit::servers::home::{self as machine, Selection};
use opencode_kit::servers::home_load::{self, Connection, Target, Timer, Unavailable};
use opencode_kit::servers::{ServerConn, ServerKey};
use rune::Tx;
use rune::machine::{Attempt, Effects};
use serde_json::{Value, json};

use crate::{App, Route};

/// One server as Home's projects column shows it.
#[derive(Clone, Debug)]
pub struct ServerView {
    pub key: ServerKey,
    pub conn: ServerConn,
    /// `servers.label`.
    pub label: String,
    pub health: Option<Health>,
    /// Its used projects, most recently active first.
    pub projects: Vec<Project>,
    /// Visible remote project rows, read on their own checkout.
    pub sessions: BTreeMap<String, Vec<(String, String)>>,
    /// Its first reads landed.
    pub loaded: bool,
}

impl ServerView {
    pub fn healthy(&self) -> bool {
        server_health::is_healthy(self.health.as_ref())
    }

    pub fn failed(&self) -> bool {
        server_health::is_failed(self.health.as_ref())
    }

    pub fn incompatible(&self) -> bool {
        server_health::is_incompatible(self.health.as_ref())
    }

    /// Its plugin asks for a sign-in, or an added server rejects its saved one (`signInRequired`).
    pub fn sign_in_required(&self) -> bool {
        match &self.conn {
            ServerConn::Plugin { status, .. } => status.http().is_none() && self.health == Some(Health::Auth),
            ServerConn::Http { .. } => server_health::is_unauthorized(self.health.as_ref()),
            ServerConn::Sidecar { .. } => false,
        }
    }

    /// Its plugin is starting it (`connecting`).
    pub fn connecting(&self) -> bool {
        matches!(&self.conn, ServerConn::Plugin { .. }) && self.health == Some(Health::Starting)
    }
}

/// An armed `home_load` timer and the event it steps when it fires.
#[derive(Clone, Debug, Default, PartialEq)]
enum Armed {
    #[default]
    Idle,
    At { due: f64, event: home_load::Event },
}

/// The project add Home asked a server for, until its project shows among the server's used ones
/// (`project.add` → `project.added`).
#[derive(Clone, Debug, Default, PartialEq)]
enum Adding {
    #[default]
    Idle,
    Waiting { attempt: Attempt, server: ServerKey, directory: String },
}

/// A window's Home.
#[derive(Default)]
pub struct Home {
    pub(crate) last_project: Option<(ServerKey, String)>,
    pub(crate) browser: crate::view::home::browse::Browser,
    landing: Landing,
    machine: machine::State,
    load: home_load::State,
    timers: [Armed; 3],
    adding: Adding,
    /// The servers Home lists, as of the last poll.
    pub servers: Vec<ServerView>,
    /// The store's version the machine last read.
    seen: u64,
}

/// What every window shares, saved in Desktop's global keys.
#[derive(Default)]
struct Store {
    selection: Selection,
    collapsed: BTreeMap<ServerKey, bool>,
    hidden: BTreeSet<ServerKey>,
    version: u64,
}

thread_local! {
    static STORE: RefCell<Store> = RefCell::new(load());
}

/// Desktop's global keys: `layout.home.selection`, `home.servers.collapsed` and `server.hidden`.
const GLOBAL: &str = "opencode.global.dat";

fn load() -> Store {
    let json = |key: &str| crate::storage::json(GLOBAL, key).unwrap_or(Value::Null);
    let (layout, servers, server) = (json("layout"), json("home.servers"), json("server"));
    let key = |v: &Value| v.as_str().filter(|k| !k.is_empty()).map(ServerKey::new);
    let sel = &layout["home"]["selection"];
    let selection = Selection::new(key(&sel["server"]), sel["directory"].as_str().map(str::to_owned));
    let collapsed = servers["collapsed"].as_object().into_iter().flatten().filter_map(|(k, b)| Some((ServerKey::new(k), b.as_bool()?))).collect();
    let hidden = server["hidden"].as_object().into_iter().flatten().filter(|(_, b)| b.as_bool() == Some(true)).map(|(k, _)| ServerKey::new(k)).collect();
    Store { selection, collapsed, hidden, version: 0 }
}

/// Each part goes to its own key; a part that didn't change writes nothing.
fn save(store: &Store) {
    if !crate::attended() {
        return;
    }
    let selection = json!({ "server": store.selection.server.as_ref().map(ServerKey::as_str), "directory": store.selection.directory });
    let collapsed: serde_json::Map<String, Value> = store.collapsed.iter().map(|(k, b)| (k.as_str().to_owned(), json!(b))).collect();
    let hidden: serde_json::Map<String, Value> = store.hidden.iter().map(|k| (k.as_str().to_owned(), json!(true))).collect();
    crate::storage::update(GLOBAL, "layout", |m| {
        let home = m.entry("home").or_insert_with(|| json!({}));
        if let Some(home) = home.as_object_mut() {
            home.insert("selection".into(), selection);
        }
    });
    crate::storage::update(GLOBAL, "home.servers", |m| {
        m.insert("collapsed".into(), Value::Object(collapsed));
    });
    crate::storage::update(GLOBAL, "server", |m| {
        m.insert("hidden".into(), Value::Object(hidden));
    });
}

fn write(f: impl FnOnce(&mut Store)) {
    STORE.with_borrow_mut(|s| {
        f(s);
        s.version += 1;
        save(s);
    });
}

/// The servers hidden from Home ("Hide from project list").
pub fn hidden() -> BTreeSet<ServerKey> {
    STORE.with_borrow(|s| s.hidden.clone())
}

pub fn set_hidden(key: &ServerKey, hide: bool) {
    write(|s| {
        if hide {
            s.hidden.insert(key.clone());
        } else {
            s.hidden.remove(key);
        }
    });
}

/// Server `key`'s projects are collapsed on Home (`home.servers.collapsed`).
pub fn collapsed(key: &ServerKey) -> bool {
    STORE.with_borrow(|s| s.collapsed.get(key).copied().unwrap_or(false))
}

/// The collapse chevron: only a healthy server with projects toggles.
pub fn toggle_collapsed(key: &ServerKey, tx: &mut Tx) {
    tx.instant();
    write(|s| {
        let next = !s.collapsed.get(key).copied().unwrap_or(false);
        s.collapsed.insert(key.clone(), next);
    });
}

/// What Home shows as selected (`selectHomeSelection`).
pub fn selection(s: &App) -> Selection {
    s.home_state.machine.selection()
}

/// The server Home shows: the selection's, else the app's own.
pub fn server(s: &App) -> ServerKey {
    if s.project_navigation() && let Some((key, _)) = &s.home_state.last_project
        && s.home_state.servers.iter().any(|v| v.key == *key) {
        return key.clone();
    }
    selection(s).server.unwrap_or_else(ServerKey::sidecar)
}

impl App {
    /// Home's selected project, on the server the window works against.
    pub fn home_selected(&self) -> Option<&Project> {
        let sel = selection(self);
        if sel.server.as_ref() != Some(crate::servers::runtime::active(self)) {
            return None;
        }
        let dir = machine::comparable_path(sel.directory.as_deref()?);
        self.used_projects().into_iter().find(|p| machine::comparable_path(&p.directory) == dir)
    }

    /// Its id.
    pub fn home_selected_id(&self) -> Option<String> {
        self.home_selected().map(|p| p.id.clone())
    }
}

/// Home focuses server `key`, all of its projects (`focusServer`).
pub fn focus(s: &mut App, key: &ServerKey) {
    send(s, machine::Event::ServerFocused(key.clone()));
}

/// A project row toggles its project on `server` (`project.toggled`).
pub fn toggle(s: &mut App, server: &ServerKey, directory: &str) {
    send(s, machine::Event::ProjectToggled { server: server.clone(), directory: directory.to_owned() });
}

/// Another part of the app selects `directory` on `server` (a session's "Show in Home"): Home
/// takes it as an outside write.
pub fn show(s: &mut App, server: &ServerKey, directory: Option<&str>) {
    let sel = Selection::new(Some(server.clone()), directory.map(str::to_owned));
    write(|st| st.selection = sel);
    sync(s);
}

/// Adds `directories` on `server` as projects; the first one's project is selected once it shows
/// (`home.project.add`).
pub fn add(s: &mut App, server: &ServerKey, directories: Vec<String>, tx: &mut Tx) {
    let fx = step(s, machine::Event::ProjectAdd { server: server.clone(), directories });
    run(s, fx, tx);
}

/// Steps `event` after the servers' latest state, as Desktop's `send` does: a user action can
/// follow a server change in the same update.
fn send(s: &mut App, event: machine::Event) {
    sync(s);
    let fx = step(s, event);
    run(s, fx, &mut Tx::default());
}

fn step(s: &mut App, event: machine::Event) -> Effects<machine::Effect> {
    let before = selection(s).server;
    let fx = machine::step(&mut s.home_state.machine, event);
    follow(s, before);
    fx
}

/// The selection moved to another server while Home shows: the window works against it now.
fn follow(s: &mut App, before: Option<ServerKey>) {
    let after = selection(s).server;
    if after == before || s.route != Route::Home || s.settings.open {
        return;
    }
    if let Some(key) = after {
        crate::servers::runtime::switch(s, &key);
    }
}

/// The servers Home lists, read from each (`env`), and the machine told.
fn sync(s: &mut App) {
    let entries = crate::servers::registry::visible(s);
    let mut servers = Vec::with_capacity(entries.len());
    for e in entries {
        let key = e.key();
        let read = |s: &mut App| {
            let mut sessions = BTreeMap::new();
            let projects: Vec<_> = s.organized_projects().into_iter().cloned().collect();
            for p in &projects {
                if !s.project_state.organize_expanded.contains(&format!("{key}:{}", p.id)) { continue; }
                let mut rows: Vec<_> = s.sessions.iter().filter(|i| i.parent.is_none() && i.archived.is_none() && s.project_for_session(i).is_some_and(|proj| proj.id == p.id)).collect();
                rows.sort_by_key(|i| std::cmp::Reverse(s.recent_page.active_ms(i)));
                sessions.insert(p.id.clone(), rows.into_iter().map(|i| (i.id.clone(), i.title.clone())).collect());
            }
            (s.used_projects().into_iter().cloned().collect::<Vec<_>>(), sessions, loaded(s, &key))
        };
        let (projects, sessions, loaded) = crate::servers::runtime::with_server(s, &key, read).unwrap_or_default();
        servers.push(ServerView { label: crate::servers::registry::label(&e.conn), health: crate::servers::health::get(&key), key, conn: e.conn, projects, sessions, loaded });
    }
    let (stored, version) = STORE.with_borrow(|st| (st.selection.clone(), st.version));
    let env = machine::Env {
        hydrated: !s.live() || crate::plugin_host::server_sources::pending_sources().is_empty(),
        stored,
        servers: servers
            .iter()
            .map(|v| machine::ServerSnapshot {
                key: v.key.clone(),
                failed: v.failed(),
                connected: v.loaded && !v.failed(),
                projects_ready: v.loaded,
                used_projects: v.projects.iter().map(|p| machine::Project { id: p.id.clone(), worktree: p.directory.clone() }).collect(),
            })
            .collect(),
    };
    s.home_state.servers = servers;
    s.home_state.seen = version;
    let fx = step(s, machine::Event::EnvSynced(env));
    run(s, fx, &mut Tx::default());
    added(s);
}

/// The server's first reads landed (or there's no server to read: a fixture).
fn loaded(s: &App, key: &ServerKey) -> bool {
    !s.live() || matches!(s.launch.phase(key.as_str()), Some(crate::launch::Phase::Connected))
}

fn run(s: &mut App, fx: Effects<machine::Effect>, tx: &mut Tx) {
    for effect in fx {
        match effect {
            machine::Effect::SelectionPersist(sel) => {
                write(|st| st.selection = sel);
                s.home_state.seen = STORE.with_borrow(|st| st.version);
            }
            machine::Effect::ProjectAdd { attempt, server, directories } => {
                let Some(first) = directories.first().cloned() else { continue };
                s.home_state.adding = Adding::Waiting { attempt, server: server.clone(), directory: first };
                crate::servers::runtime::with_server(s, &server, |s| {
                    for dir in &directories {
                        s.open_project_dir(dir, false, tx);
                    }
                });
            }
            machine::Effect::WorktreesList { attempt, server, project_id } => {
                crate::servers::runtime::with_server(s, &server, |s| s.list_worktrees(&project_id));
                let fx = machine::step(&mut s.home_state.machine, machine::Event::WorktreesListed { attempt });
                run(s, fx, tx);
            }
            machine::Effect::WorktreesRefresh { .. } => {}
        }
    }
}

/// A project add Home waits for landed: its project is selected.
fn added(s: &mut App) {
    let Adding::Waiting { attempt, server, directory } = s.home_state.adding.clone() else { return };
    let dir = machine::comparable_path(&directory);
    let Some(project) = s.home_state.servers.iter().find(|v| v.key == server).and_then(|v| v.projects.iter().find(|p| machine::comparable_path(&p.directory) == dir)) else { return };
    let project = machine::Project { id: project.id.clone(), worktree: project.directory.clone() };
    s.home_state.adding = Adding::Idle;
    let fx = step(s, machine::Event::ProjectAdded { attempt, project: Some(project) });
    run(s, fx, &mut Tx::default());
}

/// Each poll: the servers' latest state reaches the selection, and while Home shows, its list's
/// phase follows the focused server. True when what Home shows changed.
/// `fresh`: the app's state may have moved since the last poll, so Home's servers are read again.
pub fn poll(s: &mut App, fresh: bool, tx: &mut Tx) -> bool {
    let landed = landing(s, tx);
    let before = (selection(s), s.home_state.servers.iter().map(|v| (v.key.clone(), v.health.clone(), v.projects.len(), v.loaded)).collect::<Vec<_>>(), list_view(s));
    let closed = super::remote_picker::poll(s, tx);
    if fresh || closed {
        sync(s);
    }
    let shown = s.route == Route::Home && !s.settings.open;
    let target = shown.then(|| target(s));
    // Every poll comes here: off Home with the load at rest, and with no timer due, the home
    // state isn't written.
    if target.is_some() || !matches!(s.home_state.load, home_load::State::Idle { .. }) {
        let fx = home_load::step(&mut s.home_state.load, home_load::Event::ContextChanged(target));
        run_load(s, fx, tx);
    }
    let now = *s.clock;
    let any_due = s.home_state.timers.iter().any(|t| matches!(t, Armed::At { due, .. } if *due <= now));
    let due: Vec<home_load::Event> = if !any_due { Vec::new() } else { s
        .home_state
        .timers
        .iter_mut()
        .filter_map(|t| match t {
            Armed::At { due, event } if *due <= now => {
                let event = event.clone();
                *t = Armed::Idle;
                Some(event)
            }
            _ => None,
        })
        .collect() };
    for event in due {
        let fx = home_load::step(&mut s.home_state.load, event);
        run_load(s, fx, tx);
    }
    for t in &s.home_state.timers {
        if let Armed::At { due, .. } = t {
            tx.wake_at(*due);
        }
    }
    let after = (selection(s), s.home_state.servers.iter().map(|v| (v.key.clone(), v.health.clone(), v.projects.len(), v.loaded)).collect::<Vec<_>>(), list_view(s));
    landed || closed || before != after
}

/// `home/projects/landing.tsx` restores once per visit. A slow or failed session open
/// must not enqueue the same navigation on every poll.
#[derive(Default, PartialEq)]
enum Landing { #[default] Away, Visited }

enum Visit { Left, Arrived(Option<String>) }

fn landing_step(state: &mut Landing, event: Visit) -> Option<String> {
    match event {
        Visit::Left => *state = Landing::Away,
        Visit::Arrived(tab) if *state == Landing::Away => { *state = Landing::Visited; return tab; }
        Visit::Arrived(_) => {}
    }
    None
}

fn landing(s: &mut App, tx: &mut Tx) -> bool {
    let project_nav = s.project_navigation();
    if project_nav && !s.settings.open && let Some(tab) = s.current_tab().and_then(|uid| s.tab(&uid)) {
        let directory = s.session(&tab.id).and_then(|ses| s.project_for_session(ses)).map_or(&tab.directory, |p| &p.directory);
        let target = (tab.server.clone(), directory.clone());
        if s.home_state.last_project.as_ref() != Some(&target) { s.home_state.last_project = Some(target); }
    }
    let visit = if project_nav && s.route == Route::Home && !s.settings.open {
        if s.home_state.landing == Landing::Visited { return false; }
        Visit::Arrived(s.restore_project_tab().map(crate::tabs::Tab::uid))
    } else {
        if s.home_state.landing == Landing::Away { return false; }
        Visit::Left
    };
    let Some(uid) = landing_step(&mut s.home_state.landing, visit) else { return false };
    s.select_tab(&uid, tx);
    true
}

/// The focused server as the list sees it (`HomeSessionsTarget`).
fn target(s: &App) -> Target {
    let key = server(s);
    let view = s.home_state.servers.iter().find(|v| v.key == key);
    let health = view.and_then(|v| v.health.clone());
    let offline = s.live() && crate::launch::offline(s);
    let cached = view.is_some_and(|v| v.loaded) && !offline;
    let connection = match health {
        _ if offline => Connection::Unavailable(Unavailable::Offline),
        Some(Health::Probe(ProbeResult::Unauthorized) | Health::Auth) => Connection::Unavailable(Unavailable::Auth),
        Some(Health::Incompatible) => Connection::Unavailable(Unavailable::Incompatible),
        Some(Health::Probe(ProbeResult::Unreachable)) if !cached => Connection::Unavailable(Unavailable::Offline),
        _ if cached => Connection::Connected,
        _ => Connection::Connecting,
    };
    Target { server: key, connection, cached, stale: false }
}

fn run_load(s: &mut App, fx: Effects<home_load::Effect>, tx: &mut Tx) {
    for effect in fx {
        match effect {
            // The window's own launch reads the list (`servers::runtime`): once it's there the
            // fetch has its answer.
            home_load::Effect::FetchStart { attempt, .. } => {
                let fx = home_load::step(&mut s.home_state.load, home_load::Event::FetchSucceeded { attempt, reduced_motion: *s.reduced_motion });
                run_load(s, fx, tx);
            }
            home_load::Effect::FetchCancel(_) => {}
            home_load::Effect::ProbeServers => {
                crate::servers::health::retry(s, tx);
                if crate::launch::offline(s) {
                    crate::launch::retry(s, tx);
                }
            }
            home_load::Effect::TimerStart { timer, delay_ms, event } => {
                let due = *s.clock + delay_ms as f64 / 1000.0;
                s.home_state.timers[slot(timer)] = Armed::At { due, event };
                tx.wake_at(due);
            }
            home_load::Effect::TimerStop(timer) => s.home_state.timers[slot(timer)] = Armed::Idle,
        }
    }
}

fn slot(timer: Timer) -> usize {
    match timer {
        Timer::Placeholder => 0,
        Timer::Deadline => 1,
        Timer::Reveal => 2,
    }
}

/// What the session list shows (`selectHomeSessionsListView`).
pub fn list_view(s: &App) -> home_load::ListView {
    s.home_state.load.list_view(!s.sessions.is_empty())
}

/// The list's Retry: probe the servers, and read the list again.
pub fn retry(s: &mut App, tx: &mut Tx) {
    let fx = home_load::step(&mut s.home_state.load, home_load::Event::Retry);
    run_load(s, fx, tx);
    tx.instant();
}
