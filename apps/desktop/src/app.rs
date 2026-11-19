//! Window state (`App`), launch-mode helpers, fixture/live bootstrapping, keyboard shortcuts, and
//! idle memory eviction (`App::rest`).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};

use opencode_kit::client::{DEFAULT_SERVER, Incoming, Request, Server, Write};
use opencode_kit::model::{Inbox, Pending, Project, SessionInfo, Transcript};
use opencode_kit::servers::ServerKey;
use rune::{KeyEvent, TextEdit, Tx, Waker};
use serde_json::Value;

use crate::pickers::PickerHost as _;
use crate::shell::app_state::{
    AppearanceState, ComposerExt, DiagState, DialogStates, FixtureRun, PlatformServices,
    QueueState, ShellGestures, TimelineFx, WorkspacePanes,
};
use crate::tabs::Tab;
use crate::{
    Route, appearance, background, commands, connect, context_window, demo, dictation, dirs, egg,
    events, files, find, fulltext, history, home_search, inbox_model, jobs, keymap, knobs, launch,
    layout_state, model_prefs, open_in, permission, plugin_host, projects, prompt_history,
    question, recent_page, restore, send_failed, servers, session_catalog, session_pages,
    session_status, settings, shortcuts, slash, stop, subagents, switcher, tabs, telemetry,
    turn_diffs, view, wall_ms, windows,
};

// A static, not a const: a const's bytes are copied into every codegen unit that names it.
pub(crate) static FIXTURE: &str = include_str!("../assets/fixture.json");

pub use opencode_runtime::TabStatus;

#[derive(Default)]
pub enum Source {
    Live { server: Server, requests: Option<Sender<Request>>, writes: Option<Sender<Write>>, allow_send: bool },
    #[default]
    Fixture,
}

/// Kernel state slot for background tool-output save error notifications.
#[derive(Default)]
pub(crate) struct OutputErrors(pub(crate) Option<(Sender<String>, Receiver<String>)>);

/// Kernel state slot counting how many times the root view has run; compared against `PolledRuns`
/// in `tick.rs` so polls know whether derived state may have moved.
#[derive(Default)]
pub(crate) struct ViewRuns(pub(crate) std::cell::Cell<u64>);

