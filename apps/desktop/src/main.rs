//! OpenCode Desktop in Rune: OpenCode's home and session pages in Rune, live against the local OpenCode server or
//! from a synthetic fixture. Live, it sends prompts, answers permission requests and questions,
//! stops runs and starts sessions (`--read-only` turns all of that off).

/// Counts each thread's allocations for the frame diagnostics and the benchmarks, over mimalloc:
/// the system heap costs a Luau plugin's rebuild (its tables, strings and elements) about a fifth
/// more on Windows.
#[global_allocator]
static ALLOC: procwatch::alloc::Counting<mimalloc::MiMalloc> = procwatch::alloc::Counting::new(mimalloc::MiMalloc);

use crate::pickers::PickerHost as _;
pub use session_view::{
    agent_start, disclosure, fold, jump, line_reveal, odometer, palette, question_row,
    subagent_card as agent_rows_view, themes, turns,
};

mod compaction {
    pub use session_view::compaction::*;
}
mod copied {
    pub use session_view::copied::*;
}
mod diagram {
    pub use session_view::diagram::*;
    pub(crate) mod stories;
}
mod genui {
    pub use session_view::genui::*;
    pub(crate) mod demo;
    pub(crate) mod diagrams;
    pub(crate) mod skill;
}
mod turn_errors {
    pub use session_view::turn_errors::*;
}
mod working {
    pub use session_view::working::*;
}

mod add_menu;
mod add_selection;
mod agent_rows;
mod agent_start_stories;
mod agent_stories;
mod app_menu;
mod appearance;
mod text_rendering;
mod appearance_stories;
mod arrivals;
mod askpass;
mod attachments;
mod auto_accept;
mod glitch;
mod auto_accept_story;
mod background;
mod background_stories;
mod browser;
mod capsule_stories;
pub mod code_view;
mod commands;
mod commands_stories;
mod compaction_stories;
mod connect;
mod context_window;
mod deep_links;
mod demo;
mod demo_script;
mod diagnostics;
mod dictation;
mod dirs;
mod disk;
mod dock_fade_stories;
mod dropzone;
mod escape;
mod events;
mod export_logs;
mod files;
mod find;
mod find_stories;
mod fixture_ids;
mod fold_stories;
mod fork;
mod forms;
mod frosted_stories;
mod composer_grow_stories;
mod history;
mod history_story;
mod home_search;
mod icons;
mod inbox_model;
mod incoming;
#[cfg(windows)]
mod install;
mod jobs;
mod keymap;
mod keys;
mod knobs;
mod language;
mod launch;
mod layout_state;
mod line_comments;
mod mcp;
mod mention_stories;
mod mentions;
mod menu_stories;
mod menubar;
mod message_nav;
mod model_picker;
mod model_prefs;
mod motion_log;
mod narrow;
mod new_session;
mod new_worktree;
mod notifications;
mod odometer_stories;
mod title_change;
mod open_in;
mod open_story;
pub mod panels;
mod permission;
mod pickers;
mod pinch;
mod plugin_host;
mod popover;
mod popup;
mod project_stories;
mod project_switcher;
mod projects;
mod prompt_history;
mod prompt_history_story;
mod question;
mod question_stories;
mod queue;
mod quote;
#[path = "quote/cards.rs"]
mod quote_cards;
#[path = "quote/stories.rs"]
mod quote_stories;
mod recently_closed;
mod restore;
mod review;
mod route;
mod rewind;
mod rewind_stories;
#[path = "view/rhythm_stories.rs"]
mod rhythm_stories;
mod rune_mcp;
mod scenes;
mod scrollbar;
mod shell_fold;
mod short_group;
mod selection_stories;
mod send;
mod served;
mod servers;
mod session_menu;
mod session_menu_stories;
mod session_menu_view;
mod send_failed;
mod session_cost;
mod session_open;
mod session_pages;
mod session_state;
mod session_status;
mod settings;
mod shortcuts;
mod side_stories;
mod slash;
mod sounds;
mod reconnect;
mod status_toast;
mod stop;
mod stop_stories;
mod stories;
#[path = "stream/pace.rs"]
mod stream_pace;
#[path = "stream/line_grow_story.rs"]
mod line_grow_story;
mod subagents;
mod submit;
mod suggestions;
mod switcher;
mod tab_actions;
mod tab_badges;
mod tabs;
mod telemetry;
mod terminal;
mod todos;
mod todo_stories;
mod tool_group_stories;
mod tool_open;
mod tool_open_stories;
mod turn_diffs;
mod turn_errors_stories;
#[path = "commands/type_to_compose.rs"]
mod type_to_compose;
#[cfg(feature = "office")]
mod office;
mod updater;
mod usage;
mod view;
mod web_fetch;
mod widgets;
mod windows;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender};

use rune::{AppSpec, Key, KeyEvent, TextEdit, Tx, Waker};
use serde_json::Value;

use opencode_kit::client::{DEFAULT_SERVER, Incoming, Prompt, Request, Server, Write};
use opencode_kit::servers::ServerKey;
use opencode_kit::model::{Delivery, Inbox, Message, Pending, Project, SessionInfo, Transcript};
use keys::on_key;
pub(crate) use scenes::open_scene;
use tabs::Tab;

const FIXTURE: &str = include_str!("../assets/fixture.json");

