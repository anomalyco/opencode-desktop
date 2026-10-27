//! OpenCode Desktop in Rune: OpenCode's home and session pages in Rune, live against the local OpenCode server or
//! from a synthetic fixture. Live, it sends prompts, answers permission requests and questions,
//! stops runs and starts sessions (`--read-only` turns all of that off).

mod add_menu;
mod arrivals;
mod attachments;
mod icons;
mod jump;
mod knobs;
mod mentions;
mod palette;
#[cfg(test)]
mod perf;
mod pickers;
mod queue;
mod question;
mod restore;
mod revert;
mod session_pages;
mod open_story;
#[cfg(test)]
mod open_story_tests;
#[cfg(test)]
mod motion_gate;
mod shortcuts;
mod stories;
mod subagents;
mod settings;
mod switcher;
mod tabs;
mod turns;
mod view;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender};

use rune::{AppSpec, Key, KeyEvent, TextEdit, Tx, Waker, edge};
use serde_json::Value;

use opencode_kit::client::{DEFAULT_SERVER, Incoming, Prompt, Request, Server, Write};
use opencode_kit::model::{Delivery, FieldKind, Inbox, Message, Pending, Project, SessionInfo, Transcript};
use tabs::Tab;

const FIXTURE: &str = include_str!("../assets/fixture.json");

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
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

pub enum Source {
    Live { server: Server, requests: Option<Sender<Request>>, writes: Option<Sender<Write>>, allow_send: bool },
    Fixture,
}

