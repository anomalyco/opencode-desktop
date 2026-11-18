//! Window state (`App`), launch-mode helpers, fixture/live bootstrapping, keyboard shortcuts, and
//! idle memory eviction (`App::rest`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender};

use opencode_kit::client::{DEFAULT_SERVER, Incoming, Request, Server, Write};
use opencode_kit::model::{Inbox, Pending, Project, SessionInfo, Transcript};
use opencode_kit::servers::ServerKey;
use rune::{KeyEvent, TextEdit, Tx, Waker};
use serde_json::Value;

use crate::pickers::PickerHost as _;
use crate::tabs::Tab;
use crate::{
    Route, add_menu, agent_rows, appearance, attachments, background, browser, commands, connect,
    context_window, copied, deep_links, demo, diagnostics, diagram, dictation, dirs, disclosure,
    egg, events, files, find, fork, fulltext, glitch, history, home_search, inbox_model, jobs,
    jump, keymap, knobs, language, launch, layout_state, line_comments, line_reveal, mcp,
    mentions, message_nav, model_prefs, motion_log, new_session, new_worktree, notifications,
    open_in, open_story, panels, permission, pickers, pinch, plugin_host, popup, project_switcher,
    projects, prompt_history, question, queue, quote, recent_page, reconnect, restore, review,
    rewind, rune_mcp, send_failed, served, servers, session_catalog, session_menu, session_pages,
    session_state, session_status, settings, shortcuts, slash, speech, status_toast, stop,
    stories, subagents, submit, suggestions, switcher, tabs, telemetry, terminal, text_rendering,
    tips, todos, tool_open, turn_diffs, undo, updater, usage, view, wall_ms, windows, working,
};

// A static, not a const: a const's bytes are copied into every codegen unit that names it.
pub(crate) static FIXTURE: &str = include_str!("../assets/fixture.json");

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabStatus {
    Busy,
    Attention,
    Unread,
    /// Unread, and its run failed.
    Failed,
}

#[derive(Default)]
pub enum Source {
    Live { server: Server, requests: Option<Sender<Request>>, writes: Option<Sender<Write>>, allow_send: bool },
    #[default]
    Fixture,
}