pub use route::Route;

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
    pub source: Source,
    inbox: Option<Receiver<Incoming>>,
    /// The live server events taken, for the motion log (`motion_log.rs`).
    motion_events: motion_log::Events,
    pages: session_pages::State,
    opening: Option<session_pages::Opening>,
    open_demo: Option<open_story::Demo>,
    /// The server's projects and sessions. This and the other server data (transcripts, running,
    /// stops, requests, pending, inboxes) are the server's copy, which every window of the server
    /// holds in turn (`windows::peer`).
    pub projects: Vec<Project>,
    pub sessions: Vec<SessionInfo>,
    pub route: Route,
    /// Where ⌘[ and ⌘] go (`history.rs`).
    pub nav: history::History,
    pub transcripts: HashMap<String, Transcript>,
    /// The disclosures the reader toggled, tool groups springing shut among them (`disclosure`).
    pub expanded: disclosure::Disclosures,
    /// Grow, then show's streaming words, held a line-grow behind (`line_reveal`).
    pub line_hold: line_reveal::Hold,
    /// The copy button showing its tick (`copied.rs`).
    pub copied: copied::Copied,
    /// The diagrams replies show, and the one open full size (`diagram`).
    pub diagram: diagram::Diagrams,
    /// Pinned session tabs, in sidebar order (see `tabs.rs` for where they come from).
    pub tabs: Vec<Tab>,
    /// The tab strip's gestures: a press, a drag to a new place, the width a close holds.
    pub tab_drag: rune_kit::shell::TabDragState,
    /// The card a resting pointer shows over a tab (`view::tab_preview`).
    pub tab_preview: view::tab_preview::Preview,
    /// Closed session tabs, newest last, with the index they had (⌘⇧T reopens; `ui.json` keeps them).
    pub closed: tabs::Closed,
    /// The tab layout's switch and the collapsed sidebar's peek (`layout_state.rs`).
    pub layout: layout_state::Shell,
    /// The Inbox layout's request times, branches and height (`inbox_model.rs`).
    pub sidebar_inbox: inbox_model::SidebarInbox,
    /// UI zoom, 1.0 = 100% (see `view::app`).
    pub zoom: f32,
    /// A pinch's zoom gesture (`pinch.rs`).
    pub zoom_pinch: pinch::Pinch,
    /// Changed files' diffs (`turn_diffs.rs`).
    pub turn_diffs: turn_diffs::TurnDiffs,
    /// Which of each session's messages Desktop would hold, for the Context tab (`context_window`).
    pub context_windows: context_window::Windows,
    /// Sessions running right now (any depth; a tab is busy when anything in its family runs).
    pub running: HashSet<String>,
    /// Runs stopped on a press, until the server confirms (`stop.rs`).
    pub stops: stop::Stops,
    /// Pending permission and form requests: (session, request id).
    pub requests: HashSet<(String, String)>,
    /// Which sessions' requests this connection has read (`session_status.rs`).
    pub request_reads: session_status::RequestReads,
    /// The open permission requests and forms we know the details of.
    pub pending: Vec<Pending>,
    /// Prompts sent while a session was busy that the server hasn't delivered yet, by session.
    pub inboxes: HashMap<String, Inbox>,
    /// Each session's queue changes this window asked for (`queue::machine`).
    pub queue_ops: HashMap<String, queue::machine::Queue>,
    /// A queued prompt dragged to a new place (`queue::drag`).
    pub queue_drag: queue::drag::Drag,
    /// The queue rewrites in flight and how far each got (`queue::run`).
    pub queue_runs: queue::run::Runs,
    /// A send held while a session opens (`submit.rs`).
    pub submit: submit::Submit,
    /// This window's place in its server's peer (`windows::peer`).
    pub store: windows::peer::Link,
    /// The question dock's place and in-progress answers for the forms it shows.
    pub question: question::Dock,
    /// Where a new draft starts, and where paths resolve off a session: the folder last picked.
    /// The open draft's own folder is `App::draft_dir`.
    pub default_dir: String,
    /// The home folder paths show as `~`: the user's, or the fixture's in a fixture run.
    pub home: String,
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
    /// Comments on lines of files, from Files and Changes (`line_comments`).
    pub line_comments: line_comments::LineComments,
    pub find: find::Find,
    pub message_nav: message_nav::MessageNav,
    pub usage: usage::Usage,
    pub search: TextEdit,
    pub home_search: home_search::HomeSearch,
    /// Session menus, the title being renamed and Delete's confirmation.
    pub menus: session_menu::Menus,
    /// The drawn menu or picker open now (off macOS).
    pub popup: popup::Popup,
    pub pickers: pickers::Pickers,
    /// The composer's `@` popover and what it lists.
    pub mentions: mentions::Mentions,
    /// Session ids named in replies, looked up once (`mentions::refs`).
    pub session_refs: mentions::refs::Lookups,
    /// Live look-and-feel sliders (⌘⇧K).
    pub knobs: rune_knobs::Knobs,
    /// The frame recorder and its scrubber, for animation glitches (⌘⇧⌥R).
    pub glitch: glitch::Glitch,
    /// The + menu, the `/` command popover and shell mode.
    pub add: add_menu::AddMenu,
    /// The composer's suggestion lists and prompt recall (`suggestions/machine.rs`).
    pub suggest: suggestions::Suggest,
    /// Hex dictation into the composer (⇧⌘M or the mic).
    pub dictation: dictation::Dictation,
    pub switcher: switcher::Switcher,
    pub project_switcher: project_switcher::ProjectSwitcher,
    pub project_state: projects::ProjectsState,
    /// Home across servers: its selection, its servers and its list's phase (`projects::home`).
    pub home_state: projects::home::Home,
    /// A server's folder picker, while one is open (`projects::remote_picker`).
    pub remote_picker: projects::remote_picker::Picker,
    pub settings: settings::Settings,
    /// Settings' Providers, Models and Extensions pages (`settings::pages`).
    pub settings_pages: settings::pages::State,
    /// The connect dialog (`connect`).
    pub connect: connect::Connect,
    /// The integrations and providers it and Settings read, by folder.
    pub catalogs: connect::catalog::Catalogs,
    /// The MCP servers' toggles and the MCPs dialog (`mcp`).
    pub mcp: mcp::Mcp,
    /// Server requests on their way back (`jobs`).
    jobs: jobs::Jobs,
    /// System notifications for finished runs, permission requests and errors.
    pub notify: notifications::Notifier,
    /// Links the system handed the app, until they open (`deep_links`).
    pub deep_links: deep_links::DeepLinks,
    /// Every server's link: live, in doubt or down (`reconnect`).
    pub server_links: reconnect::Links,
    /// Whether each open session can show, by (server, session) (`session_state`).
    pub session_states: session_state::States,
    /// The open session and composer drafts, saved for the next launch.
    pub restore: restore::Restore,
    /// Background shells, and the running-work list above the composer (`subagents::activity`).
    pub shells: subagents::shells::Shells,
    /// Permission replies still on their way, to bring back if the server refuses one.
    pub permission_sent: permission::Sent,
    /// The plan's list over the todo pill is open (`todos.rs`).
    pub todo_open: bool,
    pub todo_ticks: todos::Ticks,
    /// Subagents' latest steps and how their runs ended, for their timeline rows.
    pub agent_rows: agent_rows::Live,
    pub demo: demo::Demo,
    /// Whether AppKit is saving the window frame yet (`rune_mac::remember_frame`).
    framed: bool,
    pub reduced_motion: bool,
    /// Where `reduced_motion` comes from (`appearance.rs`).
    pub motion: appearance::Motion,
    /// The mode Appearance › Text rendering draws with (`text_rendering.rs`).
    pub text_rendering: text_rendering::Rendering,
    /// The open session's prompts from another client growing in (`arrivals`).
    pub glows: session_view::arrivals::Arrivals,
    /// Where each session's timeline stands against its end (`jump.rs`).
    pub ends: jump::Ends,
    /// When each run started, for the Working label's rhythm (`working.rs`).
    pub working: working::Working,
    /// Background compaction by directory and how each compaction enters (`compaction.rs`).
    pub compactions: compaction::Compactions,
    /// `--debug`: the status bar with the edge, the source and the last status.
    pub debug: bool,
    pub status: String,
    pub rewind: rewind::RewindState,
    pub fork: fork::ForkState,
    pub toast: status_toast::StatusToast,
    /// Prompts sent and not yet seen back, for rolling a failed one back (`send_failed.rs`).
    pub unsent: send_failed::Log,
    /// Sessions shown before the server has them (`new_session.rs`).
    pub starts: new_session::Starts,
    /// Drafts that start in a new worktree, worktree lists and the branch picker (`new_worktree.rs`).
    pub new_worktree: new_worktree::NewWorktree,
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
    pub updater: updater::Updater,
    pub icons: project_icon::Icons,
    pub side: panels::SideState,
    pub browser: browser::BrowserPane,
    pub review: review::ReviewPane,
    /// The file sidebar the Review and Files panels share (Desktop's `PanelSidebar`).
    pub panel_sidebar: plugin_host::luau::native::sidebar::PanelSidebar,
    pub files: files::FilesPane,
    pub open_in: open_in::OpenIn,
    /// What a tool step last opened in the side region (`tool_open`).
    pub tool_open: Option<tool_open::Opened>,
    pub terminal: terminal::TerminalPane,
    pub rune_mcp: rune_mcp::WindowMcp,
    /// Native plugins over the C ABI: their panels, commands and settings rows (`plugin_host`).
    pub plugins: plugin_host::PluginHost,
    /// The server the window works against now and its halves of the others (`servers::runtime`).
    pub checkout: servers::runtime::Checkout,
    /// The session header's location card: each folder's branch once read.
    pub location: view::location::Location,
    /// A storybook (⌘⇧D) story's stage, in the world the playground made for it.
    pub stage: Option<stories::Stage>,
    /// The story's variant (1 to 9 in the playground pick it).
    pub variant: usize,
    /// Which window this is, and what it keeps of itself (`windows`).
    pub window: windows::Window,
    /// The frame diagnostics every window reports to.
    pub diagnostics: diagnostics::Diagnostics,
    /// The locale this window's view last built in (`language`).
    pub language: language::Applied,
}

