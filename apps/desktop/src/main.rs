//! OpenCode Desktop in Rune: OpenCode's home and session pages in Rune, live against the local OpenCode server or
//! from a synthetic fixture. Live, it sends prompts, answers permission requests and questions,
//! stops runs and starts sessions (`--read-only` turns all of that off).

mod add_menu;
mod agent_rows;
mod agent_rows_view;
mod agent_stories;
mod arrivals;
mod attachments;
mod dictation;
#[cfg(test)]
mod dictation_tests;
mod events;
mod icons;
mod jump;
mod keys;
mod knobs;
mod launch;
mod mentions;
mod palette;
#[cfg(test)]
mod perf;
mod pickers;
mod queue;
mod question;
mod restore;
mod revert;
mod scenes;
mod send;
mod session_open;
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
mod tab_actions;
mod tabs;
mod turns;
mod view;
mod working;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender};

use rune::{AppSpec, Key, KeyEvent, TextEdit, Tx, Waker, edge};
use serde_json::Value;

use opencode_kit::client::{DEFAULT_SERVER, Incoming, Prompt, Request, Server, Write};
use opencode_kit::model::{Delivery, FieldKind, Inbox, Message, Pending, Project, SessionInfo, Transcript};
use keys::on_key;
pub(crate) use scenes::open_scene;
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
    /// Hex dictation into the composer (⇧⌘M or the mic).
    pub dictation: dictation::Dictation,
    pub switcher: switcher::Switcher,
    pub settings: settings::Settings,
    /// The open session and composer drafts, saved for the next launch.
    pub restore: restore::Restore,
    /// Links opened from messages, newest last.
    pub opened: Vec<String>,
    /// The running-subagents list above the composer is open.
    pub activity_open: bool,
    /// Subagents' latest steps and how their runs ended, for the Subagent rows styles.
    pub agent_rows: agent_rows::Live,
    /// Whether AppKit is saving the window frame yet (`mac_app::remember_frame`).
    framed: bool,
    pub edge: usize,
    pub reduced_motion: bool,
    /// User messages that arrived from another client, by id: the poll clock when they did.
    pub glows: HashMap<String, f64>,
    /// Where each session's timeline stands against its end (`jump.rs`).
    pub ends: jump::Ends,
    /// When each run started, for the Working label's rhythm (`working.rs`).
    pub working: working::Working,
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
    launch: launch::Launch,
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
            dictation: Default::default(),
            switcher: switcher::Switcher::default(),
            settings: settings::Settings::default(),
            restore: restore::Restore::default(),
            opened: Vec::new(),
            activity_open: false,
            agent_rows: Default::default(),
            knobs: rune_knobs::Knobs::default(),
            framed: false,
            edge,
            reduced_motion: flag("--reduced-motion"),
            glows: HashMap::new(),
            ends: jump::Ends::default(),
            working: working::Working::default(),
            debug: flag("--debug"),
            status: String::new(),
            now_ms: 0,
            base_ms: 0,
            clock: 0.0,
            fixture: Value::Null,
            playback: VecDeque::new(),
            sends: 0,
            waker: None,
            launch: Default::default(),
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
        // A headless snapshot reads the server now; a window reads it after its first frame.
        let first = if launch::deferred() {
            self.launch.waiting = true;
            None
        } else {
            let Ok(first) = launch::First::fetch(&server) else { return false };
            Some(first)
        };
        match tabs::load(&server.base).or_else(|| if server.base == DEFAULT_SERVER { tabs::from_beta() } else { None }) {
            Some(tabs) => self.tabs = tabs,
            None => self.launch.recent_tabs = true,
        }
        if let Some(first) = first {
            self.apply_first(first);
        }
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
        if dictation::key(self, ev, tx) {
            return true;
        }
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

}

fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    s.clock = now;
    s.now_ms = if s.live() { wall_ms() } else { s.base_ms + (now * 1000.0) as i64 };
    // A story's steps first: the events they play land below as arrivals.
    let mut changed = stories::poll(s, now, tx);
    changed |= dictation::poll(s, now, tx);
    changed |= launch::poll(s, tx);
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
                // Most events are other sessions' streams: they redraw only what they change.
                Incoming::Event { ty, data } => {
                    changed |= s.apply_event(&ty, &data);
                    continue;
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
                Incoming::Reconnected => {
                    tx.instant();
                    s.resync();
                }
                Incoming::Active(ids) => {
                    tx.instant();
                    s.running = ids.into_iter().collect();
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
    arrivals.land(s, now, tx);
    changed |= working::sync(s);
    if s.question_shown() != asked {
        s.morph(tx);
    }
    let glow = knobs::glow_secs(&s.knobs);
    s.glows.retain(|_, at| now - *at < glow);
    if !s.glows.is_empty() {
        tx.wake_after(std::time::Duration::ZERO);
        changed = true;
    }
    changed |= agent_rows::poll(s, tx);
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
    launch::setup(s);
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

fn main() {
    rune::prewarm_gpu();
    #[cfg(target_os = "macos")]
    dock_icon();
    let spec = AppSpec {
        scenes: scenes::SCENES,
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
    {
        if !headless() {
            launch::defer();
        }
        let app = rune::App::new(spec).chrome(rune::Chrome {
            full_size_content: true,
            traffic_lights: Some((25.0, 25.0)),
            drag_regions: true,
        });
        let handle = app.handle();
        app.on_ready(move |_| handle.update_if(|s, tx| { launch::ready(s, tx); false })).run();
    }
}
