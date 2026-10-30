//! OpenCode Desktop in Rune: OpenCode's home and session pages in Rune, live against the local OpenCode server or
//! from a synthetic fixture. Live, it sends prompts, answers permission requests and questions,
//! stops runs and starts sessions (`--read-only` turns all of that off).

/// Counts each thread's allocations for the frame diagnostics and the benchmarks.
#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

mod add_menu;
mod agent_rows;
mod agent_rows_view;
mod agent_start;
mod agent_start_stories;
mod agent_stories;
mod arrivals;
mod attachments;
mod background;
mod background_stories;
mod browser;
mod capsule_stories;
pub mod code_view;
mod compaction;
mod compaction_stories;
mod diagnostics;
mod dictation;
#[cfg(test)]
mod dictation_tests;
mod disclosure;
mod disk;
mod dropzone;
mod events;
mod find;
mod find_stories;
#[cfg(test)]
mod find_tests;
mod fixture_ids;
mod fold;
mod fold_stories;
#[cfg(test)]
mod fold_tests;
mod forms;
mod frosted_stories;
mod history;
mod history_story;
#[cfg(test)]
mod history_tests;
mod home_search;
#[cfg(test)]
mod home_search_tests;
mod icons;
mod jump;
mod keys;
mod knobs;
mod launch;
mod mention_stories;
mod mentions;
#[cfg(test)]
mod motion_gate;
mod motion_log;
mod notifications;
mod odometer;
mod odometer_stories;
mod open_story;
#[cfg(test)]
mod open_story_tests;
mod palette;
pub mod panels;
#[cfg(test)]
mod perf;
mod picker_stories;
mod pickers;
mod project_stories;
mod project_switcher;
mod projects;
mod prompt_history;
mod prompt_history_story;
mod question;
mod question_row;
mod question_stories;
mod queue;
mod quote;
mod quote_cards;
mod quote_stories;
mod restore;
mod review;
#[cfg(test)]
mod revert;
mod scenes;
mod selection_stories;
mod send;
mod served;
mod session_menu;
mod session_menu_stories;
#[cfg(test)]
mod session_menu_tests;
mod session_menu_view;
mod session_open;
mod session_pages;
mod settings;
mod shortcuts;
mod side_stories;
mod slash;
mod stories;
mod subagents;
mod switcher;
mod tab_actions;
mod tabs;
mod terminal;
mod tool_group_stories;
mod turns;
mod view;
mod working;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender};

use rune::{AppSpec, Key, KeyEvent, TextEdit, Tx, Waker};
use serde_json::Value;

use opencode_kit::client::{DEFAULT_SERVER, Incoming, Prompt, Request, Server, Write};
use opencode_kit::model::{Delivery, Inbox, Message, Pending, Project, SessionInfo, Transcript};
use keys::on_key;
pub(crate) use scenes::open_scene;
use tabs::Tab;