fn arg(name: &str) -> Option<String> {
    let a = rune::extra_args();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
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
}

impl session_view::TimelineHost for App {
    fn knobs(&self) -> &rune_knobs::Knobs {
        &self.knobs
    }
    fn clock(&self) -> f64 {
        self.clock
    }
    fn now_ms(&self) -> i64 {
        self.now_ms
    }
    fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }
    fn zoom(&self) -> f32 {
        self.zoom
    }
    fn timeline_detail(&self) -> session_view::detail::Detail {
        self.settings.timeline_detail
    }
    fn code_block_lang(&self, language: &str) -> bool {
        plugin_host::luau::surfaces::code_block_lang(self, language)
    }
    fn code_block(&self, p: &palette::Pal, language: &str, key: &str, code: &str, streaming: bool, version: u64, native: rune::El) -> Option<rune::El> {
        plugin_host::luau::surfaces::code_block(self, p, language, key, code, streaming, version, native)
    }
    fn shell_output(&self, shell: &str) -> Option<&str> {
        self.shells.follow.output(shell)
    }
    fn shell_output_version(&self) -> u64 {
        self.shells.follow.version()
    }
    fn turn_diffs(&self) -> bool {
        self.settings.turn_diffs
    }
    fn turn_diff(&self, session: &str, from: &str) -> session_view::turn_diff::State<'_> {
        self.turn_diffs.view(session, from)
    }
    fn retry_turn_diff(&mut self, session: &str, from: &str) {
        self.turn_diffs.retry(session, from);
    }
    fn lane_align(&self) -> session_view::jump::Align {
        match self.settings.background_activity {
            settings::BackgroundActivity::Center => session_view::jump::Align::Center,
            settings::BackgroundActivity::Right => session_view::jump::Align::End,
            settings::BackgroundActivity::Left | settings::BackgroundActivity::Off => session_view::jump::Align::Start,
        }
    }
    fn content_width(&self) -> f32 {
        narrow::content_width(self).unwrap_or_else(|| self.settings.timeline_width.width().content())
    }
    fn timeline_frame(&self) -> Option<&'static str> {
        Some("session")
    }
    fn active_session_id(&self) -> Option<&str> {
        match &self.route {
            Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }
    fn transcript(&self, session: &str) -> Option<&Transcript> {
        self.transcripts.get(session)
    }
    fn session_directory(&self, session: &str) -> Option<&str> {
        self.session(session).map(|i| i.directory.as_str())
    }
    fn session_title(&self, session: &str) -> Option<String> {
        self.mention_title(session)
    }
    fn is_session_live(&self, session: &str) -> bool {
        self.running.contains(session)
            || (self.transcripts.get(session).is_some_and(|t| self.timeline_busy(t)) && !self.stops.has(session))
    }
    fn timeline_busy(&self,t:&Transcript)->bool {
        let last=t.messages.iter().rev().find(|m| !matches!(m,Message::Notice(_)) && !matches!(self.unsent.state(m.id()),Some(send_failed::State::Waiting|send_failed::State::Unconfirmed|send_failed::State::Failed)));
        matches!(last,Some(Message::User{..})) || matches!(last,Some(Message::Assistant(a)) if a.completed.is_none() && a.error.is_none())
    }
    fn working_label_for(&self, session: &str) -> Option<&'static str> {
        self.starts.status(session)
    }
    fn is_session_stopped(&self, session: &str) -> bool {
        self.stops.has(session)
    }
    fn session_asking(&self, session: &str) -> bool {
        self.pending.iter().any(|p| p.session() == session)
    }
    fn session_revert(&self, session: &str) -> Option<&str> {
        self.session(session).and_then(|i| i.revert.as_deref())
    }
    fn for_each_tracked_session(&self, mut f: impl FnMut(&str)) {
        for id in self.transcripts.keys().chain(self.running.iter()) {
            f(id);
        }
    }
    fn expanded(&self) -> &disclosure::Disclosures {
        &self.expanded
    }
    fn expanded_mut(&mut self) -> &mut disclosure::Disclosures {
        &mut self.expanded
    }
    fn copied_state(&self) -> &copied::Copied {
        &self.copied
    }
    fn copied_state_mut(&mut self) -> &mut copied::Copied {
        &mut self.copied
    }
    fn working_state(&self) -> &working::Working {
        &self.working
    }
    fn working_state_mut(&mut self) -> &mut working::Working {
        &mut self.working
    }
    fn compactions_state(&self) -> &compaction::Compactions {
        &self.compactions
    }
    fn compactions_state_mut(&mut self) -> &mut compaction::Compactions {
        &mut self.compactions
    }
    fn diagrams(&self) -> &diagram::Diagrams {
        &self.diagram
    }
    fn diagrams_mut(&mut self) -> &mut diagram::Diagrams {
        &mut self.diagram
    }
    fn media_store(&self) -> &session_view::images::MediaStore {
        &self.attach.store
    }
    fn media_store_mut(&mut self) -> &mut session_view::images::MediaStore {
        &mut self.attach.store
    }
    fn line_hold(&self) -> &line_reveal::Hold {
        &self.line_hold
    }
    fn line_hold_mut(&mut self) -> &mut line_reveal::Hold {
        &mut self.line_hold
    }
    fn ends(&self) -> &jump::Ends {
        &self.ends
    }
    fn ends_mut(&mut self) -> &mut jump::Ends {
        &mut self.ends
    }
    fn fold_user_messages(&self) -> bool {
        self.settings.fold_user
    }
    fn open_link(&mut self, url: &str, tx: &mut Tx) {
        self.open_reply_link(url, tx);
    }
    fn context_menu(&mut self, at: session_view::images::menu::MenuAt, press: rune::PointerPress, tx: &mut Tx) {
        popup::context::open(self, at, press, tx);
    }
    fn reply_link_help(url: &str) -> Option<String> {
        crate::files::link_tip::tip(url, "")
    }
    fn code_block_actions(p: &palette::Pal, language: Option<&str>, source: &str) -> Vec<rune::El> {
        plugin_host::code_block_actions(p, language, source)
    }
    fn send_request(&self, req: Request) {
        if let Source::Live { requests: Some(r), .. } = &self.source {
            let _ = r.send(req);
        }
    }
    fn can_switch_model(&self) -> bool {
        true
    }
    fn switch_model(&mut self, _tx: &mut Tx) {
        if self.pickers.open != Some(pickers::Kind::Model) {
            self.open_picker(pickers::Kind::Model);
        }
    }
    fn model_name(&self, model: &opencode_kit::model::ModelRef) -> String {
        self.pickers.model_name(model)
    }
    fn tool_row_action(&self, p: &palette::Pal, session: &str, call: &session_view::tools::Call) -> Option<rune::El> {
        tool_open::action(self, p, session, call)
    }
    fn tool_row_action_version(&self, id: &str) -> u64 {
        u64::from(self.tool_step_showing(id))
    }
    fn can_open_subagent(&self) -> bool {
        true
    }
    fn resolve_subagent<'a>(&'a self, parent: &str, session: Option<&'a str>, title: &str) -> Option<&'a str> {
        subagents::child_of(self, parent, session, title)
    }
    fn subagent_run(&self, parent: &str, call: &str, child: Option<&str>, status: &opencode_kit::model::ToolStatus) -> agent_rows::Run {
        self.agent_run(parent, call, child, status)
    }
    fn prefetch_subagent(&self, child: &str) {
        self.prefetch_session(child);
    }
    fn open_subagent(&mut self, parent: &str, child: &str, title: &str, agent: &str, tx: &mut Tx) {
        self.open_child(parent, child, title, agent, tx);
    }
    fn can_open_session(&self) -> bool {
        true
    }
    fn open_session(&mut self, id: &str, tx: &mut Tx) {
        self.open_mentioned(id, tx);
    }
    fn session_ref(&self, id: &str) -> Option<session_view::session_chips::SessionRef> {
        self.session_ref_of(id)
    }
    fn session_chip_menu(&mut self, id: &str, press: rune::PointerPress, tx: &mut Tx) {
        self.open_session_chip_menu(id, press, tx);
    }
    fn loading_error(&self, session: &str) -> Option<&str> {
        self.opening.as_ref().filter(|o| o.id == session).and_then(|o| o.error())
    }
    fn loading_older(&self, session: &str) -> bool {
        self.pages.loading_older(session)
    }
    fn more_above(&self, session: &str) -> bool {
        self.pages.more_above(session)
    }
    fn watch_scroll_top(&self, session: &str) -> Option<Box<dyn Fn(rune::ScrollPos)>> {
        let (pages, nav) = (self.pages.watch_top(session), self.message_nav.watch());
        Some(Box::new(move |at| {
            pages(at);
            nav(at);
        }))
    }
    fn on_timeline_selection_change(s: &mut Self, change: &rune::SelectionChange, tx: &mut Tx) {
        quote::selection_changed(s, change, tx);
    }
    fn find_row_version(&self, row_key: &str) -> u64 {
        self.find.row_version(row_key)
    }
    fn find_text_highlights(&self, p: &palette::Pal, row_key: &str) -> Vec<rune::TextHighlight> {
        self.find.text_highlights(p, row_key)
    }
    fn find_markdown_highlights(&self, p: &palette::Pal, row_key: &str) -> Vec<(&[std::ops::Range<usize>], rune_kit::Highlight)> {
        self.find.markdown_highlights(p, row_key)
    }
    fn find_row_hits(&self, row_key: &str) -> Option<(&[Vec<std::ops::Range<usize>>], Option<usize>)> {
        self.find.row_hits(row_key)
    }
    fn user_glow_at(&self, id: &str) -> Option<f64> {
        self.glows.glow_at(id)
    }
    fn user_menu_open_on(&self, id: &str) -> bool {
        self.rewind.menu_open_on(id)
    }
    fn quote_more_open(&self, id: &str) -> bool {
        self.quote.more.contains(id)
    }
    fn open_quote_more(&mut self, id: String) {
        self.quote.more.insert(id);
    }
    fn user_menu(&self, p: &palette::Pal, session: &str, id: &str, _body: &str, pending: bool) -> Option<rune::El> {
        rewind::user_menu(self, p, session, id, pending)
    }
    fn user_status(&self, p: &palette::Pal, id: &str) -> Option<rune::El> {
        send_failed::footer(self, p, id)
    }
    fn user_status_version(&self, id: &str) -> u64 {
        if let Some(s) = self.unsent.durable(id) {
            use std::hash::{DefaultHasher, Hash, Hasher};
            let mut version = DefaultHasher::new();
            (s.state(), s.then(), s.why()).hash(&mut version);
            return version.finish() | 8;
        }
        match self.unsent.state(id) {
            Some(send_failed::State::Unconfirmed) => 1,
            Some(send_failed::State::Failed) => if self.composer.text.is_empty() && !self.attach.any() { 2 } else { 3 },
            _ => 0,
        }
    }
    fn inbox_for(&self, session: &str) -> Option<&Inbox> {
        App::inbox_for(self, session)
    }
    fn queued_compaction(&self, session: &str) -> Option<&str> {
        self.inboxes.get(session).and_then(|i| i.compaction.as_deref())
    }
    fn pending_steer_label(&self, item: &opencode_kit::model::InboxItem) -> String {
        queue::pending_label(item)
    }
    fn turn_tail_extra(&self, p: &palette::Pal, session: &str, busy: bool) -> Option<rune::El> {
        background::hint(self, p, session, busy)
    }
}