#[derive(Default)]
pub struct App {
    /// Decoupled shell kernel registries (lifecycle scheduler, typed feature slots, overlays,
    /// commands, deferred event bus, and native surface catalog).
    pub kernel: opencode_kernel::Kernel<App>,
    pub source: rune::Tracked<Source>,
    pub(crate) inbox: rune::Tracked<Option<Receiver<Incoming>>>,
    pub(crate) pages: rune::Tracked<session_pages::State>,
    pub(crate) opening: rune::Tracked<Option<session_pages::Opening>>,
    /// The server's projects and sessions. This and the other server data (transcripts, running,
    /// stops, requests, pending, inboxes) are the server's copy, which every window of the server
    /// holds in turn (`windows::peer`).
    pub projects: rune::Tracked<Vec<Project>>,
    pub sessions: rune::Tracked<Vec<SessionInfo>>,
    pub recent_page: rune::Tracked<recent_page::Page>,
    pub recent_reload: recent_page::Reload,
    pub(crate) session_search: switcher::search_index::SessionSearch,
    /// The palette's and Home's questions to the message text kept on this device.
    pub(crate) fulltext: fulltext::Surfaces,
    pub route: rune::Tracked<Route>,
    /// Where ⌘[ and ⌘] go (`history.rs`).
    pub nav: rune::Tracked<history::History>,
    pub transcripts: rune::Tracked<HashMap<String, Transcript>>,
    pub(crate) transcript_edits: events::TranscriptEdits,
    /// Pinned session tabs, in sidebar order (see `tabs.rs` for where they come from).
    pub tabs: rune::Tracked<Vec<Tab>>,
    /// The tab layout's switch and the collapsed sidebar's peek (`layout_state.rs`).
    pub layout: rune::Tracked<layout_state::Shell>,
    /// The Inbox layout's request times, branches and height (`inbox_model.rs`).
    pub sidebar_inbox: rune::Tracked<inbox_model::SidebarInbox>,
    /// UI zoom, 1.0 = 100% (see `view::app`).
    pub zoom: rune::Tracked<f32>,
    /// Changed files' diffs (`turn_diffs.rs`).
    pub turn_diffs: rune::Tracked<turn_diffs::TurnDiffs>,
    /// Which of each session's messages Desktop would hold, for the Context tab (`context_window`).
    pub context_windows: rune::Tracked<context_window::Windows>,
    /// Sessions running right now (any depth; a tab is busy when anything in its family runs).
    pub running: rune::Tracked<HashSet<String>>,
    /// Runs stopped on a press, until the server confirms (`stop.rs`).
    pub stops: rune::Tracked<stop::Stops>,
    /// Pending permission and form requests: (session, request id).
    pub requests: rune::Tracked<HashSet<(String, String)>>,
    /// Which sessions' requests this connection has read (`session_status.rs`).
    pub request_reads: rune::Tracked<session_status::RequestReads>,
    /// The open permission requests and forms we know the details of.
    pub pending: rune::Tracked<Vec<Pending>>,
    /// Prompts sent while a session was busy that the server hasn't delivered yet, by session.
    pub inboxes: rune::Tracked<HashMap<String, Inbox>>,
    /// This window's place in its server's peer (`windows::peer`).
    pub store: rune::Tracked<Box<windows::peer::Link>>,
    /// The question dock's place and in-progress answers for the forms it shows.
    pub question: rune::Tracked<question::Dock>,
    /// Where a new draft starts, and where paths resolve off a session: the folder last picked.
    /// The open draft's own folder is `App::draft_dir`.
    pub default_dir: rune::Tracked<String>,
    /// The home folder paths show as `~`: the user's, or the fixture's in a fixture run.
    pub home: rune::Tracked<String>,
    pub sidebar_open: rune::Tracked<bool>,
    pub sidebar_w: rune::Tracked<f32>,
    pub composer: rune::Tracked<TextEdit>,
    pub search: rune::Tracked<TextEdit>,
    pub home_search: rune::Tracked<Box<home_search::HomeSearch>>,
    /// The new-session wordmark's Easter egg (`egg`): nothing until the pointer comes near its "e".
    /// Untracked: its frames step and draw without the view running; `egg::poll` says when the
    /// view must.
    pub egg: egg::Egg,
    pub project_state: rune::Tracked<Box<projects::ProjectsState>>,
    /// Home across servers: its selection, its servers and its list's phase (`projects::home`).
    pub home_state: rune::Tracked<projects::home::Home>,
    pub settings: rune::Tracked<settings::Settings>,
    /// Settings' Providers, Models and Extensions pages (`settings::pages`).
    pub settings_pages: rune::Tracked<Box<settings::pages::State>>,
    /// The integrations and providers it and Settings read, by folder.
    pub catalogs: rune::Tracked<connect::catalog::Catalogs>,
    /// Server requests on their way back (`jobs`).
    pub(crate) jobs: rune::Tracked<jobs::Jobs>,
    /// The open session and composer drafts, saved for the next launch.
    pub restore: rune::Tracked<Box<restore::Restore>>,
    /// Background shells, and the running-work list above the composer (`subagents::activity`).
    pub shells: rune::Tracked<subagents::shells::Shells>,
    /// Permission replies still on their way, to bring back if the server refuses one.
    pub permission_sent: rune::Tracked<permission::Sent>,
    /// Prompts sent and not yet seen back, for rolling a failed one back (`send_failed.rs`).
    pub unsent: rune::Tracked<send_failed::Log>,
    /// Wall-clock milliseconds for relative times (fixture: its own fixed "now").
    pub now_ms: rune::Tracked<i64>,
    pub(crate) clock: rune::Tracked<f64>,
    pub(crate) waker: rune::Tracked<Option<Waker>>,
    pub(crate) launch: rune::Tracked<launch::Launch>,
    /// The plugins: their panels, commands, menus, settings and MCP tools (`plugin_host`).
    pub plugins: rune::Tracked<Box<plugin_host::PluginHost>>,
    /// The server the window works against now and its halves of the others (`servers::runtime`).
    pub checkout: rune::Tracked<servers::runtime::Checkout>,
    /// The session header's location card: each folder's branch once read.
    pub location: rune::Tracked<view::location::Location>,
    /// Which window this is, and what it keeps of itself (`windows`).
    pub window: rune::Tracked<windows::Window>,
    /// Fixture playback, demo script, and storybook playground state.
    pub fixture_run: FixtureRun,
    /// Diagnostics, debug bar, live knobs, frame recorder, and motion log state.
    pub diag: DiagState,
    /// Modal dialogs, popovers, switchers, menus, toasts, and onboarding tips.
    pub dialogs: DialogStates,
    /// Side/bottom workspace surfaces, panes, external openers, and client MCP hub.
    pub workspace_panes: WorkspacePanes,
    /// Shell gestures: tab drag/preview/reopen, pinch zoom, sidebar resize, and Home return tab.
    pub gestures: ShellGestures,
    /// Timeline presentation, animations, disclosures, hover state, plan drawer, find, and nav.
    pub timeline_fx: TimelineFx,
    /// Theme, motion, text rendering, background, locale, project icons, and window frame state.
    pub appearance_state: AppearanceState,
    /// Composer auxiliary inputs: history, attachments, quotes, comments, pickers, mentions, slash,
    /// suggestions, dictation, and toast undo stacks.
    pub composer_ext: ComposerExt,
    /// Prompt queue mutations, drag/rewrite runs, held sends, and optimistic session starts.
    pub queue: QueueState,
    /// Platform and connectivity services: notifications, deep links, updater, reconnect links,
    /// and session availability states.
    pub services: PlatformServices,
}