const FIXTURE: &str = include_str!("../assets/fixture.json");

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Route {
    #[default]
    Home,
    Session(String),
    /// "New session": an empty page with the composer.
    Draft,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabStatus {
    Busy,
    Attention,
    Unread,
}

#[derive(Default)]
pub enum Source {
    Live { server: Server, requests: Option<Sender<Request>>, writes: Option<Sender<Write>>, allow_send: bool },
    #[default]
    Fixture,
}

#[derive(Default)]
pub struct App {
    pub source: Source,
    inbox: Option<Receiver<Incoming>>,
    /// The live server events taken, for the motion log (`motion_log.rs`).
    motion_events: motion_log::Events,
    pages: session_pages::State,
    opening: Option<session_pages::Opening>,
    open_demo: Option<open_story::Demo>,
    pub projects: Vec<Project>,
    pub sessions: Vec<SessionInfo>,
    pub route: Route,
    /// Where ⌘[ and ⌘] go (`history.rs`).
    pub nav: history::History,
    pub transcripts: HashMap<String, Transcript>,
    pub expanded: HashSet<String>,
    /// Tool groups springing shut, by key: the poll clock when they shut (`disclosure.rs`).
    pub shutting: HashMap<String, f64>,
    /// Pinned session tabs, in sidebar order (see `tabs.rs` for where they come from).
    pub tabs: Vec<Tab>,
    /// A tab being dragged to a new place in the sidebar.
    pub tab_drag: tabs::Reorder,
    /// Tabs closed this run, newest last, with the index they had (⌘⇧T reopens).
    closed: Vec<(usize, Tab)>,
    /// UI zoom, 1.0 = 100% (see `view::app`).
    pub zoom: f32,
    /// Sessions running right now (any depth; a tab is busy when anything in its family runs).
    pub running: HashSet<String>,
    /// Pending permission and form requests: (session, request id).
    pub requests: HashSet<(String, String)>,
    /// The open permission requests and forms we know the details of.
    pub pending: Vec<Pending>,
    /// Prompts sent while a session was busy that the server hasn't delivered yet, by session.
    pub inboxes: HashMap<String, Inbox>,
    /// The question dock's place and in-progress answers for the forms it shows.
    pub question: question::Dock,
    /// Where "New session" creates its session.
    pub draft_dir: String,
    pub sidebar_open: bool,
    pub sidebar_w: f32,
    /// The width when a resize drag started.
    pub sidebar_drag: Option<f32>,
    pub composer: TextEdit,
    /// Sent prompts, for ↑ and ↓ in the composer (`prompt_history.rs`).
    pub prompts: prompt_history::PromptHistory,
    /// The composer's images, and sent prompts waiting on the server's copy.
    pub attach: attachments::Attach,
    /// The Quote button over a timeline selection, and the quotes waiting in the composer.
    pub quote: quote::Quotes,
    pub find: find::Find,
    pub search: TextEdit,
    pub home_search: home_search::HomeSearch,
    /// Session menus, the title being renamed and Delete's confirmation.
    pub menus: session_menu::Menus,
    pub pickers: pickers::Pickers,
    /// The composer's `@` popover and what it lists.
    pub mentions: mentions::Mentions,
    /// Live look-and-feel sliders (⌘⇧K).
    pub knobs: rune_knobs::Knobs,
    /// The + menu, the `/` command popover and shell mode.
    pub add: add_menu::AddMenu,
    /// Hex dictation into the composer (⇧⌘M or the mic).
    pub dictation: dictation::Dictation,
    pub switcher: switcher::Switcher,
    pub project_switcher: project_switcher::ProjectSwitcher,
    pub project_state: projects::ProjectsState,
    pub settings: settings::Settings,
    /// System notifications for finished runs, permission requests and errors.
    pub notify: notifications::Notifier,
    /// The open session and composer drafts, saved for the next launch.
    pub restore: restore::Restore,
    /// The running-subagents list above the composer is open.
    pub activity_open: bool,
    /// Subagents' latest steps and how their runs ended, for their timeline rows.
    pub agent_rows: agent_rows::Live,
    /// Whether AppKit is saving the window frame yet (`rune_mac::remember_frame`).
    framed: bool,
    pub reduced_motion: bool,
    /// User messages that arrived from another client, by id: the poll clock when they did.
    pub glows: HashMap<String, f64>,
    /// Where each session's timeline stands against its end (`jump.rs`).
    pub ends: jump::Ends,
    /// When each run started, for the Working label's rhythm (`working.rs`).
    pub working: working::Working,
    /// Background compaction by directory and how each compaction enters (`compaction.rs`).
    pub compactions: compaction::Compactions,
    /// `--debug`: the status bar with the edge, the source and the last status.
    pub debug: bool,
    pub status: String,
    /// Wall-clock milliseconds for relative times (fixture: its own fixed "now").
    pub now_ms: i64,
    base_ms: i64,
    clock: f64,
    fixture: Value,
    playback: VecDeque<(f64, String, Value)>,
    /// What the fixture's server stored from the replies it streamed (`served.rs`).
    served: served::Served,
    background: background::Background,
    sends: u32,
    waker: Option<Waker>,
    launch: launch::Launch,
    pub icons: project_icon::Icons,
    pub side: panels::SideState,
    pub browser: browser::BrowserPane,
    pub review: review::ReviewPane,
    pub terminal: terminal::TerminalPane,
    /// A storybook (⌘⇧D) story's stage, in the world the playground made for it.
    pub stage: Option<stories::Stage>,
    /// The story's variant (1 to 9 in the playground pick it).
    pub variant: usize,
}

fn arg(name: &str) -> Option<String> {
    let a = rune::extra_args();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

fn flag(name: &str) -> bool {
    rune::extra_args().iter().any(|x| x == name)
}

/// A run that renders without a person at it (snapshots, trees, scripts, videos). It can read a
/// live server but never sends to it or writes the saved tabs, zoom, session and drafts, so a
/// test can't post or type as Kit.
pub(crate) use rune::is_headless as headless;

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl App {
    fn empty() -> App {
        App {
            draft_dir: arg("--directory").unwrap_or_else(|| std::env::var("HOME").unwrap_or_default()),
            sidebar_open: !flag("--sidebar-closed"),
            sidebar_w: view::SIDEBAR_W,
            zoom: arg("--zoom").and_then(|z| z.parse::<f32>().ok()).map_or(1.0, |z| z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX)),
            composer: TextEdit::new(""),
            search: TextEdit::new(""),
            reduced_motion: flag("--reduced-motion"),
            debug: flag("--debug"),
            ..Default::default()
        }
    }

    /// Back to Home, dropping a session that was still opening.
    pub(crate) fn go_home(&mut self, tx: &mut Tx) {
        self.opening = None;
        self.route = Route::Home;
        tx.settle();
    }

    fn load() -> App {
        let mut app = Self::empty();
        let fixture_path = arg("--fixture");
        let server = Server::new(&arg("--server").unwrap_or_else(|| DEFAULT_SERVER.into())).with_media_refs();
        let deferred = fixture_path.is_none() && launch::deferred();
        let want_live = fixture_path.is_none() && (deferred || flag("--live") || (!headless() && !cfg!(test) && server.reachable()));
        if want_live && app.load_live(server, deferred && !flag("--live")) { return app; }
        let src = match &fixture_path {
            Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}")),
            None => FIXTURE.to_owned(),
        };
        app.load_fixture(&src);
        app
    }

    fn new() -> App {
        let mut app = Self::load();
        if let Some(name) = arg("--preset") {
            knobs::apply_preset(&mut app, &name);
        }
        app
    }

    fn load_fixture(&mut self, src: &str) {
        let v: Value = serde_json::from_str(src).expect("fixture JSON");
        self.pickers.load(&v);
        self.mentions.load(&v);
        self.add.slash.commands = slash::commands(&v["commands"]);
        self.base_ms = v["now"].as_i64().unwrap_or(0);
        self.now_ms = self.base_ms;
        self.projects = opencode_kit::model::parse_projects(&v["projects"]);
        self.sessions = opencode_kit::model::parse_sessions(&v["sessions"]);
        if let Some(map) = v["messages"].as_object() {
            for (id, list) in map {
                let msgs = opencode_kit::model::parse_messages(&serde_json::json!({ "data": list }));
                self.transcripts.insert(id.clone(), Transcript { session: id.clone(), messages: msgs });
            }
        }
        let ids: Vec<String> = v["tabs"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        self.tabs = ids.iter().map(|id| Tab { id: id.clone(), title: String::new(), directory: String::new() }).collect();
        self.running = v["running"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        for r in v["requests"].as_array().into_iter().flatten() {
            if let (Some(s), Some(id)) = (r["sessionID"].as_str(), r["id"].as_str()) {
                self.requests.insert((s.to_owned(), id.to_owned()));
            }
        }
        self.status = "Fixture".into();
        self.fixture = v;
        self.source = Source::Fixture;
        self.init_used_projects(None);
        self.sort_projects();
        if arg("--directory").is_none()
            && let Some(p) = self.projects.first()
        {
            self.draft_dir = p.directory.clone();
        }
    }

    fn load_live(&mut self, server: Server, fallback_fixture: bool) -> bool {
        // A headless snapshot reads the server now; a window reads it and probes after its first frame.
        let first = if launch::deferred() {
            self.launch.waiting = true;
            self.launch.fallback_fixture = fallback_fixture;
            None
        } else {
            let Ok(first) = launch::First::fetch(&server) else { return false };
            Some(first)
        };
        match tabs::load(&server.base).or_else(|| {
            (!self.launch.waiting && server.base == DEFAULT_SERVER).then(tabs::from_beta).flatten()
        }) {
            Some(tabs) => self.tabs = tabs,
            None => {
                self.launch.beta_tabs = self.launch.waiting && server.base == DEFAULT_SERVER;
                self.launch.recent_tabs = true;
            }
        }
        if let Some(first) = first {
            self.apply_first(first);
        }
        if let Some(z) = tabs::load_zoom() {
            self.zoom = z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX);
        }
        self.settings = settings::load();
        self.prompts = prompt_history::PromptHistory::load();
        self.restore_ui(&server.base);
        self.base_ms = wall_ms();
        self.now_ms = self.base_ms;
        let allow_send = !flag("--read-only") && !headless();
        self.source = Source::Live { server, requests: None, writes: None, allow_send };
        self.status = self.live_status();
        true
    }

    fn sort_projects(&mut self) {
        // Projects by their latest session, then by activity.
        let latest: HashMap<&str, i64> = self.sessions.iter().fold(HashMap::new(), |mut m, s| {
            let e = m.entry(s.project.as_str()).or_insert(0);
            *e = (*e).max(s.updated);
            m
        });
        let key = |p: &Project| latest.get(p.id.as_str()).copied().unwrap_or(0).max(p.active);
        let mut ps = std::mem::take(&mut self.projects);
        ps.sort_by_key(|p| std::cmp::Reverse(key(p)));
        self.projects = ps;
    }

    pub fn set_zoom(&mut self, dir: i32) {
        self.zoom = shortcuts::step_zoom(self.zoom, dir);
        if self.persists() {
            tabs::save_zoom(self.zoom);
        }
    }

    /// OpenCode Desktop's key defaults; true when `ev` was one.
    pub fn shortcut(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        use shortcuts::Action;
        if dictation::key(self, ev, tx) || background::key(self, ev, tx) || find::shortcut(self, ev, tx) {
            return true;
        }
        if let Some(forward) = history::bracket(ev, tx.physical_key()) {
            self.navigate(forward, tx);
            return true;
        }
        let Some(action) = shortcuts::action(ev) else { return false };
        match action {
            // Zoom lands at once; the scale and every re-laid-out row would otherwise spring.
            Action::ZoomIn => self.zoom_by(1, tx),
            Action::ZoomOut => self.zoom_by(-1, tx),
            Action::ZoomReset => self.zoom_by(0, tx),
            Action::Tab(offset) => self.select_adjacent(offset, false, tx),
            Action::UnreadTab(offset) => self.select_adjacent(offset, true, tx),
            Action::TabAt(i) => {
                let id = if i == 8 { self.tabs.last() } else { self.tabs.get(i) }.map(|t| t.id.clone());
                if let Some(id) = id {
                    self.open(&id, tx);
                }
            }
            Action::NewSession => self.new_session(tx),
            Action::OpenProject => self.open_project_dialog(tx),
            Action::SelectProject => self.toggle_project_switcher(tx),
            Action::Settings => self.toggle_settings(tx),
            Action::CloseTab => {
                if let Some(id) = self.current_tab() {
                    self.close_tab(&id, tx);
                }
            }
            Action::ReopenTab => self.reopen_tab(tx),
        }
        true
    }

    pub fn live(&self) -> bool {
        matches!(self.source, Source::Live { .. })
    }

    /// Saves tabs, zoom, the open session and drafts: live, with a person at it.
    pub fn persists(&self) -> bool {
        self.live() && !headless() && !cfg!(test)
    }

    /// Asks the server for one session's info (live only).
    pub fn fetch_info(&self, id: &str) {
        if let Source::Live { requests: Some(r), .. } = &self.source {
            let _ = r.send(Request::Info(id.to_owned()));
        }
    }

    /// `id` and its loaded parents, up to 32 steps.
    pub(crate) fn ancestors<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a str> {
        std::iter::successors(Some(id), |cur| self.session(cur)?.parent.as_deref()).take(33)
    }

    /// The first session in `id`'s parent chain (itself included) that isn't loaded, so a
    /// running subagent can be traced to its tab.
    fn missing_ancestor(&self, id: &str) -> Option<String> {
        self.ancestors(id).take(32).last().filter(|cur| self.session(cur).is_none()).map(str::to_owned)
    }

    pub fn session(&self, id: &str) -> Option<&SessionInfo> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// Whether anything in the open session's family is running.
    pub fn busy(&self, id: &str) -> bool {
        let root = self.root_of(id);
        self.running.iter().any(|s| self.root_of(s) == root)
    }

    /// Stops every running session in the open session's family.
    pub fn stop(&mut self) {
        let Route::Session(id) = self.route.clone() else { return };
        let root = self.root_of(&id).to_owned();
        let running: Vec<String> = self.running.iter().filter(|s| self.root_of(s) == root).cloned().collect();
        for s in running {
            self.write(Write::Interrupt(s));
        }
    }

    pub fn new_session(&mut self, tx: &mut Tx) {
        self.close_picker(false);
        self.close_project_switcher(false);
        self.project_state.home_menu_open = false;
        match &self.route {
            Route::Session(id) => {
                if let Some(dir) = self.session(id).map(|s| s.directory.clone()).filter(|d| !d.is_empty()) {
                    self.draft_dir = dir;
                }
            }
            Route::Home => {
                let dir = self
                    .project_state
                    .home_selected
                    .as_deref()
                    .and_then(|id| self.project(id))
                    .or_else(|| self.used_projects().into_iter().next())
                    .map(|p| p.directory.clone());
                if let Some(dir) = dir.filter(|d| !d.is_empty()) {
                    self.draft_dir = dir;
                }
            }
            Route::Draft => {}
        }
        self.opening = None;
        self.route = Route::Draft;
        tx.settle();
    }

}

fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    s.clock = now;
    s.sync_history();
    s.now_ms = if s.live() { wall_ms() } else { s.base_ms + (now * 1000.0) as i64 };
    // The morph loop is for watching: the composer it lands on doesn't take the keys.
    if s.stage.is_some_and(|st| st.watch()) {
        s.composer.focused = false;
    }
    let mut changed = dictation::poll(s, now, tx);
    changed |= launch::poll(s, tx);
    changed |= notifications::poll(s, tx);
    changed |= quote::poll(s, now, tx);
    changed |= dropzone::poll(s, tx);
    changed |= s.poll_pages(tx);
    let mut arrivals = None;
    let asked = s.question_shown();
    if let Some(inbox) = s.inbox.take() {
        if let Ok(first_in) = inbox.try_recv() {
            arrivals = Some(arrivals::Arrivals::before(s));
            for m in std::iter::once(first_in).chain(inbox.try_iter()) {
                match m {
                    Incoming::Sessions(mut list) => {
                        for n in &mut list {
                            if let Some(old) = s.session(&n.id) {
                                n.viewed = n.viewed.max(old.viewed);
                            }
                        }
                        // Sessions past the page (a tab's, a subagent's parent) stay known.
                        let rest: Vec<SessionInfo> = s.sessions.drain(..).filter(|o| !list.iter().any(|n| n.id == o.id)).collect();
                        list.extend(rest);
                        s.sessions = list;
                        s.sort_projects();
                    }
                    Incoming::Info(mut info) => {
                        let parent = info.parent.clone();
                        match s.sessions.iter_mut().find(|x| x.id == info.id) {
                            Some(old) => {
                                info.viewed = info.viewed.max(old.viewed);
                                *old = info;
                            }
                            None => s.sessions.push(info),
                        }
                        if let Some(missing) = parent.and_then(|p| s.missing_ancestor(&p)) {
                            s.fetch_info(&missing);
                        }
                    }
                    Incoming::Messages { session, messages } => {
                        let cold = s.launch.cold_transcript.as_deref() == Some(session.as_str());
                        if cold {
                            s.launch.cold_transcript = None;
                        }
                        let first = !s.transcripts.contains_key(&session);
                        let t = s.transcripts.entry(session.clone()).or_insert_with(|| Transcript { session: session.clone(), messages: vec![] });
                        attachments::reconcile(&mut s.attach.echoes, t, &messages);
                        let delivered: Vec<String> = messages.iter().map(|m| m.id().to_owned()).collect();
                        t.merge(messages);
                        s.settle_inbox(&session, &delivered);
                        if (first || cold) && s.route == Route::Session(session) {
                            // Guarantee 8: the first load of the open session lands, nothing grows in.
                            tx.settle();
                        }
                        if cold {
                            arrivals = None;
                        }
                    }
                    // Most events are other sessions' streams: they redraw only what they change.
                    Incoming::Event { ty, data } => {
                        s.motion_events.note(&ty);
                        changed |= s.apply_event(&ty, &data);
                        continue;
                    }
                    Incoming::Status(e) => s.status = background::status(e),
                    Incoming::Pending { session, items } => {
                        s.pending.retain(|p| p.session() != session || items.iter().any(|i| i.id() == p.id()));
                        for p in items {
                            s.add_pending(p);
                        }
                    }
                    Incoming::Reconnected => {
                        tx.instant();
                        s.resync();
                    }
                    Incoming::Active(ids) => {
                        tx.instant();
                        s.running = ids.into_iter().collect();
                    }
                    Incoming::Projects(projects) => {
                        tx.instant();
                        s.projects = projects;
                        s.sort_projects();
                    }
                    Incoming::ProjectOpened { location, projects, session } => {
                        s.apply_project_opened(location, projects, session, tx);
                    }
                    Incoming::Created(info) => {
                        let id = info.id.clone();
                        if s.session(&id).is_none() {
                            s.sessions.insert(0, info);
                        }
                        s.status = s.live_status();
                        s.open(&id, tx);
                    }
                }
                changed = true;
            }
        }
        s.inbox = Some(inbox);
    }
    if s.playback.front().is_some_and(|e| e.0 <= now + 1e-9) {
        arrivals.get_or_insert_with(|| arrivals::Arrivals::before(s));
        while s.playback.front().is_some_and(|e| e.0 <= now + 1e-9) {
            let (_, ty, mut data) = s.playback.pop_front().unwrap();
            // A scripted delivery names the oldest prompt of its kind, which has no id until sent.
            if let Some(kind) = Delivery::parse(&data["next"])
                && let Some(item) = s.inboxes.get(data["sessionID"].as_str().unwrap_or("")).and_then(|i| i.items.iter().find(|i| i.delivery == kind))
            {
                data["inboxID"] = item.id.clone().into();
            }
            if ty == "fixture.page" {
                s.show_fixture_page(data["sessionID"].as_str().unwrap_or(""), tx);
                changed = true;
                continue;
            }
            s.apply_event(&ty, &data);
            if let Some(sid) = data["sessionID"].as_str().filter(|sid| ty == "session.step.started" && s.route == Route::Session(sid.to_string())) {
                s.refresh_fixture_page(sid);
            }
            changed = true;
        }
    }
    if let Some(arrivals) = arrivals {
        arrivals.land(s, now, tx);
    }
    changed |= working::sync(s);
    changed |= compaction::sync(s);
    if s.question_shown() != asked {
        s.morph(tx);
    }
    let glow = knobs::glow_secs(&s.knobs);
    s.glows.retain(|_, at| now - *at < glow);
    if !s.glows.is_empty() {
        tx.wake_after(std::time::Duration::ZERO);
        changed = true;
    }
    changed |= disclosure::poll(s, now, tx);
    changed |= agent_rows::poll(s, tx);
    changed |= find::poll(s, tx);
    changed |= s.poll_home_search(tx);
    changed |= s.poll_switcher(tx);
    changed |= s.poll_mentions(tx);
    changed |= s.poll_commands();
    changed |= s.poll_rename();
    changed |= s.poll_uploads();
    changed |= s.poll_panels(tx);
    changed |= s.poll_terminal(tx);
    if s.persists() {
        s.poll_restore();
        if !s.framed {
            s.framed = rune_mac::remember_frame("OpenCodeRuneMain");
        }
    }
    changed
}