fn flag(name: &str) -> bool {
    rune::extra_args().iter().any(|x| x == name)
}

/// A run that renders without a person at it (snapshots, trees, scripts, videos). It can read a
/// live server but never sends to it or writes the saved tabs, zoom, session and drafts, so a
/// test can't post or type as Kit.
pub(crate) use rune::is_headless as headless;

/// `--harness-writes`, honoured only when `RUNE_DIRS_ROOT` moves every folder this app writes:
/// a headless run (the parity harness's `--realtime` probes) sends, stops and saves as a window
/// does, into its own root and to the server it was given.
fn harness_writes() -> bool {
    flag("--harness-writes") && dirs::rooted()
}

/// A person, or the harness standing in for one, is at this run: it may write.
fn attended() -> bool {
    !headless() || harness_writes()
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl App {
    fn empty() -> App {
        let home = dirs::home();
        App {
            default_dir: arg("--directory").unwrap_or_else(|| home.clone()),
            home,
            sidebar_open: !flag("--sidebar-closed"),
            sidebar_w: view::SIDEBAR_W,
            zoom: arg("--zoom").and_then(|z| z.parse::<f32>().ok()).map_or(1.0, |z| z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX)),
            composer: TextEdit::new(""),
            search: TextEdit::new(""),
            reduced_motion: flag("--reduced-motion"),
            motion: appearance::Motion::new(flag("--reduced-motion")),
            text_rendering: Default::default(),
            debug: flag("--debug"),
            icons: desktop_icons(),
            ..Default::default()
        }
    }

    /// Back to Home, dropping a session that was still opening.
    pub(crate) fn go_home(&mut self, tx: &mut Tx) {
        self.close_settings();
        self.opening = None;
        self.route = Route::Home;
        tx.settle();
    }

    fn load(window: windows::Window) -> App {
        let mut app = Self::empty();
        app.window = window;
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
            app.zoom = z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX);
        }
        app
    }

    fn new() -> App {
        Self::new_window(windows::Window::unsaved())
    }

    /// A hot build's one window: the profile's first window, whose tabs and page it reads and
    /// saves as the app's own first window does.
    #[cfg(feature = "hot")]
    fn hot() -> App {
        Self::new_window(windows::hot())
    }

    /// `new`, for window `window`: its own tabs and page.
    fn new_window(window: windows::Window) -> App {
        rune::marks::mark("state.load");
        let mut app = telemetry::startup("launch.load", || Self::load(window));
        appearance::apply_args(&mut app);
        if let Some(layout) = arg("--tab-layout") {
            app.settings.tab_layout = layout_state::TabLayout::parse(Some(&layout));
            app.layout = layout_state::Shell::at_rest(app.settings.tab_layout);
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
        rune::marks::mark("state.restored");
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
        self.tabs = ids.iter().map(|id| Tab::session(id.clone(), "", "")).collect();
        self.running = v["running"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        for r in v["requests"].as_array().into_iter().flatten() {
            if let (Some(s), Some(id)) = (r["sessionID"].as_str(), r["id"].as_str()) {
                self.requests.insert((s.to_owned(), id.to_owned()));
            }
        }
        self.status = "Fixture".into();
        self.open_in = open_in::OpenIn::with(std::sync::Arc::new(open_in::Fixed(vec!["vscode", "cursor", "zed", "terminal", "ghostty", "xcode", "powershell"])));
        self.open_in.detect();
        if let Some(home) = v["home"].as_str() {
            self.home = home.to_owned();
        }
        self.fixture = v;
        self.source = Source::Fixture;
        self.init_used_projects(None);
        self.sort_projects();
        if arg("--directory").is_none()
            && let Some(p) = self.projects.first()
        {
            self.default_dir = p.directory.clone();
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
        let doc = telemetry::startup("launch.window_doc", || restore::read_window(self, &server));
        let saved_tabs = doc.tabs.as_ref().map(|list| list.iter().map(|t| Tab::from_doc(t, doc.info(&t.key()))).collect());
        // Desktop Beta's tabs come into the profile's first window only.
        let beta = self.window.legacy && server.base == DEFAULT_SERVER;
        match saved_tabs.or_else(|| (beta && !deferred).then(tabs::from_beta).flatten()) {
            Some(tabs) => self.tabs = tabs,
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
            self.zoom = z.clamp(shortcuts::ZOOM_MIN, shortcuts::ZOOM_MAX);
        }
        self.settings = settings::load();
        self.layout = layout_state::Shell::at_rest(self.settings.tab_layout);
        self.load_open_in();
        if self.persists() {
            self.files.wrap = files::markdown::WrapPrefs::load();
            self.panel_sidebar = plugin_host::luau::native::sidebar::PanelSidebar::load();
        }
        self.prompts = prompt_history::PromptHistory::load();
        self.pickers.prefs = model_prefs::load();
        telemetry::startup("launch.restore_ui", || self.restore_ui(&doc, &server.base));
        self.base_ms = wall_ms();
        self.now_ms = self.base_ms;
        let allow_send = !flag("--read-only") && attended();
        self.source = Source::Live { server, requests: None, writes: None, allow_send };
        self.open_outbox();
        self.status = self.live_status();
        self.resume_settings_page();
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
        self.zoom_pinch.leveled();
        if self.persists() {
            tabs::save_zoom(self.zoom);
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
        matches!(self.source, Source::Live { .. })
    }

    /// Saves tabs, zoom, the open session and drafts: live, with a person at it.
    pub fn persists(&self) -> bool {
        self.live() && attended()
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
    pub(crate) fn missing_ancestor(&self, id: &str) -> Option<String> {
        self.ancestors(id).take(32).last().filter(|cur| self.session(cur).is_none()).map(str::to_owned)
    }

    pub fn session(&self, id: &str) -> Option<&SessionInfo> {
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
        matches!(&self.route, Route::Session { id, .. } if self.busy(id))
    }

    /// `+`, ⌘T and New session: a new "New session" draft tab, in the open session's or draft's
    /// folder, as Desktop's `openDraft`. On Home and in Settings it's Home's project, else the
    /// last used or the first the server lists, and with no project it does nothing.
    pub fn new_session(&mut self, tx: &mut Tx) {
        let dir = if self.settings.open || self.route == Route::Home {
            let project = self
                .home_selected()
                .or_else(|| self.used_projects().into_iter().next())
                .or_else(|| self.projects.first());
            let Some(project) = project else { return };
            Some(project.directory.clone())
        } else {
            match &self.route {
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
            self.default_dir = dir;
        }
        self.opening = None;
        self.open_draft_tab();
        tx.settle();
    }

}

fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    s.clock = now;
    s.sync_history();
    s.now_ms = if s.live() { wall_ms() } else { s.base_ms + (now * 1000.0) as i64 };
    demo::poll(s);
    // The morph loop is for watching: the composer it lands on doesn't take the keys.
    if s.stage.is_some_and(|st| st.watch()) {
        s.composer.focused = false;
    }
    // Another window changed the session data this one shows.
    let mut changed = windows::peer::behind(s);
    // The servers the window isn't checked out on: their launches, replies and streams.
    changed |= servers::runtime::poll(s, tx);
    changed |= servers::health::poll(s, tx);
    changed |= projects::home::poll(s, tx);
    changed |= project_switcher::poll(s);
    changed |= dictation::poll(s, now, tx);
    changed |= launch::poll(s, tx);
    launch::trace::poll(tx);
    changed |= updater::poll(s, tx);
    #[cfg(feature = "office")]
    {
        changed |= office::poll(s, tx);
    }
    changed |= glitch::poll(s, tx);
    changed |= appearance::poll(s);
    text_rendering::poll(s, tx);
    changed |= language::poll(s, tx);
    changed |= settings::search::poll(s, tx);
    settings::pages::servers::unavailable::poll(s, tx);
    changed |= notifications::poll(s, tx);
    changed |= deep_links::drain(s, tx);
    changed |= quote::poll(s, now, tx);
    changed |= line_comments::poll(s, tx);
    changed |= message_nav::poll(s, tx);
    changed |= turn_errors::poll(s, tx);
    changed |= session_view::turns::poll(s, tx);
    changed |= dropzone::poll(s, tx);
    changed |= s.poll_pages(tx);
    changed |= s.poll_context_window();
    changed |= s.poll_sends();
    changed |= s.tick_outbox(tx);
    changed |= s.poll_open_in();
    changed |= s.poll_jobs(tx);
    let mut arrivals = None;
    let asked = s.dock_face();
    changed |= incoming::poll(s, tx, &mut arrivals);
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
            if ty.starts_with("demo.") {
                demo::apply(s, &ty, &data, tx);
                changed = true;
                continue;
            }
            if ty == "fixture.page" {
                s.show_fixture_page(data["sessionID"].as_str().unwrap_or(""), tx);
                changed = true;
                continue;
            }
            s.apply_event(&ty, &data);
            if let Some(sid) = data["sessionID"].as_str().filter(|sid| ty == "session.step.started" && s.route == Route::here(sid.to_string())) {
                s.refresh_fixture_page(sid);
            }
            changed = true;
        }
    }
    if let Some(arrivals) = arrivals {
        arrivals.land(s, now, tx);
    }
    changed |= stop::poll(s, now, tx);
    changed |= s.revert_timeouts(tx);
    changed |= fork::poll(s, tx);
    s.queue_timers(tx);
    s.start_timers(tx);
    changed |= submit::poll(s, tx);
    changed |= working::sync(s);
    changed |= compaction::sync(s);
    changed |= subagents::shells::poll(s, tx) | todos::sync(s, now, tx) | settings::pages::worktrees::poll(s, tx);
    if s.dock_face() != asked {
        s.morph(tx);
    }
    changed |= s.glows.poll(&s.knobs, now, tx);
    changed |= disclosure::poll(s, now, tx);
    changed |= line_reveal::poll(s, now, tx);
    changed |= copied::poll(s, now, tx);
    changed |= diagram::poll(s, now, tx);
    changed |= session_view::images::poll(s, now, tx);
    changed |= model_picker::hover::poll(s, now, tx);
    changed |= agent_rows::poll(s, tx);
    changed |= find::poll(s);
    changed |= s.poll_home_search(tx);
    changed |= s.poll_switcher(tx);
    changed |= s.poll_mentions(tx);
    changed |= s.poll_commands(tx);
    changed |= s.poll_suggestions(tx);
    changed |= s.poll_rename(now, tx);
    changed |= s.poll_branch_picker(now, tx);
    changed |= s.poll_delete(now, tx);
    changed |= s.poll_uploads(tx);
    changed |= s.attach.poll();
    changed |= s.poll_panels(tx);
    changed |= rune_mcp::poll(s, tx);
    changed |= s.poll_turn_diffs();
    changed |= settings::pages::project::poll(s, tx);
    changed |= settings::pages::about::poll(s, now, tx);
    changed |= settings::pages::projects::poll(s);
    changed |= reconnect::poll(s, tx);
    changed |= session_state::poll(s, tx);
    changed |= status_toast::poll(s, now, tx);
    changed |= layout_state::poll(s, now, tx);
    changed |= narrow::poll(s, now, tx);
    changed |= inbox_model::poll(s, now, tx);
    changed |= view::tab_preview::poll(s, now, tx);
    changed |= session_status::poll(s, now, tx);
    // Each draft keeps its own composer in any window, saved or not (`restore::show_draft`).
    changed |= s.show_draft();
    if s.persists() {
        s.poll_restore();
        s.poll_window();
        // A window with its own id saves its frame itself (`windows`), as AppKit's one autosave
        // name can't tell windows apart; a hot build's one window isn't among them.
        if !s.framed && (s.window.id.is_empty() || cfg!(feature = "hot")) {
            s.framed = rune_mac::remember_frame("OpenCodeRuneMain");
        }
    }
    changed
}

/// As in Desktop: the server's image for a project or its letter, never a file found in its
/// folder (Seer's repo holds this app's icon, `~/code` every app's).
fn desktop_icons() -> project_icon::Icons {
    let mut icons = project_icon::Icons::default();
    icons.set_repo_icons(false);
    icons
}

fn setup(s: &mut App, waker: Waker) {
    s.waker = Some(waker.clone());
    windows::peer::set_waker(s, waker.clone());
    s.attach.set_waker(waker.clone());
    s.icons.set_waker(waker.clone());
    s.mentions.set_waker(waker.clone());
    s.quote.mention_fetch.set_waker(waker.clone());
    s.switcher.set_waker(waker.clone());
    s.home_search.set_waker(waker.clone());
    s.add.slash.set_waker(waker.clone());
    s.review.set_waker(waker.clone());
    s.files.set_waker(waker.clone());
    s.open_in.set_waker(waker.clone());
    s.turn_diffs.set_waker(waker.clone());
    s.icons.set_sync(headless());
    s.setup_browser(waker.clone());
    rune::marks::mark("browser.setup");
    if launch::deferred() && !headless() && s.launch.cold_transcript.is_some() {
        s.launch.plugins = launch::PluginSetup::AfterFrame(waker.clone());
    } else {
        telemetry::startup("launch.plugins", || plugin_host::setup(s, waker.clone()));
        rune::marks::mark("plugins.started");
    }
    s.setup_rune_mcp(waker);
    rune::marks::mark("mcp.setup");
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
            // Desktop sets every IBM Plex Mono weight below 600 in its Text cut and the rest in its
            // Bold cut (`index.css`), which a Markdown heading in a diff takes.
            include_bytes!("../assets/fonts/IBMPlexMono-Text.ttf"),
            include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf"),
            include_bytes!("../assets/fonts/CommitMono-400-Regular.otf"),
            include_bytes!("../assets/fonts/CommitMono-700-Regular.otf"),
            include_bytes!("../assets/fonts/CommitMono-400-Italic.otf"),
            include_bytes!("../assets/fonts/CommitMono-700-Italic.otf"),
            // Desktop's terminal face (	erminalDefault), Nerd Font glyphs included for prompts.
            include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
            include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Bold.ttf"),
        ],
        families: (appearance::UI_FONTS[0].family, appearance::CODE_FONTS[0].family),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new(launch::window_title(), (1280.0, 820.0), init, view::app)
    }
}