#[derive(Default)]
pub struct App {
    /// Decoupled shell kernel registries (lifecycle scheduler, typed feature slots, overlays,
    /// commands, deferred event bus, and native surface catalog).
    pub kernel: opencode_kernel::Kernel<App>,
    pub source: rune::Tracked<Source>,
    pub(crate) inbox: rune::Tracked<Option<Receiver<Incoming>>>,
    /// The live server events taken, for the motion log (`motion_log.rs`).
    pub(crate) motion_events: rune::Tracked<motion_log::Events>,
    pub(crate) pages: rune::Tracked<session_pages::State>,
    // Created by the first Save, never before Frame 1. A failed write wakes the window once.
    pub(crate) output_errors: Option<(Sender<String>, Receiver<String>)>,
    pub(crate) opening: rune::Tracked<Option<session_pages::Opening>>,
    pub(crate) open_demo: rune::Tracked<Option<open_story::Demo>>,
    /// The server's projects and sessions. This and the other server data (transcripts, running,
    /// stops, requests, pending, inboxes) are the server's copy, which every window of the server
    /// holds in turn (`windows::peer`).
    pub projects: rune::Tracked<Vec<Project>>,
    pub sessions: rune::Tracked<Vec<SessionInfo>>,
    pub recent_page: rune::Tracked<recent_page::Page>,
    pub recent_reload: recent_page::Reload,
    pub(crate) catalog: session_catalog::Catalog,
    pub(crate) session_search: switcher::search_index::SessionSearch,
    /// The palette's and Home's questions to the message text kept on this device.
    pub(crate) fulltext: fulltext::Surfaces,
    pub route: rune::Tracked<Route>,
    /// The open tab this window left for Home, independent of tab order or layout.
    pub(crate) home_return_tab: rune::Tracked<Option<String>>,
    /// Where ⌘[ and ⌘] go (`history.rs`).
    pub nav: rune::Tracked<history::History>,
    pub transcripts: rune::Tracked<HashMap<String, Transcript>>,
    pub(crate) transcript_edits: events::TranscriptEdits,
    /// The disclosures the reader toggled, tool groups springing shut among them (`disclosure`).
    pub expanded: rune::Tracked<disclosure::Disclosures>,
    /// Grow, then show's streaming words, held a line-grow behind (`line_reveal`).
    pub line_hold: rune::Tracked<line_reveal::Hold>,
    /// The copy button showing its tick (`copied.rs`).
    pub copied: rune::Tracked<copied::Copied>,
    /// The diagrams replies show, and the one open full size (`diagram`).
    pub diagram: rune::Tracked<diagram::Diagrams>,
    /// Pinned session tabs, in sidebar order (see `tabs.rs` for where they come from).
    pub tabs: rune::Tracked<Vec<Tab>>,
    /// The tab strip's gestures: a press, a drag to a new place, the width a close holds.
    pub tab_drag: rune::Tracked<rune_kit::shell::TabDragState>,
    /// The card a resting pointer shows over a tab (`view::tab_preview`).
    pub tab_preview: rune::Tracked<view::tab_preview::Preview>,
    /// Closed session tabs, newest last, with the index they had (⌘⇧T reopens; `ui.json` keeps them).
    pub closed: rune::Tracked<tabs::Closed>,
    /// When a titlebar tab last closed (ms), for ⇧⌘T (`reopensSideTab`).
    pub tab_closed_ms: rune::Tracked<Option<i64>>,
    /// The tab layout's switch and the collapsed sidebar's peek (`layout_state.rs`).
    pub layout: rune::Tracked<layout_state::Shell>,
    /// The Inbox layout's request times, branches and height (`inbox_model.rs`).
    pub sidebar_inbox: rune::Tracked<inbox_model::SidebarInbox>,
    /// UI zoom, 1.0 = 100% (see `view::app`).
    pub zoom: rune::Tracked<f32>,
    /// A pinch's zoom gesture (`pinch.rs`).
    pub zoom_pinch: rune::Tracked<pinch::Pinch>,
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
    /// Each session's queue changes this window asked for (`queue::machine`).
    pub queue_ops: rune::Tracked<HashMap<String, queue::machine::Queue>>,
    /// A queued prompt dragged to a new place (`queue::drag`).
    pub queue_drag: rune::Tracked<queue::drag::Drag>,
    /// The queue rewrites in flight and how far each got (`queue::run`).
    pub queue_runs: rune::Tracked<queue::run::Runs>,
    /// A send held while a session opens (`submit.rs`).
    pub submit: rune::Tracked<submit::Submit>,
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
    /// The width when a resize drag started.
    pub sidebar_drag: rune::Tracked<view::sidebar::Resize>,
    pub composer: rune::Tracked<TextEdit>,
    /// Sent prompts, for ↑ and ↓ in the composer (`prompt_history.rs`).
    pub prompts: rune::Tracked<prompt_history::PromptHistory>,
    /// The composer's images, and sent prompts waiting on the server's copy.
    pub attach: rune::Tracked<Box<attachments::Attach>>,
    /// The Quote button over a timeline selection, and the quotes waiting in the composer.
    pub quote: rune::Tracked<Box<quote::Quotes>>,
    /// Comments on lines of files, from Files and Changes (`line_comments`).
    pub line_comments: rune::Tracked<Box<line_comments::LineComments>>,
    pub find: rune::Tracked<Box<find::Find>>,
    pub message_nav: rune::Tracked<message_nav::MessageNav>,
    pub usage: rune::Tracked<usage::Usage>,
    pub search: rune::Tracked<TextEdit>,
    pub home_search: rune::Tracked<Box<home_search::HomeSearch>>,
    /// Session menus, the title being renamed and Delete's confirmation.
    pub menus: rune::Tracked<Box<session_menu::Menus>>,
    /// The drawn menu or picker open now (off macOS).
    pub popup: rune::Tracked<Box<popup::Popup>>,
    pub pickers: rune::Tracked<Box<pickers::Pickers>>,
    /// The composer's `@` popover and what it lists.
    pub mentions: rune::Tracked<Box<mentions::Mentions>>,
    /// Session ids named in replies, looked up once (`mentions::refs`).
    pub session_refs: rune::Tracked<mentions::refs::Lookups>,
    /// Live look-and-feel sliders (⌘⇧K).
    pub knobs: rune::Tracked<Box<rune_knobs::Knobs>>,
    /// The frame recorder and its scrubber, for animation glitches (⌘⇧⌥R).
    pub glitch: rune::Tracked<glitch::Glitch>,
    /// The new-session wordmark's Easter egg (`egg`): nothing until the pointer comes near its "e".
    /// Untracked: its frames step and draw without the view running; `egg::poll` says when the
    /// view must.
    pub egg: egg::Egg,
    /// The + menu, the `/` command popover and shell mode.
    pub add: rune::Tracked<add_menu::AddMenu>,
    /// The composer's suggestion lists and prompt recall (`suggestions/machine.rs`).
    pub suggest: rune::Tracked<suggestions::Suggest>,
    /// Hex dictation into the composer (⇧⌘M or the mic).
    pub dictation: rune::Tracked<dictation::Dictation>,
    pub switcher: rune::Tracked<Box<switcher::Switcher>>,
    pub project_switcher: rune::Tracked<project_switcher::ProjectSwitcher>,
    pub project_state: rune::Tracked<Box<projects::ProjectsState>>,
    /// Home across servers: its selection, its servers and its list's phase (`projects::home`).
    pub home_state: rune::Tracked<projects::home::Home>,
    /// A server's folder picker, while one is open (`projects::remote_picker`).
    pub remote_picker: rune::Tracked<projects::remote_picker::Picker>,
    pub settings: rune::Tracked<settings::Settings>,
    /// Settings' Providers, Models and Extensions pages (`settings::pages`).
    pub settings_pages: rune::Tracked<Box<settings::pages::State>>,
    /// The connect dialog (`connect`).
    pub connect: rune::Tracked<Box<connect::Connect>>,
    /// The integrations and providers it and Settings read, by folder.
    pub catalogs: rune::Tracked<connect::catalog::Catalogs>,
    /// The MCP servers' toggles and the MCPs dialog (`mcp`).
    pub mcp: rune::Tracked<Box<mcp::Mcp>>,
    /// Server requests on their way back (`jobs`).
    pub(crate) jobs: rune::Tracked<jobs::Jobs>,
    /// System notifications for finished runs, permission requests and errors.
    pub notify: rune::Tracked<notifications::Notifier>,
    /// What a screen reader hears of the agent's work (`speech.rs`).
    pub speech: speech::Speech,
    /// Links the system handed the app, until they open (`deep_links`).
    pub deep_links: rune::Tracked<deep_links::DeepLinks>,
    /// Every server's link: live, in doubt or down (`reconnect`).
    pub server_links: rune::Tracked<reconnect::Links>,
    /// Whether each open session can show, by (server, session) (`session_state`).
    pub session_states: rune::Tracked<session_state::States>,
    /// The open session and composer drafts, saved for the next launch.
    pub restore: rune::Tracked<Box<restore::Restore>>,
    /// Background shells, and the running-work list above the composer (`subagents::activity`).
    pub shells: rune::Tracked<subagents::shells::Shells>,
    /// Permission replies still on their way, to bring back if the server refuses one.
    pub permission_sent: rune::Tracked<permission::Sent>,
    /// The plan's list over the todo pill is open (`todos.rs`).
    pub todo_open: rune::Tracked<bool>,
    pub todo_ticks: rune::Tracked<todos::Ticks>,
    /// Subagents' latest steps and how their runs ended, for their timeline rows.
    pub agent_rows: rune::Tracked<agent_rows::Live>,
    pub demo: rune::Tracked<demo::Demo>,
    /// Whether AppKit is saving the window frame yet (`rune_mac::remember_frame`).
    pub(crate) framed: rune::Tracked<bool>,
    pub reduced_motion: rune::Tracked<bool>,
    /// Where `reduced_motion` comes from (`appearance.rs`).
    pub motion: rune::Tracked<appearance::Motion>,
    /// The mode Appearance › Text rendering draws with (`text_rendering.rs`).
    pub text_rendering: rune::Tracked<text_rendering::Rendering>,
    /// Where each session's timeline stands against its end (`jump.rs`).
    pub ends: rune::Tracked<jump::Ends>,
    /// The subagent call whose row is under the pointer, its chevron slid in (`subagent_card`).
    pub agent_hover: rune::Tracked<Option<String>>,
    /// When each run started, for the Working label's rhythm (`working.rs`).
    pub working: rune::Tracked<working::Working>,
    /// `--debug`: the status bar with the edge, the source and the last status.
    pub debug: rune::Tracked<bool>,
    pub status: rune::Tracked<String>,
    pub rewind: rune::Tracked<rewind::RewindState>,
    pub fork: rune::Tracked<Box<fork::ForkState>>,
    pub jump: rune::Tracked<message_nav::jump::JumpState>,
    pub toast: rune::Tracked<status_toast::StatusToast>,
    /// The paced one-time tips (`tips.rs`).
    pub tips: rune::Tracked<tips::Tips>,
    /// Prompts sent and not yet seen back, for rolling a failed one back (`send_failed.rs`).
    pub unsent: rune::Tracked<send_failed::Log>,
    /// Sessions shown before the server has them (`new_session.rs`).
    pub starts: rune::Tracked<new_session::Starts>,
    /// What the window's toasts offer back (`undo`).
    pub undos: rune::Tracked<undo::Undos>,
    /// Drafts that start in a new worktree, worktree lists and the branch picker (`new_worktree.rs`).
    pub new_worktree: rune::Tracked<Box<new_worktree::NewWorktree>>,
    /// Wall-clock milliseconds for relative times (fixture: its own fixed "now").
    pub now_ms: rune::Tracked<i64>,
    pub(crate) base_ms: rune::Tracked<i64>,
    pub(crate) clock: rune::Tracked<f64>,
    /// How many times the view has run; compared against `PolledRuns` in `kernel.slots` so a poll
    /// knows whether derived state (panel tabs, plugins' watched signals) may have moved.
    pub view_runs: std::cell::Cell<u64>,
    pub(crate) fixture: rune::Tracked<Value>,
    pub(crate) playback: rune::Tracked<VecDeque<(f64, String, Value)>>,
    /// What the fixture's server stored from the replies it streamed (`served.rs`).
    pub(crate) served: rune::Tracked<served::Served>,
    pub(crate) background: rune::Tracked<background::Background>,
    pub(crate) sends: rune::Tracked<u32>,
    pub(crate) waker: rune::Tracked<Option<Waker>>,
    pub(crate) launch: rune::Tracked<launch::Launch>,
    pub updater: rune::Tracked<updater::Updater>,
    pub icons: rune::Tracked<project_icon::Icons>,
    pub side: rune::Tracked<Box<panels::SideState>>,
    pub browser: rune::Tracked<Box<browser::BrowserPane>>,
    pub review: rune::Tracked<Box<review::ReviewPane>>,
    /// The file sidebar the Review and Files panels share (Desktop's `PanelSidebar`).
    pub panel_sidebar: rune::Tracked<plugin_host::native::sidebar::PanelSidebar>,
    pub files: rune::Tracked<Box<files::FilesPane>>,
    pub open_in: rune::Tracked<open_in::OpenIn>,
    /// What a tool step last opened in the side region (`tool_open`).
    pub tool_open: rune::Tracked<Option<tool_open::Opened>>,
    pub terminal: rune::Tracked<Box<terminal::TerminalPane>>,
    pub rune_mcp: rune::Tracked<rune_mcp::WindowMcp>,
    /// The plugins: their panels, commands, menus, settings and MCP tools (`plugin_host`).
    pub plugins: rune::Tracked<Box<plugin_host::PluginHost>>,
    /// The server the window works against now and its halves of the others (`servers::runtime`).
    pub checkout: rune::Tracked<servers::runtime::Checkout>,
    /// The session header's location card: each folder's branch once read.
    pub location: rune::Tracked<view::location::Location>,
    /// A storybook (⌘⇧D) story's stage, in the world the playground made for it.
    pub stage: rune::Tracked<Option<stories::Stage>>,
    /// The story's variant (1 to 9 in the playground pick it).
    pub variant: rune::Tracked<usize>,
    /// Which window this is, and what it keeps of itself (`windows`).
    pub window: rune::Tracked<windows::Window>,
    /// The frame diagnostics every window reports to.
    pub diagnostics: rune::Tracked<diagnostics::Diagnostics>,
    /// The locale this window's view last built in (`language`).
    pub language: rune::Tracked<language::Applied>,
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
            reduced_motion: flag("--reduced-motion").into(),
            motion: appearance::Motion::new(flag("--reduced-motion")).into(),
            text_rendering: Default::default(),
            debug: flag("--debug").into(),
            icons: desktop_icons().into(),
            ..Default::default()
        }
    }

    /// Back to Home, dropping a session that was still opening.
    pub(crate) fn go_home(&mut self, tx: &mut Tx) {
        if self.route != Route::Home {
            *self.home_return_tab = self.current_tab();
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

    /// A hot build's one window: the profile's first window, whose tabs and page it reads and
    /// saves as the app's own first window does.
    #[cfg(feature = "hot")]
    pub(crate) fn hot() -> App {
        Self::new_window(windows::first_saved())
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
        let v: Value = serde_json::from_str(src).expect("fixture JSON");
        self.pickers.load(&v);
        self.mentions.load(&v);
        self.add.slash.commands = slash::commands(&v["commands"]);
        *self.base_ms = v["now"].as_i64().unwrap_or(0);
        *self.now_ms = *self.base_ms;
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
        *self.status = "Fixture".into();
        *self.open_in = open_in::OpenIn::with(std::sync::Arc::new(open_in::Fixed(vec!["vscode", "cursor", "zed", "terminal", "ghostty", "xcode", "powershell"])));
        self.open_in.detect();
        if let Some(home) = v["home"].as_str() {
            *self.home = home.to_owned();
        }
        *self.fixture = v;
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
            self.launch.fallback_fixture = fallback_fixture;
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
            None if deferred => self.launch.set_phase(ServerKey::SIDECAR, launch::Phase::Waiting { beta_tabs: beta }),
            None => {}
        }
        if deferred && self.launch.phase(ServerKey::SIDECAR).is_none() {
            self.launch.set_phase(ServerKey::SIDECAR, launch::Phase::Waiting { beta_tabs: false });
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
            self.files.wrap = files::markdown::WrapPrefs::load(self);
            *self.panel_sidebar = plugin_host::native::sidebar::PanelSidebar::load();
        }
        *self.prompts = prompt_history::PromptHistory::load(attended());
        self.pickers.prefs = model_prefs::load();
        telemetry::startup("launch.restore_ui", || self.restore_ui(&doc, &server.base));
        *self.base_ms = wall_ms();
        *self.now_ms = *self.base_ms;
        let allow_send = !flag("--read-only") && attended();
        *self.source = Source::Live { server, requests: None, writes: None, allow_send };
        self.open_outbox();
        *self.status = self.live_status();
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
        self.zoom_pinch.leveled();
        if self.persists() {
            tabs::save_zoom(*self.zoom);
        }
    }

    /// OpenCode Desktop's key defaults; true when `ev` was one.
    pub fn shortcut(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        use shortcuts::Action;
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
        let Some(action) = shortcuts::action_tx(ev, tx) else { return false };
        match action {
            // Zoom lands at once; the scale and every re-laid-out row would otherwise spring.
            Action::ZoomIn => self.zoom_by(1, tx),
            Action::ZoomOut => self.zoom_by(-1, tx),
            Action::ZoomReset => self.zoom_by(0, tx),
            Action::Tab(offset) => self.select_adjacent(offset, tx),
            Action::UnreadTab(offset) => self.select_unread(offset, tx),
            // Desktop numbers the first nine it shows (`titlebar-tab-numbers`).
            Action::TabAt(i) => self.select_nth(i, tx),
            Action::NewSession => self.new_session(tx),
            Action::OpenProject => self.open_project_dialog(tx),
            Action::SelectProject => self.toggle_project_switcher(tx),
            Action::Settings => self.toggle_settings(tx),
            Action::AutoAccept => self.toggle_auto_accept(tx),
            Action::CloseTab => self.close_tab_key(tx),
            Action::ReopenTab => self.reopen_tab(tx),
        }
        true
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
            let _ = r.send(Request::Info(id.to_owned()));
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
        let review_gone: Vec<String> = s.review.sessions.keys().filter(|id| !kept(id)).cloned().collect();
        let files_gone: Vec<String> = s.files.sessions.keys().filter(|id| !kept(id)).cloned().collect();
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
        s.files.retain_reply_links(&kept);
        if !gone.is_empty() {
            let transcripts = &mut *s.transcripts;
            for id in &gone { transcripts.remove(id); }
        }
        // A closed session's diffs and open files (their source lines, highlight chunks, images).
        if !review_gone.is_empty() { for id in &review_gone { s.review.sessions.remove(id); } }
        if !files_gone.is_empty() {
            for id in &files_gone {
                s.files.sessions.remove(id);
                s.files.reads.forget_session(id);
                files::watch::forget_session(s, id);
            }
        }
        // What `opencode_kit` keeps beside each user message (files, mentions, comments, times):
        // only for the messages of the transcripts left.
        let cached = s.pages.user_message_ids();
        let messages: HashSet<&str> = s.transcripts.values().flat_map(|t| t.messages.iter()).filter_map(|m| match m { opencode_kit::model::Message::User { id, .. } => Some(id.as_str()), _ => None }).chain(cached.iter().map(String::as_str)).collect();
        opencode_kit::model::retain_user_messages(&|id| messages.contains(id));
        s.attach.store.rest();
        // No session tab or draft and not on a session: pictures keep their encoded bytes, not
        // their pixels.
        if s.tabs.is_empty() && !matches!(&*s.route, Route::Session { .. }) {
            s.attach.store.rest_pixels();
            // The built-ins a session started sleep again, as at launch (`plugin_host::dormant`).
            if matches!(&*s.route, Route::Home) { plugin_host::dormant::sleep_unused(s); }
        }
        if has_rest_hooks {
            scheduler.run_rest(s, &|id| kept_ids.contains(id));
        }
        plugin_host::memory::collect_garbage(s);
        #[cfg(feature = "alloc-sites")]
        plugin_host::memory::publish_vms(s);
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
        if self.sessions.len() >= 512 {
            return self.catalog.lookup(&self.sessions, id).map(|(i, _)| &self.sessions[i]);
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
        if self.stage.is_none() && auto_focus_composer(tx) {
            self.composer.focused = true;
        }
        tx.settle();
    }
}