fn setup(s: &mut App, waker: Waker) {
    s.waker = Some(waker.clone());
    s.attach.set_waker(waker.clone());
    s.icons.set_waker(waker.clone());
    s.mentions.set_waker(waker.clone());
    s.switcher.set_waker(waker.clone());
    s.home_search.set_waker(waker.clone());
    s.add.slash.set_waker(waker.clone());
    s.review.set_waker(waker.clone());
    s.icons.set_sync(headless());
    s.setup_browser(waker);
    if !s.playback.is_empty() {
        s.wake_at_playback();
    }
    launch::setup(s);
}

/// The app's spec: `init` makes its state (`App::new`, or `App::empty` in tests).
pub(crate) fn spec(init: fn() -> App) -> AppSpec<App> {
    AppSpec {
        scenes: stories::scenes(),
        fonts: &[
            include_bytes!("../assets/fonts/InterVariable.ttf"),
            include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
            include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
            include_bytes!("../assets/fonts/CommitMono-400-Regular.otf"),
            include_bytes!("../assets/fonts/CommitMono-700-Regular.otf"),
            include_bytes!("../assets/fonts/CommitMono-400-Italic.otf"),
            include_bytes!("../assets/fonts/CommitMono-700-Italic.otf"),
        ],
        families: ("Inter", "IBM Plex Mono"),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("OpenCode (Rune)", (1280.0, 820.0), init, view::app)
    }
}