pub struct App {
    pub source: Source,
    inbox: Option<Receiver<Incoming>>,
    pages: Option<session_pages::Pages>,
    opening: Option<session_pages::Opening>,
    open_demo: Option<open_story::Demo>,
    page_history: Option<(std::time::Instant, session_pages::Key, String)>,
    page_history_loaded: HashSet<String>,
    pub projects: Vec<Project>,
    pub sessions: Vec<SessionInfo>,
    pub route: Route,
    pub transcripts: HashMap<String, Transcript>,
    pub expanded: HashSet<String>,
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
    /// Form answers in progress, by `form/key`: picked option values and typed text.
    pub picks: HashMap<String, Vec<String>>,
    pub typed: HashMap<String, TextEdit>,
    /// The question dock's place in the form it shows.
    pub question: question::Dock,
    /// Where "New session" creates its session.
    pub draft_dir: String,
    pub sidebar_open: bool,
    pub sidebar_w: f32,
    /// The width when a resize drag started.
    pub sidebar_drag: Option<f32>,
    pub composer: TextEdit,
    /// The composer's images, and sent prompts waiting on the server's copy.
    pub attach: attachments::Attach,
    pub search: TextEdit,
    pub pickers: pickers::Pickers,
    /// The composer's `@` popover and what it lists.
    pub mentions: mentions::Mentions,
    /// Live look-and-feel sliders (⌘⇧K).
    pub knobs: rune_knobs::Knobs,
    /// The + menu, the `/` command popover and shell mode.
    pub add: add_menu::AddMenu,
    pub switcher: switcher::Switcher,
    pub settings: settings::Settings,
    /// The open session and composer drafts, saved for the next launch.
    pub restore: restore::Restore,
    /// Links opened from messages, newest last.
    pub opened: Vec<String>,
    /// The running-subagents list above the composer is open.
    pub activity_open: bool,
    /// Whether AppKit is saving the window frame yet (`mac_app::remember_frame`).
    framed: bool,
    pub edge: usize,
    pub reduced_motion: bool,
    /// User messages that arrived from another client, by id: the poll clock when they did.
    pub glows: HashMap<String, f64>,
    /// Where each session's timeline stands against its end (`jump.rs`).
    pub ends: jump::Ends,
    /// `--debug`: the status bar with the edge, the source and the last status.
    pub debug: bool,
    pub status: String,
    /// Wall-clock milliseconds for relative times (fixture: its own fixed "now").
    pub now_ms: i64,
    base_ms: i64,
    clock: f64,
    fixture: Value,
    playback: VecDeque<(f64, String, Value)>,
    sends: u32,
    waker: Option<Waker>,
    pub icons: project_icon::Icons,
    /// The `icon-playground` scenes: every icon style instead of the panel, in these
    /// appearances (dark or not); empty shows the app.
    pub playground: &'static [bool],
    /// The storybook (⌘⇧D) is open, with the app as it was beside it.
    pub story: Option<stories::Storybook>,
    /// The storybook's keys, folds and queued trigger steps, carried from story to story.
    pub play: rune_playground::Play<App>,
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
fn headless() -> bool {
    static H: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *H.get_or_init(|| {
        const FLAGS: &[&str] = &["--snap", "--tree", "--script", "--out", "--video", "--strip", "--frames", "--duration", "--trace", "--list-scenes", "--snap-on-patch"];
        std::env::args().any(|a| FLAGS.contains(&a.as_str()))
    })
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl App {
    fn empty() -> App {
        let edge_name = arg("--edge").unwrap_or_else(|| "feather".into());
        let edge = edge::ALL.iter().position(|e| e.name == edge_name).unwrap_or(0);
        App {
            source: Source::Fixture,
            inbox: None,
            pages: None,
            opening: None,
            open_demo: None,
            page_history: None,
            page_history_loaded: HashSet::new(),
            projects: Vec::new(),
            sessions: Vec::new(),
            route: Route::Home,
            transcripts: HashMap::new(),
            expanded: HashSet::new(),
            tabs: Vec::new(),
            tab_drag: tabs::Reorder::default(),
            closed: Vec::new(),
            zoom: 1.0,
            running: HashSet::new(),
            requests: HashSet::new(),
            pending: Vec::new(),
            inboxes: HashMap::new(),
            picks: HashMap::new(),
            typed: HashMap::new(),
            question: question::Dock::default(),
            draft_dir: arg("--directory").unwrap_or_else(|| std::env::var("HOME").unwrap_or_default()),
            sidebar_open: !flag("--sidebar-closed"),
            sidebar_w: view::SIDEBAR_W,
            sidebar_drag: None,
            composer: TextEdit::new(""),
            attach: Default::default(),
            search: TextEdit::new(""),
            pickers: pickers::Pickers::default(),
            mentions: mentions::Mentions::default(),
            add: add_menu::AddMenu::default(),
            switcher: switcher::Switcher::default(),
            settings: settings::Settings::default(),
            restore: restore::Restore::default(),
            opened: Vec::new(),
            activity_open: false,
            knobs: rune_knobs::Knobs::default(),
            framed: false,
            edge,
            reduced_motion: flag("--reduced-motion"),
            glows: HashMap::new(),
            ends: jump::Ends::default(),
            debug: flag("--debug"),
            status: String::new(),
            now_ms: 0,
            base_ms: 0,
            clock: 0.0,
            fixture: Value::Null,
            playback: VecDeque::new(),
            sends: 0,
            waker: None,
            icons: Default::default(),
            playground: &[],
            story: None,
            play: Default::default(),
        }
    }

    fn load() -> App {
        let mut app = Self::empty();
        let fixture_path = arg("--fixture");
        let server = Server::new(&arg("--server").unwrap_or_else(|| DEFAULT_SERVER.into())).with_media_refs();
        let want_live = fixture_path.is_none() && (flag("--live") || (!headless() && server.reachable()));
        if want_live && app.load_live(server) { return app; }
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
        self.add.commands = add_menu::commands(&v["commands"]);
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
        self.sort_projects();
    }

    fn load_live(&mut self, server: Server) -> bool {
        // The first loads are synchronous so the first frame (and a headless snapshot) has data.
        let Ok(p) = server.get("/api/project") else { return false };
        self.projects = opencode_kit::model::parse_projects(&p);
        if let Ok(s) = server.get("/api/session?limit=120") {
            self.sessions = opencode_kit::model::parse_sessions(&s);
        }
        if let Ok(m) = server.get("/api/model") {
            let agents = server.get("/api/agent").unwrap_or_default();
            self.pickers.load_live(m.get("data").unwrap_or(&m), agents.get("data").unwrap_or(&agents));
        }
        if let Ok(a) = server.get("/api/session/active") {
            let map = a.get("data").unwrap_or(&a);
            self.running = map.as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect();
        }
        self.tabs = tabs::load(&server.base)
            .or_else(|| if server.base == DEFAULT_SERVER { tabs::from_beta() } else { None })
            .unwrap_or_else(|| self.recent_tabs());
        if let Some(z) = tabs::load_zoom() {
            self.zoom = z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX);
        }
        self.settings = settings::load();
        self.restore_ui(&server.base);
        self.base_ms = wall_ms();
        self.now_ms = self.base_ms;
        self.status = format!("Live · {}", server.base.trim_start_matches("http://"));
        let allow_send = !flag("--read-only") && !headless();
        self.source = Source::Live { server, requests: None, writes: None, allow_send };
        self.sort_projects();
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

    /// A fresh profile's tabs: the most recent root sessions.
    fn recent_tabs(&self) -> Vec<Tab> {
        let mut roots: Vec<&SessionInfo> = self.sessions.iter().filter(|s| s.parent.is_none()).collect();
        roots.sort_by_key(|s| std::cmp::Reverse(s.updated));
        roots.iter().take(8).map(|s| Tab { id: s.id.clone(), title: s.title.clone(), directory: s.directory.clone() }).collect()
    }

    fn save_tabs(&mut self) {
        // Keep titles fresh for sessions outside the loaded list next launch.
        for t in &mut self.tabs {
            if let Some(s) = self.sessions.iter().find(|s| s.id == t.id) {
                t.title = s.title.clone();
                t.directory = s.directory.clone();
            }
        }
        #[cfg(test)]
        tabs::SAVED.with(|saved| *saved.borrow_mut() = Some(self.tabs.iter().map(|t| t.id.clone()).collect()));
        if let Source::Live { server, .. } = &self.source
            && self.persists()
        {
            tabs::save(&server.base, &self.tabs);
        }
    }

    /// A dropped tab: it takes its new place, and ⌘1–9 follow the new order.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.save_tabs();
    }

    /// The root of a session's family.
    pub fn root_of<'a>(&'a self, id: &'a str) -> &'a str {
        let mut cur = id;
        for _ in 0..32 {
            match self.session(cur).and_then(|s| s.parent.as_deref()) {
                Some(p) => cur = p,
                None => break,
            }
        }
        cur
    }

    /// A tab's mark, as Desktop decides it: busy while anything in the family runs (unless it's
    /// waiting on you), then nothing for the open tab, then attention before unread.
    pub fn tab_status(&self, id: &str) -> Option<TabStatus> {
        let attention = self.requests.iter().any(|(s, _)| self.root_of(s) == id);
        if !attention && self.running.iter().any(|s| self.root_of(s) == id) {
            return Some(TabStatus::Busy);
        }
        if self.route == Route::Session(id.to_owned()) {
            return None;
        }
        if attention {
            return Some(TabStatus::Attention);
        }
        self.session(id).is_some_and(SessionInfo::unread).then_some(TabStatus::Unread)
    }

    pub fn close_tab(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        let tab = self.tabs.remove(i);
        self.closed.push((i, tab));
        if self.route == Route::Session(id.to_owned()) {
            match self.tabs.get(i).or_else(|| self.tabs.get(i.wrapping_sub(1))).map(|t| t.id.clone()) {
                Some(next) => self.open(&next, tx),
                None => {
                    self.close_picker(false);
                    self.route = Route::Home;
                    tx.settle();
                }
            }
        }
        self.save_tabs();
    }

    /// The tab the open page belongs to, if any.
    fn current_tab(&self) -> Option<String> {
        let Route::Session(id) = &self.route else { return None };
        let root = self.root_of(id);
        self.tabs.iter().any(|t| t.id == root).then(|| root.to_owned())
    }

    fn select_adjacent(&mut self, offset: i32, unread_only: bool, tx: &mut Tx) {
        let order: Vec<&str> = self.tabs.iter().map(|t| t.id.as_str()).collect();
        let current = self.current_tab();
        let next = shortcuts::adjacent(&order, current.as_deref(), offset, |k| {
            !unread_only || (Some(k) != current.as_deref() && self.tab_status(k) == Some(TabStatus::Unread))
        });
        if let Some(id) = next {
            self.open(&id, tx);
        }
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
        let Some(action) = shortcuts::action(ev) else { return false };
        match action {
            // Zoom lands at once; the scale and every re-laid-out row would otherwise spring.
            Action::ZoomIn => { tx.instant(); self.set_zoom(1) }
            Action::ZoomOut => { tx.instant(); self.set_zoom(-1) }
            Action::ZoomReset => { tx.instant(); self.set_zoom(0) }
            Action::Tab(offset) => self.select_adjacent(offset, false, tx),
            Action::UnreadTab(offset) => self.select_adjacent(offset, true, tx),
            Action::TabAt(i) => {
                let id = if i == 8 { self.tabs.last() } else { self.tabs.get(i) }.map(|t| t.id.clone());
                if let Some(id) = id {
                    self.open(&id, tx);
                }
            }
            Action::NewSession => self.new_session(tx),
            Action::Settings => self.toggle_settings(tx),
            Action::CloseTab => {
                if let Some(id) = self.current_tab() {
                    self.close_tab(&id, tx);
                }
            }
            Action::ReopenTab => {
                while let Some((i, tab)) = self.closed.pop() {
                    if self.tabs.iter().any(|t| t.id == tab.id) {
                        continue;
                    }
                    let id = tab.id.clone();
                    self.tabs.insert(i.min(self.tabs.len()), tab);
                    self.open(&id, tx);
                    self.save_tabs();
                    break;
                }
            }
        }
        true
    }

    /// Opens a link from a message in the default browser (web and mail links only). Without a
    /// live server it only records the link, for tests.
    pub fn open_link(&mut self, url: &str) {
        if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")) {
            return;
        }
        self.opened.push(url.to_owned());
        if self.live() {
            let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
            let _ = std::process::Command::new(opener).arg(url).spawn();
        }
    }