pub(crate) fn arg(name: &str) -> Option<String> {
    let a = rune::extra_args();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

pub(crate) fn flag(name: &str) -> bool {
    rune::extra_args().iter().any(|x| x == name)
}

/// A run that renders without a person at it (snapshots, trees, scripts, videos). It can read a
/// live server but never sends to it or writes the saved tabs, zoom, session and drafts, so a
/// test can't post or type as Kit.
pub(crate) use rune::is_headless as headless;

/// `--harness-writes`, honoured only when `RUNE_DIRS_ROOT` moves every folder this app writes:
/// a headless run (the parity harness's `--realtime` probes) sends, stops and saves as a window
/// does, into its own root and to the server it was given.
pub(crate) fn harness_writes() -> bool {
    flag("--harness-writes") && dirs::rooted()
}

/// A person, or the harness standing in for one, is at this run: it may write.
pub(crate) fn attended() -> bool {
    !headless() || harness_writes()
}

/// On a touch screen (`touch_controls()`), only a direct tap inside a text field or a hardware key
/// (`tx.by_keys()`) puts focus in the composer; opening a session or tab, leaving Settings or a
/// dialog, or tapping a toolbar button must not pop up the OS keyboard.
pub(crate) fn auto_focus_composer(tx: &Tx) -> bool {
    !session_view::style::touch_controls() || tx.by_keys()
}

/// As in Desktop: the server's image for a project or its letter, never a file found in its
/// folder (Seer's repo holds this app's icon, `~/code` every app's).
fn desktop_icons() -> project_icon::Icons {
    let mut icons = project_icon::Icons::default();
    icons.set_repo_icons(false);
    icons
}

impl App {
    pub fn toggle_expanded(&mut self, key: &str) -> bool {
        session_view::disclosure::toggle_expanded(self, key)
    }

    pub fn shutting(&self, key: &str) -> bool {
        session_view::disclosure::is_shutting(self, key)
    }

    pub fn mark_copied(&mut self, key: impl Into<String>, tx: &mut Tx) {
        session_view::copied::mark_copied(self, key, tx);
    }

    pub fn copied(&self, key: &str) -> bool {
        session_view::copied::is_copied(self, key)
    }

    pub(crate) fn view_runs(&self) -> u64 {
        self.kernel.slots.get::<ViewRuns>().map_or(0, |r| r.0.get())
    }

    pub(crate) fn bump_view_runs(&self) {
        if let Some(r) = self.kernel.slots.get::<ViewRuns>() {
            r.0.set(r.0.get().wrapping_add(1));
        }
    }

    pub(crate) fn empty() -> App {
        crate::init_features();
        let home = dirs::home();
        App {
            kernel: crate::kernel::build(),
            default_dir: arg("--directory").unwrap_or_else(|| home.clone()).into(),
            home: home.into(),
            sidebar_open: (!flag("--sidebar-closed")).into(),
            sidebar_w: view::SIDEBAR_W.into(),
            zoom: arg("--zoom").and_then(|z| z.parse::<f32>().ok()).map_or(1.0, |z| z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX)).into(),
            composer: TextEdit::new("").into(),
            search: TextEdit::new("").into(),
            appearance_state: AppearanceState {
                reduced_motion: flag("--reduced-motion").into(),
                motion: appearance::Motion::new(flag("--reduced-motion")).into(),
                icons: desktop_icons().into(),
                ..Default::default()
            },
            diag: DiagState {
                debug: flag("--debug").into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Back to Home, dropping a session that was still opening.
    pub(crate) fn go_home(&mut self, tx: &mut Tx) {
        if self.route != Route::Home {
            *self.gestures.home_return_tab = self.current_tab();
        }
        self.close_settings();
        *self.opening = None;
        *self.route = Route::Home;
        tx.settle();
    }

    fn load(window: windows::Window) -> App {
        let mut app = Self::empty();
        *app.window = window;
        let fixture_path = arg("--fixture");
        let server = launch::initial_server();
        let deferred = fixture_path.is_none() && launch::deferred();
        let want_live = fixture_path.is_none() && !demo::requested() && (deferred || flag("--live") || (!headless()));
        if want_live && app.load_live(server, false) { return app; }
        let src = match &fixture_path {
            Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}")),
            None => FIXTURE.to_owned(),
        };
        app.load_fixture(&src);
        // The demo plays on Kit's own zoom; a fixture run never saves it.
        if demo::requested() && arg("--zoom").is_none() && !headless() && let Some(z) = tabs::load_zoom() {
            *app.zoom = z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX);
        }
        app
    }

    pub(crate) fn new() -> App {
        // A harness run that writes keeps a real window's rows, so its tabs and drafts come back.
        Self::new_window(if harness_writes() { windows::first_saved() } else { windows::Window::unsaved() })
    }

    /// `new`, for window `window`: its own tabs and page.
    pub(crate) fn new_window(window: windows::Window) -> App {
        rune::marks::mark("state.load");
        let mut app = telemetry::startup("launch.load", || Self::load(window));
        appearance::apply_args(&mut app);
        if let Some(layout) = arg("--tab-layout") {
            app.settings.tab_layout = layout_state::TabLayout::parse(Some(&layout));
            *app.layout = layout_state::Shell::at_rest(app.settings.tab_layout);
        }
        app.apply_appearance();
        if let Some(name) = arg("--preset") {
            knobs::apply_preset(&mut app, &name);
        }
        // `--knob <name>=<value>` (repeatable): a run starts with that knob set, by its key or its
        // panel name and a choice's label or a number, as a capture needs an experiment on.
        let all = rune::extra_args();
        for pair in all.iter().zip(all.iter().skip(1)).filter(|(k, _)| *k == "--knob").map(|(_, v)| v) {
            knobs::apply_arg(&mut app, pair);
        }
        session_view::scroller::set_device_scale(windows::view_scale(&app));
        rune::marks::mark("state.restored");
        app
    }

    pub(crate) fn load_fixture(&mut self, src: &str) {
        let v: Value = serde_json::from_str(src).unwrap_or_else(|e| panic!("fixture JSON: {e}"));
        self.composer_ext.pickers.load(&v);
        self.composer_ext.mentions.load(&v);
        self.composer_ext.add.slash.commands = slash::commands(&v["commands"]);
        *self.fixture_run.base_ms = v["now"].as_i64().unwrap_or(0);
        *self.now_ms = *self.fixture_run.base_ms;
        *self.projects = opencode_kit::model::parse_projects(&v["projects"]);
        *self.sessions = opencode_kit::model::parse_sessions(&v["sessions"]);
        if let Some(map) = v["messages"].as_object() {
            for (id, list) in map {
                let msgs = opencode_kit::model::parse_messages(&serde_json::json!({ "data": list }));
                self.transcripts.insert(id.clone(), Transcript { session: id.clone(), messages: msgs });
            }
        }
        let ids: Vec<String> = v["tabs"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        *self.tabs = ids.iter().map(|id| Tab::session(id.clone(), "", "")).collect();
        *self.running = v["running"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        for r in v["requests"].as_array().into_iter().flatten() {
            if let (Some(s), Some(id)) = (r["sessionID"].as_str(), r["id"].as_str()) {
                self.requests.insert((s.to_owned(), id.to_owned()));
            }
        }
        *self.diag.status = "Fixture".into();
        *self.workspace_panes.open_in = open_in::OpenIn::with(std::sync::Arc::new(open_in::Fixed(vec!["vscode", "cursor", "zed", "terminal", "ghostty", "xcode", "powershell"])));
        self.workspace_panes.open_in.detect();
        if let Some(home) = v["home"].as_str() {
            *self.home = home.to_owned();
        }
        *self.fixture_run.fixture = v;
        *self.source = Source::Fixture;
        self.init_used_projects(None);
        self.sort_projects();
        if arg("--directory").is_none()
            && let Some(p) = self.projects.first()
        {
            *self.default_dir = p.directory.clone();
        }
    }

    pub(crate) fn load_live(&mut self, server: Server, fallback_fixture: bool) -> bool {
        // A headless snapshot reads the server now; a window reads it and probes after its first frame.
        let deferred = launch::deferred();
        let first = if deferred {
            None
        } else {
            match launch::First::fetch(&server) {
                Ok(f) => Some(f),
                Err(_) if fallback_fixture => return false,
                // No answer before the first frame: the launch tries again from `poll`.
                Err(_) => {
                    self.launch.set_phase(ServerKey::SIDECAR, launch::Phase::Offline { retries: 0, retry_at: 0.0 });
                    None
                }
            }
        };
        // The window starts on the app's own server; its tabs' other servers join as they're listed.
        let server = servers::runtime::bind(self, server);
        let doc = telemetry::startup("launch.window_doc", || restore::read_window(self));
        let saved_tabs = doc.tabs.as_ref().map(|list| list.iter().map(|t| Tab::from_doc(t, doc.info(&t.key()))).collect());
        self.restore.unread_tabs = doc.unread_tabs.clone();
        // Desktop's tabs come into a fresh profile's first window only.
        let beta = self.window.fresh && server.base == DEFAULT_SERVER;
        match saved_tabs.or_else(|| (beta && !deferred).then(tabs::from_beta).flatten()) {
            Some(tabs) => *self.tabs = tabs,
            None if deferred => self.launch.set_phase(ServerKey::SIDECAR, launch::Phase::Waiting { beta_tabs: beta, fallback_fixture }),
            None => {}
        }
        if deferred && self.launch.phase(ServerKey::SIDECAR).is_none() {
            self.launch.set_phase(ServerKey::SIDECAR, launch::Phase::Waiting { beta_tabs: false, fallback_fixture });
        }
        if let Some(first) = first {
            self.apply_first(first);
            // Read before the source goes live: this client's used projects, none on a fresh profile.
            self.init_used_projects(Some(tabs::load_projects(&server.base).unwrap_or_default()));
        }
        if let Some(z) = tabs::load_zoom() {
            *self.zoom = z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX);
        }
        *self.settings = settings::load();
        *self.layout = layout_state::Shell::at_rest(self.settings.tab_layout);
        self.load_open_in();
        if self.persists() {
            self.workspace_panes.files.wrap = files::markdown::WrapPrefs::load(self);
            *self.workspace_panes.panel_sidebar = plugin_host::native::sidebar::PanelSidebar::load();
        }
        *self.composer_ext.prompts = prompt_history::PromptHistory::load(attended());
        self.composer_ext.pickers.prefs = model_prefs::load();
        telemetry::startup("launch.restore_ui", || self.restore_ui(&doc, &server.base));
        *self.fixture_run.base_ms = wall_ms();
        *self.now_ms = *self.fixture_run.base_ms;
        let allow_send = !flag("--read-only") && attended();
        *self.source = Source::Live { server, requests: None, writes: None, allow_send };
        self.open_outbox();
        *self.diag.status = self.live_status();
        self.resume_settings_page();
        true
    }

    pub(crate) fn sort_projects(&mut self) {
        // Projects by their latest session, then by activity.
        let latest: HashMap<&str, i64> = self.sessions.iter().fold(HashMap::new(), |mut m, s| {
            let e = m.entry(s.project.as_str()).or_insert(0);
            *e = (*e).max(s.updated);
            m
        });
        let key = |p: &Project| latest.get(p.id.as_str()).copied().unwrap_or(0).max(p.active);
        self.projects.sort_by_key(|p| std::cmp::Reverse(key(p)));
    }

    pub fn set_zoom(&mut self, dir: i32) {
        *self.zoom = shortcuts::step_zoom(*self.zoom, dir);
        self.gestures.zoom_pinch.leveled();
        if self.persists() {
            tabs::save_zoom(*self.zoom);
        }
    }

    /// OpenCode Desktop's key defaults; true when `ev` was one.
    pub fn shortcut(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if settings::pages::shortcuts::run_bound(self, ev, tx) || dictation::key(self, ev, tx) || background::key(self, ev, tx) || find::shortcut(self, ev, tx) || commands::key(self, ev, tx) || plugin_host::key(self, ev, tx) {
            return true;
        }
        if tx.focused().is_some() && keymap::Bind::PastePlain.matches_tx(ev, tx) {
            tx.request_paste();
            return true;
        }
        if let Some(forward) = history::bracket(ev, tx.physical_key()) {
            self.navigate(forward, tx);
            return true;
        }
        shortcuts::run(self, ev, tx)
    }

    pub fn live(&self) -> bool {
        matches!(*self.source, Source::Live { .. })
    }

    /// Saves tabs, zoom, the open session and drafts: live, with a person at it.
    pub fn persists(&self) -> bool {
        self.live() && attended()
    }

    /// Asks the server for one session's info (live only).
    pub fn fetch_info(&self, id: &str) {
        if let Source::Live { requests: Some(r), .. } = &*self.source {
            r.send(Request::Info(id.to_owned())).ok();
        }
    }

    /// `App::on_rest`: the window rests (10 s without a frame, or out of sight). What it holds
    /// for sessions no tab, route or run shows goes: their transcripts, their pages in the
    /// reader's cache, and the images only those held. A session opened again is read again,
    /// as one opened for the first time is. Nothing on screen changes.
    pub(crate) fn rest(s: &mut App, _: &mut rune::Tx) -> bool {
        let scheduler = s.kernel.scheduler.clone();
        let has_rest_hooks = scheduler.rest_names().next().is_some();
        let shown: HashSet<&str> = s.tabs.iter().map(|t| t.id.as_str()).chain(match &*s.route { Route::Session { id, .. } => Some(id.as_str()), _ => None }).chain(s.running.iter().map(String::as_str)).collect();
        let kept = |id: &str| s.ancestors(id).any(|a| shown.contains(a));
        let gone: Vec<String> = s.transcripts.keys().filter(|id| !kept(id)).cloned().collect();
        let review_gone: Vec<String> = s.workspace_panes.review.sessions.keys().filter(|id| !kept(id)).cloned().collect();
        let files_gone: Vec<String> = s.workspace_panes.files.sessions.keys().filter(|id| !kept(id)).cloned().collect();
        let kept_ids: HashSet<String> = if has_rest_hooks {
            shown
                .iter()
                .map(|id| (*id).to_owned())
                .chain(s.sessions.iter().filter(|sess| kept(&sess.id)).map(|sess| sess.id.clone()))
                .collect()
        } else {
            HashSet::new()
        };
        s.pages.rest(&kept);
        s.workspace_panes.files.retain_reply_links(kept);
        if !gone.is_empty() {
            let transcripts = &mut *s.transcripts;
            for id in &gone { transcripts.remove(id); }
        }
        // A closed session's diffs and open files (their source lines, highlight chunks, images).
        if !review_gone.is_empty() { for id in &review_gone { s.workspace_panes.review.sessions.remove(id); } }
        if !files_gone.is_empty() {
            for id in &files_gone {
                s.workspace_panes.files.sessions.remove(id);
                s.workspace_panes.files.reads.forget_session(id);
                files::watch::forget_session(s, id);
            }
        }
        // What `opencode_kit` keeps beside each user message (files, mentions, comments, times):
        // only for the messages of the transcripts left.
        let cached = s.pages.user_message_ids();
        let messages: HashSet<&str> = s.transcripts.values().flat_map(|t| t.messages.iter()).filter_map(|m| match m { opencode_kit::model::Message::User { id, .. } => Some(id.as_str()), _ => None }).chain(cached.iter().map(String::as_str)).collect();
        opencode_kit::model::retain_user_messages(&|id| messages.contains(id));
        s.composer_ext.attach.store.rest();
        // No session tab or draft and not on a session: pictures keep their encoded bytes, not
        // their pixels.
        if s.tabs.is_empty() && !matches!(&*s.route, Route::Session { .. }) {
            s.composer_ext.attach.store.rest_pixels();
        }
        if has_rest_hooks {
            scheduler.run_rest(s, &|id| kept_ids.contains(id));
        }
        // What all that freed: mimalloc purges freed pages only when some thread next allocates,
        // and an app at rest doesn't. Forced, it gives them back now.
        #[cfg(not(target_arch = "wasm32"))]
        crate::purge_free_pages();
        false
    }

    /// `id` and its loaded parents, up to 32 steps.
    pub(crate) fn ancestors<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a str> {
        std::iter::successors(Some(id), |cur| self.session(cur)?.parent.as_deref()).take(33)
    }

    /// The first session in `id`'s parent chain (itself included) that isn't loaded, so a
    /// running subagent can be traced to its tab.
    pub(crate) fn missing_ancestor(&self, id: &str) -> Option<String> {
        self.ancestors(id).take(32).last().filter(|cur| self.session(cur).is_none()).map(str::to_owned)
    }

    pub fn session(&self, id: &str) -> Option<&SessionInfo> {
        if self.sessions.len() >= 512
            && let Some(cat) = self.kernel.slots.get::<session_catalog::Catalog>()
        {
            return cat.lookup(&self.sessions, id).map(|(i, _)| &self.sessions[i]);
        }
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// The composer's own session, as Desktop's session status. Background helpers have their
    /// own runs: they keep the activity chip busy, not this session's Send button.
    pub fn busy(&self, id: &str) -> bool {
        self.running.contains(id)
    }

    pub(crate) fn route_busy(&self) -> bool {
        matches!(&*self.route, Route::Session { id, .. } if self.busy(id))
    }

    /// `+`, ⌘T and New session: a new "New session" draft tab, in the open session's or draft's
    /// folder, as Desktop's `openDraft`. On Home and in Settings it's Home's project, else the
    /// last used or the first the server lists, and with no project it does nothing.
    pub fn new_session(&mut self, tx: &mut Tx) {
        if self.project_navigation() && (self.settings.open || self.route == Route::Home) {
            return view::home::browse::start(self, tx);
        }
        let dir = if self.settings.open || self.route == Route::Home {
            let project = self
                .home_selected()
                .or_else(|| self.last_used_project())
                .or_else(|| self.projects.first());
            let Some(project) = project else { return };
            Some(project.directory.clone())
        } else {
            match &*self.route {
                Route::Session { id, .. } => self.session(id).map(|s| s.directory.clone()),
                Route::Draft(_) => Some(self.draft_dir().to_owned()),
                Route::Home => None,
            }
        };
        self.close_settings();
        self.close_picker(false);
        self.close_project_switcher(false);
        self.project_state.home_menu_open = false;
        if let Some(dir) = dir.filter(|d| !d.is_empty()) {
            self.sidebar_inbox.branches.enqueue([dir.clone()]);
            *self.default_dir = dir;
        }
        *self.opening = None;
        self.open_draft_tab();
        if self.fixture_run.stage.is_none() && auto_focus_composer(tx) {
            self.composer.focused = true;
        }
        tx.settle();
    }
}