/// The app as it runs: its history, its storybook (⌘⇧D) and its window chrome.
pub(crate) fn rune_app(spec: AppSpec<App>) -> rune::App<App> {
    find::install(history::install(
        rune::App::new(spec)
            .playground(stories::playground())
            .on_ime(|s, ime, _| s.panels_ime(ime))
            .chrome(rune::Chrome {
                full_size_content: true,
                traffic_lights: Some((25.0, 25.0)),
                drag_regions: true,
            }),
    ))
}

fn main() {
    rune_mac::launch();
    rune::prewarm_gpu();
    let _ = rune::set_app_icon(include_bytes!("../assets/icon.icns").as_slice());
    #[cfg(feature = "hot")]
    rune::run(spec(App::new));
    #[cfg(not(feature = "hot"))]
    {
        if !headless() {
            launch::defer();
        }
        let app = rune_app(spec(App::new));
        let handle = app.handle();
        let mut motion_log = motion_log::Log::default();
        let diagnostics = diagnostics::Diagnostics::default();
        let (encoded, drawn) = (diagnostics.clone(), diagnostics.clone());
        app.on_ready(move |_| handle.update_if(|s, tx| { launch::ready(s, tx); false }))
            .on_frame_encoded(move |f| encoded.encoded(f))
            .on_frame_drawn(move |rt| {
                drawn.drawn(rt);
                motion_log.frame(rt);
            })
            .on_close(|s, _| s.save_on_close())
            .run();
        disk::flush();
        diagnostics.shutdown();
    }
}

#[cfg(test)]
impl App {
    /// The fixture's open session at the end of its stream, without probing a server.
    pub(crate) fn fixture_session() -> Self {
        let mut app = Self::empty();
        app.load_fixture(FIXTURE);
        open_scene(&mut app);
        app.finish_stream();
        app
    }

    /// Switches `source` to a live write channel so tests can read what would be sent.
    pub(crate) fn fake_writes(&mut self) -> std::sync::mpsc::Receiver<Write> {
        let (w, writes) = std::sync::mpsc::channel();
        self.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
        writes
    }

    pub(crate) fn into_runtime(self) -> rune::Runtime<Self> {
        let mut rt = rune::Runtime::new(self, view::app, (1280.0, 820.0), rune::Env::new(true, 1.0));
        rt.set_srgb_blend(true);
        rt.poll = Some(poll);
        rt.on_key = Some(on_key);
        rt
    }
}

#[cfg(test)]
pub(crate) fn fixture_runtime() -> rune::Runtime<App> {
    let mut rt = App::fixture_session().into_runtime();
    rt.frame(0.0);
    rt
}