    pub fn live(&self) -> bool {
        matches!(self.source, Source::Live { .. })
    }

    /// Saves tabs, zoom, the open session and drafts: live, with a person at it.
    pub fn persists(&self) -> bool {
        self.live() && !headless()
    }

    /// Asks the server for one session's info (live only).
    pub fn fetch_info(&self, id: &str) {
        if let Source::Live { requests: Some(r), .. } = &self.source {
            let _ = r.send(Request::Info(id.to_owned()));
        }
    }

    /// The first session in `id`'s parent chain (itself included) that isn't loaded, so a
    /// running subagent can be traced to its tab.
    fn missing_ancestor(&self, id: &str) -> Option<String> {
        let mut cur = id;
        for _ in 0..32 {
            match self.session(cur) {
                None => return Some(cur.to_owned()),
                Some(s) => match s.parent.as_deref() {
                    Some(p) => cur = p,
                    None => return None,
                },
            }
        }
        None
    }

    pub fn session(&self, id: &str) -> Option<&SessionInfo> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// Opens a session: lands at the bottom on the first frame with nothing animating in.
    pub fn open(&mut self, id: &str, tx: &mut Tx) {
        self.cancel_open(tx);
        if self.pages.is_some() && self.route != Route::Session(id.to_owned()) {
            self.begin_open(id, tx);
        } else {
            self.open_now(id, tx);
            self.open_page(id, tx);
        }
    }