/// The app as it runs: its history, its storybook (⌘⇧D), its window chrome, and Rune's native
/// Metal backend (`RUNE_GPU_BACKEND=wgpu` draws with wgpu again, no rebuild).
pub(crate) fn rune_app(spec: AppSpec<App>) -> rune::App<App> {
    let app = rune::App::new(spec);
    // Office pages draw their Vello scenes into effect slots; a window without one draws as
    // Rune's default frame does.
    #[cfg(feature = "office")]
    let app = app.compositor(office::OfficeCompositor::default());
    menubar::install(
        app
            .gpu_backend(rune::GpuBackend::Metal)
            .playground(stories::playground())
            .on_ime(|s, ime, _| s.panels_ime(ime))
            .on_open_url(deep_links::open)
            // The page's server's copy of the session data moves into the window whose turn it is.
            .turn_scope(servers::runtime::enter, servers::runtime::leave)
            .on_modifiers(view::home_modifiers)
            .on_visibility(status_toast::visibility)
            .titlebar(rune::Titlebar::new(view::TITLEBAR_H))
            .chrome(rune::Chrome {
                full_size_content: true,
                traffic_lights: Some((20.0, 25.0)),
                drag_regions: true,
            }),
    )
}

fn main() {
    launch::trace::begin();
    telemetry::launched();
    launch::attach_console();
    askpass::run_if_asked();
    #[cfg(windows)]
    updater::apply::run_if_asked();
    #[cfg(windows)]
    install::run_if_asked();
    launch::print_version_if_asked();
    if plugin_host::luau::types::from_args() {
        return;
    }
    if genui::skill::maybe_cli() {
        return;
    }
    // A launch while the app runs hands its links (`deep_links`) to it and ends here.
    if !headless() && !rune::single_instance(&launch::instance_id()) {
        return;
    }
    rune::marks::mark("instance.claimed");
    opencode_kit::client::set_download_dir(dirs::downloads());
    telemetry::startup("launch.platform", rune_mac::launch);
    telemetry::startup("launch.notifications", notifications::identify);
    telemetry::startup("launch.gpu", rune::prewarm_gpu);
    telemetry::startup("launch.icon", launch::apply_app_icon);
    #[cfg(feature = "hot")]
    rune::run(AppSpec { families: appearance::launch_families(), ..spec(App::hot) });
    #[cfg(not(feature = "hot"))]
    {
        let diagnostics = diagnostics::Diagnostics::default();
        if !headless() {
            launch::defer();
            rune::marks::mark("windows.launch");
            // Every window saved at the last quit, each with its own tabs (`windows`).
            let opened = windows::launch(diagnostics.clone());
            disk::flush();
            rune::marks::mark("storage.flushed");
            diagnostics.shutdown();
            rune::marks::mark("diagnostics.stopped");
            launch::trace::finish(opened);
            return;
        }
        // The harness's `--realtime` probes launch as a window does: the server's first reads
        // wait for `on_ready`, which a realtime run calls after its first frame.
        if harness_writes() {
            launch::defer();
        }
        let app = rune_app(AppSpec { families: appearance::launch_families(), ..spec(App::new) });
        let handle = app.handle();
        let mut motion_log = motion_log::Log::default();
        let (encoded, drawn) = (diagnostics.clone(), diagnostics.clone());
        app.on_ready(move |_| {
            let wake = launch::inbox_waker(handle.clone());
            handle.update_if(move |s, tx| { s.launch.inbox_wake = Some(wake); launch::ready(s, tx) });
        })
            .on_frame_encoded(move |f| {
                encoded.encoded(f);
                telemetry::encoded(f);
            })
            .on_frame_drawn(move |rt| {
                drawn.drawn(rt);
                if let Some(frame) = telemetry::last_frame() { telemetry::drawn(rt.state.painted_session(), frame); }
                motion_log.frame(rt);
            })
            .on_close(|s, _| {
                s.updater.closing();
                s.save_on_close();
            })
            .run();
        disk::flush();
        diagnostics.shutdown();
    }
}