    /// Commits the route, header and transcript together, once the opening gate is ready.
    fn open_now(&mut self, id: &str, tx: &mut Tx) {
        self.close_picker(false);
        self.activity_open = false;
        if self.route != Route::Session(id.to_owned()) {
            // Its timeline mounts afresh on its end, so "Jump to latest" from the last visit is stale.
            self.ends.forget(id);
        }
        self.route = Route::Session(id.to_owned());
        let root = self.root_of(id).to_owned();
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == root) {
            // Viewing it reads it (locally: this build never writes to the server).
            s.viewed = s.idle.max(s.viewed);
        }
        if !self.tabs.iter().any(|t| t.id == root) {
            let (title, directory) = self.session(&root).map(|s| (s.title.clone(), s.directory.clone())).unwrap_or_default();
            self.tabs.push(Tab { id: root, title, directory });
            self.save_tabs();
        }
        tx.settle();
        match &self.source {
            Source::Live { requests: Some(r), .. } => {
                if self.pages.is_none() { let _ = r.send(Request::Session(id.to_owned())); }
                let _ = r.send(Request::Pending(id.to_owned()));
                let _ = r.send(Request::Inbox(id.to_owned()));
            }
            Source::Live { .. } => {}
            Source::Fixture => self.start_stream_if_due(id),
        }
    }

    fn start_stream_if_due(&mut self, id: &str) {
        let stream = &self.fixture["stream"];
        if stream["session"].as_str() != Some(id) || stream["played"].as_bool() == Some(true) || flag("--no-stream") {
            return;
        }
        let at = self.clock;
        for e in stream["events"].as_array().into_iter().flatten() {
            let t = at + e["t"].as_f64().unwrap_or(0.0);
            self.playback.push_back((t, e["type"].as_str().unwrap_or("").to_owned(), e["data"].clone()));
        }
        self.fixture["stream"]["played"] = true.into();
        self.wake_at_playback();
    }

    /// Applies the fixture's streaming turn at once (the finished session).
    fn finish_stream(&mut self) {
        let events = self.fixture["stream"]["events"].as_array().cloned().unwrap_or_default();
        for e in events {
            self.apply_event(e["type"].as_str().unwrap_or(""), &e["data"]);
        }
        self.fixture["stream"]["played"] = true.into();
    }

    /// Windowed runs sleep between frames; a timer thread wakes the window for each event.
    fn wake_at_playback(&self) {
        let Some(w) = self.waker.clone() else { return };
        let times: Vec<f64> = self.playback.iter().map(|e| e.0 - self.clock).collect();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            for t in times {
                let d = std::time::Duration::from_secs_f64(t.max(0.0));
                if let Some(rest) = d.checked_sub(start.elapsed()) {
                    std::thread::sleep(rest);
                }
                w();
            }
        });
    }

    pub fn send(&mut self, tx: &mut Tx) {
        self.submit(false, tx);
    }

    /// Sends the composer's prompt. While the session is busy it steers, or queues when
    /// `alternate` (⌘⏎); see `queue.rs`.
    pub fn submit(&mut self, alternate: bool, tx: &mut Tx) {
        if self.opening.is_some() { return; }
        if self.add.shell {
            return self.submit_shell(alternate, tx);
        }
        let wire = self.mention_wire();
        if wire.text.is_empty() && !self.attach.any() {
            return;
        }
        if !self.attach.uploading.is_empty() {
            self.status = "Waiting for attachments to upload…".into();
            return;
        }
        self.close_mentions();
        let command = self.slash_command(&wire.text);
        // Commands take no path attachments; they wait in the composer for the next prompt.
        let refs = if command.is_none() { std::mem::take(&mut self.attach.paths) } else { Vec::new() };
        let text = match &command {
            Some((_, args)) => args.clone(),
            None => attachments::path_lines(&wire.text, &refs),
        };
        // The server's user message takes this id, so it replaces the local copy below.
        let prompt = Prompt {
            id: Some(opencode_kit::client::message_id()),
            text: text.clone(),
            files: wire.files.into_iter().chain(attachments::inputs(&self.attach.files)).collect(),
            agents: wire.agents,
            skills: wire.skills,
            display_text: Some(wire.display.clone()),
            delivery: None,
            attachments: refs,
            command: command.map(|(name, _)| name),
        };
        if prompt.command.is_some() {
            return self.send_command(prompt, alternate, tx);
        }
        if let Route::Session(id) = self.route.clone()
            && self.busy(&id)
        {
            let local = prompt.id.clone().unwrap_or_default();
            self.admit(&id, prompt, if alternate { Delivery::Queue } else { Delivery::Steer });
            tx.launch("prompt", if alternate { format!("queued-{local}") } else { format!("u-{local}/bubble") });
            return;
        }
        let id = match self.route.clone() {
            Route::Session(id) => id,
            Route::Draft if self.live() => {
                if self.write(Write::Start { directory: self.draft_dir.clone(), prompt }) {
                    self.status = "Starting a session…".into();
                    self.composer.clear();
                    self.attach.files.clear();
                }
                return;
            }
            _ => {
                self.status = "Pick a session to send to".into();
                return;
            }
        };
        let local = match &self.source {
            Source::Live { .. } => {
                let local = prompt.id.clone().unwrap_or_default();
                if !self.write(Write::Send { session: id.clone(), prompt }) {
                    return;
                }
                self.attach.echoes.push((id.clone(), local.clone(), text.clone()));
                local
            }
            Source::Fixture => {
                let selection = self.selection();
                self.sends += 1;
                let reply = format!("msg_zzzzreply{:04}", self.sends);
                let at = self.clock;
                for e in self.fixture["reply"].as_array().cloned().unwrap_or_default() {
                    let mut d = e["data"].clone();
                    d["sessionID"] = id.clone().into();
                    d["assistantMessageID"] = reply.clone().into();
                    if let Some(call) = d["id"].as_str() {
                        d["id"] = format!("{call}-{:04}", self.sends).into();
                    }
                    if e["type"] == "session.step.started" {
                        d["agent"] = selection.agent.clone().into();
                        d["model"] = serde_json::json!({"id": selection.model.id, "providerID": selection.model.provider, "variant": selection.model.variant});
                    }
                    self.playback.push_back((at + e["t"].as_f64().unwrap_or(0.0), e["type"].as_str().unwrap_or("").into(), d));
                }
                self.wake_at_playback();
                format!("msg_zzzzsent{:04}", self.sends)
            }
        };
        opencode_kit::model::set_user_files(&local, std::mem::take(&mut self.attach.files));
        // It rises out of the composer, its words where they were typed.
        tx.launch("prompt", format!("u-{local}/bubble"));
        let t = self.transcripts.entry(id.clone()).or_insert_with(|| Transcript { session: id.clone(), messages: vec![] });
        t.messages.push(Message::User { id: local, text: wire.display });
        self.composer.clear();
    }

    /// Queues a write to the server; false (with a status line) when this build can't write.
    pub fn write(&mut self, w: Write) -> bool {
        if self.story.is_some() {
            // A story's answers go nowhere; it acts as if they were sent.
            self.status = "Storybook: not sent".into();
            return true;
        }
        match &self.source {
            Source::Live { allow_send: false, .. } => self.status = "Read-only: relaunch without --read-only to act".into(),
            Source::Live { writes: Some(tx), .. } => return tx.send(w).is_ok(),
            _ => self.status = "Not connected to an OpenCode server".into(),
        }
        false
    }

    /// The open requests in a session's family (a subagent's question shows on its root).
    pub fn pending_for(&self, id: &str) -> Vec<&Pending> {
        let root = self.root_of(id);
        self.pending.iter().filter(|p| self.root_of(p.session()) == root).collect()
    }

    fn forget(&mut self, id: &str) {
        self.pending.retain(|p| p.id() != id);
        self.requests.retain(|(_, r)| r != id);
        let prefix = format!("{id}/");
        self.picks.retain(|k, _| !k.starts_with(&prefix));
        self.typed.retain(|k, _| !k.starts_with(&prefix));
        self.question.forget(id);
    }

    pub fn reply_permission(&mut self, id: &str, decision: &str) {
        let Some(p) = self.pending.iter().find(|p| p.id() == id) else { return };
        let w = Write::Permission { session: p.session().to_owned(), request: id.to_owned(), decision: decision.to_owned() };
        if self.write(w) {
            self.forget(id);
        }
    }

    pub fn typed_mut(&mut self, form: &str, key: &str) -> &mut TextEdit {
        self.typed.entry(format!("{form}/{key}")).or_insert_with(|| TextEdit::new(""))
    }

    /// The answer to send, or None while a required field is empty.
    pub fn form_answer(&self, id: &str) -> Option<Value> {
        let Some(Pending::Form { fields, .. }) = self.pending.iter().find(|p| p.id() == id) else { return None };
        let mut answer = serde_json::Map::new();
        for f in fields {
            let k = format!("{id}/{}", f.key);
            let typed = self.typed.get(&k).filter(|_| self.question.custom_on(&k)).map(|t| t.text.trim().to_owned()).filter(|t| !t.is_empty());
            let picks = self.picks.get(&k).cloned().unwrap_or_default();
            let v = match f.kind {
                FieldKind::Multi => {
                    let mut all = picks;
                    all.extend(typed);
                    (!all.is_empty()).then(|| Value::from(all))
                }
                FieldKind::Boolean => picks.first().map(|v| Value::Bool(v == "true")),
                FieldKind::Number => typed.and_then(|t| t.parse::<f64>().ok()).map(Value::from),
                FieldKind::Text => typed.or_else(|| picks.first().cloned()).map(Value::from),
            };
            match v {
                Some(v) => {
                    answer.insert(f.key.clone(), v);
                }
                None if f.required || fields.len() == 1 => return None,
                None => {}
            }
        }
        Some(Value::Object(answer))
    }

    pub fn submit_form(&mut self, id: &str) {
        let Some(answer) = self.form_answer(id) else {
            self.status = "Answer the questions first".into();
            return;
        };
        let Some(p) = self.pending.iter().find(|p| p.id() == id) else { return };
        if self.write(Write::Form { session: p.session().to_owned(), form: id.to_owned(), answer }) {
            self.forget(id);
        }
    }

    pub fn dismiss_form(&mut self, id: &str) {
        let Some(p) = self.pending.iter().find(|p| p.id() == id) else { return };
        if self.write(Write::CancelForm { session: p.session().to_owned(), form: id.to_owned() }) {
            self.forget(id);
        }
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
        if let Route::Session(id) = &self.route
            && let Some(dir) = self.session(id).map(|s| s.directory.clone()).filter(|d| !d.is_empty())
        {
            self.draft_dir = dir;
        }
        self.opening = None;
        self.route = Route::Draft;
        tx.settle();
    }

    /// Tab marks: running sessions, read state and pending requests.
    fn apply_status(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        let id = |v: &Value| v.as_str().unwrap_or("").to_owned();
        match ty {
            "session.execution.started" => {
                if let Some(missing) = self.missing_ancestor(sid) {
                    self.fetch_info(&missing);
                }
                self.running.insert(sid.to_owned())
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                let was = self.running.remove(sid);
                if ty == "session.execution.interrupted" && data["reason"] == "shutdown" {
                    return was;
                }
                let now = self.now_ms;
                // Desktop reports the open session's idle mark as viewed, so it never lights up.
                let open = matches!(&self.route, Route::Session(r) if self.root_of(r) == self.root_of(sid));
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    s.idle = Some(now);
                    if open {
                        s.viewed = Some(now);
                    }
                }
                // The server's own idle mark, like Desktop's re-read after a run ends.
                if let Source::Live { requests: Some(r), .. } = &self.source {
                    let _ = r.send(Request::Info(sid.to_owned()));
                }
                true
            }
            "session.viewed" => {
                let idle = data["idle"].as_i64();
                self.sessions.iter_mut().find(|s| s.id == sid).is_some_and(|s| {
                    s.viewed = s.viewed.max(idle);
                    true
                })
            }
            "permission.asked" | "form.created" => {
                let parsed = if ty == "form.created" { opencode_kit::model::parse_form(data) } else { opencode_kit::model::parse_permission(data) };
                let Some(p) = parsed else { return false };
                self.requests.insert((p.session().to_owned(), p.id().to_owned()));
                if !self.pending.iter().any(|x| x.id() == p.id()) {
                    self.pending.push(p);
                }
                true
            }
            "permission.replied" | "form.replied" | "form.cancelled" => {
                let rid = if ty == "permission.replied" { id(&data["requestID"]) } else { id(&data["id"]) };
                let had = self.requests.iter().any(|(_, r)| *r == rid) || self.pending.iter().any(|p| p.id() == rid);
                self.forget(&rid);
                had
            }
            _ => false,
        }
    }

    /// `session.revert.staged` hides the transcript from its boundary on, `cleared` shows it again, and
    /// `committed` ends the revert and drops what it hid: the steers waiting behind it and their glows
    /// (the transcript's own rows go in `Transcript::apply`).
    fn apply_revert(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        let next = match ty {
            "session.revert.staged" => data["revert"]["messageID"].as_str().map(str::to_owned),
            "session.revert.cleared" | "session.revert.committed" => None,
            _ => return false,
        };
        let mut changed = false;
        if let Some(info) = self.sessions.iter_mut().find(|i| i.id == sid) {
            changed |= std::mem::replace(&mut info.revert, next) != info.revert;
        }
        if ty == "session.revert.committed"
            && let Some(to) = data["to"].as_str()
        {
            if let Some(inbox) = self.inboxes.get_mut(sid) {
                let before = inbox.items.len();
                inbox.items.retain(|i| i.id.as_str() < to);
                changed |= inbox.items.len() != before;
            }
            if let Some(t) = self.transcripts.get(sid) {
                for m in &t.messages[t.visible_len(Some(to))..] {
                    changed |= self.glows.remove(m.id()).is_some();
                }
            }
        }
        changed
    }

    fn apply_event(&mut self, ty: &str, data: &Value) -> bool {
        let sid = data["sessionID"].as_str().unwrap_or("").to_owned();
        let now = self.now_ms;
        let mut changed = self.apply_status(ty, data, &sid);
        if ty.starts_with("session.inbox.") && !sid.is_empty() {
            changed |= self.apply_inbox(ty, data, &sid);
        }
        if ty.starts_with("session.revert.") && !sid.is_empty() {
            changed |= self.apply_revert(ty, data, &sid);
        }
        if let Some(t) = self.transcripts.get_mut(&sid) {
            changed |= t.apply(ty, data, now);
        }
        if ty.starts_with("session.") && !sid.is_empty() {
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                if s.updated < now - 1000 || ty == "session.step.ended" {
                    s.updated = now;
                    changed = true;
                }
                if ty == "session.renamed"
                    && let Some(t) = data["title"].as_str()
                {
                    s.title = t.to_owned();
                    changed = true;
                }
            } else if ty == "session.created" || ty == "session.step.started" {
                // A session we haven't listed: refresh the list (one GET).
                if let Source::Live { requests: Some(r), .. } = &self.source {
                    let _ = r.send(Request::Sessions);
                }
            }
            // A step started in the open session may follow a prompt we haven't seen.
            if ty == "session.step.started"
                && self.route == Route::Session(sid.clone())
                && let Source::Live { requests: Some(r), .. } = &self.source
            {
                if let Some(pages) = &self.pages {
                    pages.request(session_pages::Key { id: sid.clone(), updated: self.session(&sid).map_or(0, |s| s.updated) }, true);
                } else { let _ = r.send(Request::Session(sid)); }
            }
        }
        if ty == "project.updated"
            && let Some(p) = self.projects.iter_mut().find(|p| Some(p.id.as_str()) == data["id"].as_str())
        {
            p.active = data["time"]["active"].as_i64().unwrap_or(p.active);
            if data.get("canonical").is_some() {
                p.icon = opencode_kit::model::ProjectIcon::parse(&data["icon"]);
            }
        }
        changed
    }
}

fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    s.clock = now;
    s.now_ms = if s.live() { wall_ms() } else { s.base_ms + (now * 1000.0) as i64 };
    // A story's steps first: the events they play land below as arrivals.
    let mut changed = stories::poll(s, now, tx);
    changed |= s.poll_pages(tx);
    let arrivals = arrivals::Arrivals::before(s);
    let asked = s.question_shown();
    if let Some(inbox) = s.inbox.take() {
        for m in inbox.try_iter() {
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
                    let first = !s.transcripts.contains_key(&session);
                    let t = s.transcripts.entry(session.clone()).or_insert_with(|| Transcript { session: session.clone(), messages: vec![] });
                    attachments::reconcile(&mut s.attach.echoes, t, &messages);
                    let delivered: Vec<String> = messages.iter().map(|m| m.id().to_owned()).collect();
                    t.merge(messages);
                    s.settle_inbox(&session, &delivered);
                    if first && s.route == Route::Session(session) {
                        // Guarantee 8: the first load of the open session lands, nothing grows in.
                        tx.settle();
                    }
                }
                Incoming::Event { ty, data } => {
                    s.apply_event(&ty, &data);
                }
                Incoming::Status(e) => s.status = e,
                Incoming::Pending { session, items } => {
                    s.pending.retain(|p| p.session() != session || items.iter().any(|i| i.id() == p.id()));
                    for p in items {
                        s.requests.insert((p.session().to_owned(), p.id().to_owned()));
                        if !s.pending.iter().any(|x| x.id() == p.id()) {
                            s.pending.push(p);
                        }
                    }
                }
                Incoming::Created(info) => {
                    let id = info.id.clone();
                    if s.session(&id).is_none() {
                        s.sessions.insert(0, info);
                    }
                    s.status = format!("Live · {}", DEFAULT_SERVER.trim_start_matches("http://"));
                    s.open(&id, tx);
                }
            }
            changed = true;
        }
        s.inbox = Some(inbox);
    }
    while s.playback.front().is_some_and(|e| e.0 <= now + 1e-9) {
        let (_, ty, mut data) = s.playback.pop_front().unwrap();
        // A scripted delivery names the oldest prompt of its kind, which has no id until sent.
        if let Some(kind) = Delivery::parse(&data["next"])
            && let Some(item) = s.inboxes.get(data["sessionID"].as_str().unwrap_or("")).and_then(|i| i.items.iter().find(|i| i.delivery == kind))
        {
            data["inboxID"] = item.id.clone().into();
        }
        s.apply_event(&ty, &data);
        changed = true;
    }
    arrivals.glow(s, now);
    if s.question_shown() != asked {
        s.morph(tx);
    }
    let glow = knobs::glow_secs(&s.knobs);
    s.glows.retain(|_, at| now - *at < glow);
    if !s.glows.is_empty() {
        tx.wake_after(std::time::Duration::ZERO);
        changed = true;
    }
    changed |= s.poll_switcher(tx);
    changed |= s.poll_mentions(tx);
    changed |= s.poll_commands();
    changed |= s.poll_uploads();
    if let Source::Live { server, .. } = &s.source
        && s.persists()
    {
        let base = server.base.clone();
        s.poll_restore(&base);
        if !s.framed {
            s.framed = mac_app::remember_frame("OpenCodeRuneMain");
        }
    }
    changed
}

fn setup(s: &mut App, waker: Waker) {
    s.waker = Some(waker.clone());
    s.attach.set_waker(waker.clone());
    s.icons.set_waker(waker.clone());
    s.icons.set_sync(headless());
    if let Some(style) = icons::style_arg() {
        s.knobs.set(&knobs::ICON_STYLE, style);
    }
    if !s.playback.is_empty() {
        s.wake_at_playback();
    }
    // Running sessions (subagents) whose family isn't loaded yet.
    let mut missing: Vec<String> = s.running.iter().filter_map(|id| s.missing_ancestor(id)).collect();
    missing.sort();
    missing.dedup();
    if let Source::Live { server, requests, writes, .. } = &mut s.source {
        s.pages = Some(session_pages::Pages::live(server.clone(), waker.clone()));
        let media = server.clone();
        s.attach.set_fetch(std::sync::Arc::new(move |hash: &str| media.media(hash)));
        let (r, w, inbox) = opencode_kit::client::connect(server.clone(), waker);
        *writes = Some(w);
        // Tabs for sessions past the listed page: their info, so marks and avatars resolve.
        for t in s.tabs.iter().filter(|t| !s.sessions.iter().any(|x| x.id == t.id)) {
            let _ = r.send(Request::Info(t.id.clone()));
        }
        for id in missing {
            let _ = r.send(Request::Info(id));
        }
        *requests = Some(r);
        s.inbox = Some(inbox);
        // The session open at last quit (see `restore`) subscribes like a click on its tab.
        if let Route::Session(id) = s.route.clone() {
            s.open(&id, &mut Tx::default());
        }
    }
    // `--session <id or title words>` opens a session at start.
    if let Some(raw) = arg("--session") {
        let q = raw.to_lowercase();
        let found = s.sessions.iter().find(|x| x.id == raw).or_else(|| s.sessions.iter().find(|x| x.title.to_lowercase().contains(&q))).map(|x| x.id.clone());
        if let Some(id) = found {
            s.open(&id, &mut Tx::default());
        }
    }
    // Small newest pages, on the speculative lane after foreground startup work is queued.
    let mut recent: Vec<_> = s.sessions.iter().filter(|s| s.parent.is_none()).collect();
    recent.sort_by_key(|s| std::cmp::Reverse(s.updated));
    for session in recent.into_iter().take(session_pages::WARM) { s.prefetch_session(&session.id); }
    // Deterministic headless snapshots have no wall-clock event loop to await the worker.
    if headless() && let (Route::Session(id), Source::Live { server, .. }) = (&s.route, &s.source)
        && !s.transcripts.contains_key(id)
        && let Ok(v) = server.read(&format!("/api/session/{id}/message?limit={}&order=desc", session_pages::LATEST))
    {
        s.transcripts.insert(id.clone(), Transcript { session: id.clone(), messages: opencode_kit::model::parse_messages(&v) });
    }
}

fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if knobs::key(s, ev, tx) || stories::key(s, ev, tx) || s.switcher_key(ev, tx) || s.settings_key(ev, tx) || (s.opening.is_none() && s.question_key(ev, tx)) || s.add_key(ev, tx) || s.shortcut(ev, tx) || s.picker_key(ev, tx) { return true; }
    match &ev.key {
        Key::Command('e') => {
            s.edge = (s.edge + 1) % edge::ALL.len();
            true
        }
        Key::Command(c) if c.eq_ignore_ascii_case(&'b') && ev.mods.shift => {
            s.sidebar_open = !s.sidebar_open;
            true
        }
        Key::Command('b') => {
            s.opening = None;
            s.route = if s.route == Route::Home { s.tabs.last().map_or(Route::Home, |t| Route::Session(t.id.clone())) } else { Route::Home };
            tx.settle();
            true
        }
        Key::Escape if s.opening.is_some() => {
            s.cancel_open(tx);
            true
        }
        Key::Escape if s.composer.focused && matches!(&s.route, Route::Session(id) if s.busy(id)) => {
            s.stop();
            true
        }
        Key::Escape if s.activity_open => {
            tx.instant();
            s.activity_open = false;
            true
        }
        Key::Escape if s.story.is_some() => {
            stories::close(s, tx);
            true
        }
        // A subagent's session has no composer: Esc goes back to the session that started it.
        Key::Escape if s.open_parent(tx) => true,
        Key::Escape if s.route != Route::Home && !s.composer.focused => {
            s.route = Route::Home;
            tx.settle();
            true
        }
        _ => false,
    }
}

fn open_scene(s: &mut App) {
    let id = s.fixture["open"].as_str().map(str::to_owned).or_else(|| s.sessions.first().map(|x| x.id.clone()));
    if let Some(id) = id {
        s.route = Route::Session(id.clone());
    }
}

/// The Dock icon, set at launch so it shows for bare binaries and survives a reinstall
/// that LaunchServices hasn't re-read.
#[cfg(target_os = "macos")]
fn dock_icon() {
    use objc2::{class, msg_send, runtime::AnyObject};
    static ICNS: &[u8] = include_bytes!("../assets/icon.icns");
    unsafe {
        let data: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: ICNS.as_ptr().cast::<std::ffi::c_void>(), length: ICNS.len()];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
    }
}

fn busy_scene(s: &mut App) {
    open_scene(s);
    s.finish_stream();
    if let Route::Session(id) = s.route.clone() {
        if let Some(Message::Assistant(a)) = s.transcripts.get_mut(&id).and_then(|t| t.messages.last_mut()) {
            a.completed = None;
        }
        s.running.insert(id);
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    dock_icon();
    let spec = AppSpec {
        scenes: &[
            ("home", |_| {}),
            ("open-before", |s| open_story::scene(s, true)),
            ("open-after", |s| open_story::scene(s, false)),
            ("session", |s| {
                open_scene(s);
                s.finish_stream();
            }),
            ("streaming", |s| {
                open_scene(s);
                // Playback starts on the first poll.
                if let Some(id) = s.fixture["open"].as_str().map(str::to_owned) {
                    s.start_stream_if_due(&id);
                }
            }),
            // The open session's turn still running, so Enter steers and ⌘⏎ queues.
            ("busy", busy_scene),
            // Busy, then the server takes the steer at 5.6 s and the queued prompts at 6.6 and
            // 7.6 s: with `scripts/steer-dock.json`, a steer and two queued prompts delivered.
            ("busy-deliver", |s| {
                busy_scene(s);
                let Route::Session(id) = s.route.clone() else { return };
                let next = |kind: &str| serde_json::json!({ "sessionID": id, "next": kind });
                s.playback = [(5.6, "session.inbox.delivered".into(), next("steer")), (6.6, "session.inbox.delivered".into(), next("queue")), (7.6, "session.inbox.delivered".into(), next("queue"))].into();
            }),
            ("draft", |s| s.route = Route::Draft),
            ("icon-playground", |s| s.playground = &[false, true]),
            ("icon-playground-light", |s| s.playground = &[false]),
            ("icon-playground-dark", |s| s.playground = &[true]),
            ("settings", |s| {
                open_scene(s);
                s.finish_stream();
                s.open_settings();
            }),
            ("question", |s| {
                open_scene(s);
                s.finish_stream();
                let q = s.fixture["question"].clone();
                s.apply_event("form.created", &q);
            }),
            // Answered elsewhere at 1.5 s, asked again at 2.5 s: the morph both ways.
            ("question-morph", |s| {
                open_scene(s);
                s.finish_stream();
                let q = s.fixture["question"].clone();
                s.apply_event("form.created", &q);
                let replied = serde_json::json!({ "id": q["id"], "sessionID": q["sessionID"] });
                s.playback = [(1.5, "form.replied".into(), replied), (2.5, "form.created".into(), q)].into();
            }),
            // The storybook (⌘⇧D) on a story: `story-morph` is the dock opening and closing on a
            // loop (pick the curve with `--preset NAME`, a Question dock preset in `knobs.rs`);
            // `story` alone opens the first. Scripts replay a story with its trigger keys.
            ("story", |s| stories::scene(s, "Composer")),
            ("story-morph", |s| stories::scene(s, "Morph")),
            ("story-long", |s| stories::scene_variant(s, "Question", crate::stories::QUESTION, 3)),
            ("story-typing", |s| stories::scene_variant(s, "Question", crate::stories::QUESTION, 4)),
            ("story-pending", |s| stories::scene_variant(s, "Composer", crate::stories::COMPOSER, 2)),
            ("story-glow", |s| stories::scene(s, "Arrival glow")),
            ("story-send", |s| stories::scene(s, "Send")),
            ("story-stream", |s| stories::scene(s, "Streaming text")),
            ("story-tools", |s| stories::scene(s, "Tool rows")),
            ("story-working", |s| stories::scene(s, "Working")),
            ("story-steer", |s| stories::scene(s, "Steer and Pending")),
            ("story-queue", |s| stories::scene(s, "Queue dock")),
            ("story-subagent", |s| stories::scene(s, "Subagent")),
            ("story-images", |s| stories::scene(s, "Images")),
            ("story-jump", |s| stories::scene(s, "Jump to latest")),
            ("story-actions", |s| stories::scene(s, "Turn actions")),
            ("sidebar-closed", |s| {
                open_scene(s);
                s.finish_stream();
                s.sidebar_open = false;
            }),
            ("status", |s| {
                open_scene(s);
                s.finish_stream();
                for e in s.fixture["status_events"].as_array().cloned().unwrap_or_default() {
                    s.playback.push_back((e["t"].as_f64().unwrap_or(0.0), e["type"].as_str().unwrap_or("").into(), e["data"].clone()));
                }
            }),
        ],
        fonts: &[
            include_bytes!("../assets/fonts/InterVariable.ttf"),
            include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
            include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
        ],
        families: ("Inter", "IBM Plex Mono"),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("OpenCode (Rune)", (1280.0, 820.0), App::new, view::app)
    };
    #[cfg(feature = "hot")]
    rune::run(spec);
    #[cfg(not(feature = "hot"))]
    rune::App::new(spec)
    .chrome(rune::Chrome {
        full_size_content: true,
        traffic_lights: Some((25.0, 25.0)),
        drag_regions: true,
    })
    .run();
}
