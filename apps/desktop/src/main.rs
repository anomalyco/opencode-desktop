//! OpenCode Desktop in Rune: OpenCode's home and session pages in Rune, live against the local OpenCode server or
//! from a synthetic fixture. Live, it sends prompts, answers permission requests and questions,
//! stops runs and starts sessions (`--read-only` turns all of that off).

/// Counts each thread's allocations for the frame diagnostics and the benchmarks.
#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

use crate::pickers::PickerHost as _;
pub use session_view::{
    agent_start, disclosure, fold, jump, line_reveal, odometer, palette, question_row,
    subagent_card as agent_rows_view, themes, turns,
};

#[cfg_attr(test, path = ".")]
mod compaction {
    pub use session_view::compaction::*;
    #[cfg(test)]
    use crate::{App, Route};
    #[cfg(test)]
    #[path = "compaction_tests.rs"]
    mod tests;
}
#[cfg_attr(test, path = ".")]
mod copied {
    pub use session_view::copied::*;
    #[cfg(test)]
    #[path = "copied_tests.rs"]
    mod tests;
}
mod diagram {
    pub use session_view::diagram::*;
    pub(crate) mod stories;
    #[cfg(test)]
    mod tests;
}
mod genui {
    pub use session_view::genui::*;
    pub(crate) mod demo;
    pub(crate) mod diagrams;
    pub(crate) mod skill;
    #[cfg(test)]
    mod tests;
}
#[cfg_attr(test, path = ".")]
mod turn_errors {
    pub use session_view::turn_errors::*;
    #[cfg(test)]
    use crate::{App, Route};
    #[cfg(test)]
    #[path = "turn_errors_tests.rs"]
    mod tests;
}
#[cfg_attr(test, path = ".")]
mod working {
    pub use session_view::working::*;
    #[cfg(test)]
    use crate::App;
    #[cfg(test)]
    use rune::Color;
    #[cfg(test)]
    use rune::color::srgb_encode;
    #[cfg(test)]
    #[path = "working_tests.rs"]
    mod tests;
}

mod add_menu;
mod agent_rows;
mod agent_start_stories;
mod agent_stories;
mod app_menu;
mod appearance;
mod appearance_stories;
mod arrivals;
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
#[cfg(test)]
mod compaction_fold_tests;
mod demo;
mod demo_script;
mod diagnostics;
mod dictation;
#[cfg(test)]
mod dictation_tests;
mod dirs;
mod disk;
mod dock_fade_stories;
mod dropzone;
mod events;
mod files;
mod find;
mod find_stories;
#[cfg(test)]
mod find_tests;
mod fixture_ids;
mod fold_stories;
#[cfg(test)]
mod fold_tests;
mod fork;
mod forms;
mod frosted_stories;
mod composer_grow_stories;
mod history;
mod history_story;
#[cfg(test)]
mod history_tests;
mod home_search;
#[cfg(test)]
mod home_search_tests;
mod icons;
mod inbox_model;
#[cfg(test)]
mod interaction_fuzz;
#[cfg(test)]
mod jump_tests;
mod keymap;
mod keys;
mod knobs;
mod launch;
mod layout_state;
mod mention_stories;
mod mentions;
mod menu_stories;
mod model_picker;
mod model_prefs;
#[cfg(test)]
mod tool_group_shut_tests;
#[cfg(test)]
mod shell_fold_tests;
#[cfg(test)]
mod motion_gate;
mod motion_log;
mod new_session;
mod new_worktree;
mod notifications;
mod odometer_stories;
mod title_change;
mod open_in;
#[cfg(test)]
mod open_replay;
mod open_story;
#[cfg(test)]
mod open_story_tests;
pub mod panels;
#[cfg(test)]
mod perf;
mod permission;
mod pickers;
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
#[cfg(test)]
mod revert;
mod rewind;
mod rewind_stories;
mod rune_mcp;
mod scenes;
mod scrollbar;
mod shell_fold;
mod short_group;
mod selection_stories;
mod send;
mod served;
mod session_menu;
mod session_menu_stories;
#[cfg(test)]
mod session_menu_tests;
mod session_menu_view;
#[cfg(test)]
mod session_older_tests;
#[cfg(test)]
mod session_stall_tests;
mod send_failed;
mod session_open;
mod session_pages;
mod settings;
mod shortcuts;
mod side_stories;
mod slash;
mod reconnect;
mod status_toast;
#[cfg(test)]
mod status_toast_tests;
mod stop;
mod stop_stories;
mod stories;
#[path = "stream/pace.rs"]
mod stream_pace;
#[path = "stream/line_grow_story.rs"]
mod line_grow_story;
#[cfg(test)]
#[path = "stream/line_grow_tests.rs"]
mod line_grow_tests;
mod subagents;
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
#[cfg(test)]
mod tool_open_tests;
mod turn_errors_stories;
mod updater;
mod view;
mod web_fetch;

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
    /// "New session": an empty page with the composer, shown by the draft tab with this id.
    Draft(String),
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
    /// Grow, then show's streaming words, held a line-grow behind (`line_reveal`).
    pub line_hold: line_reveal::Hold,
    /// The copy button showing its tick (`copied.rs`).
    pub copied: copied::Copied,
    /// The diagrams replies show, and the one open full size (`diagram`).
    pub diagram: diagram::Diagrams,
    /// Pinned session tabs, in sidebar order (see `tabs.rs` for where they come from).
    pub tabs: Vec<Tab>,
    /// A tab being dragged to a new place in the sidebar.
    pub tab_drag: tabs::Reorder,
    /// Closed session tabs, newest last, with the index they had (⌘⇧T reopens; `ui.json` keeps them).
    pub closed: tabs::Closed,
    /// The tab layout's switch and the collapsed sidebar's peek (`layout_state.rs`).
    pub layout: layout_state::Shell,
    /// The Inbox layout's request times, branches and height (`inbox_model.rs`).
    pub sidebar_inbox: inbox_model::SidebarInbox,
    /// UI zoom, 1.0 = 100% (see `view::app`).
    pub zoom: f32,
    /// Sessions running right now (any depth; a tab is busy when anything in its family runs).
    pub running: HashSet<String>,
    /// Runs stopped on a press, until the server confirms (`stop.rs`).
    pub stops: stop::Stops,
    /// Pending permission and form requests: (session, request id).
    pub requests: HashSet<(String, String)>,
    /// The open permission requests and forms we know the details of.
    pub pending: Vec<Pending>,
    /// Prompts sent while a session was busy that the server hasn't delivered yet, by session.
    pub inboxes: HashMap<String, Inbox>,
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
    pub find: find::Find,
    pub search: TextEdit,
    pub home_search: home_search::HomeSearch,
    /// Session menus, the title being renamed and Delete's confirmation.
    pub menus: session_menu::Menus,
    /// The drawn menu or picker open now (off macOS).
    pub popup: popup::Popup,
    pub pickers: pickers::Pickers,
    /// The composer's `@` popover and what it lists.
    pub mentions: mentions::Mentions,
    /// Live look-and-feel sliders (⌘⇧K).
    pub knobs: rune_knobs::Knobs,
    /// The frame recorder and its scrubber, for animation glitches (⌘⇧⌥R).
    pub glitch: glitch::Glitch,
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
    /// Replies to subagents still on their way, to take back if the send fails.
    pub subagent_replies: subagents::reply::Sent,
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
    pub files: files::FilesPane,
    pub open_in: open_in::OpenIn,
    /// What a tool step last opened in the side region (`tool_open`).
    pub tool_open: Option<tool_open::Opened>,
    pub terminal: terminal::TerminalPane,
    pub rune_mcp: rune_mcp::WindowMcp,
    /// Native plugins over the C ABI: their panels, commands and settings rows (`plugin_host`).
    pub plugins: plugin_host::PluginHost,
    /// A storybook (⌘⇧D) story's stage, in the world the playground made for it.
    pub stage: Option<stories::Stage>,
    /// The story's variant (1 to 9 in the playground pick it).
    pub variant: usize,
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
    fn active_session_id(&self) -> Option<&str> {
        match &self.route {
            Route::Session(id) => Some(id.as_str()),
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
            || (self.transcripts.get(session).is_some_and(Transcript::busy) && !self.stops.has(session))
    }
    fn is_session_stopped(&self, session: &str) -> bool {
        self.stops.has(session)
    }
    fn session_revert(&self, session: &str) -> Option<&str> {
        self.session(session).and_then(|i| i.revert.as_deref())
    }
    fn for_each_tracked_session(&self, mut f: impl FnMut(&str)) {
        for id in self.transcripts.keys().chain(self.running.iter()) {
            f(id);
        }
    }
    fn expanded(&self) -> &HashSet<String> {
        &self.expanded
    }
    fn expanded_mut(&mut self) -> &mut HashSet<String> {
        &mut self.expanded
    }
    fn shutting_map(&self) -> &HashMap<String, f64> {
        &self.shutting
    }
    fn shutting_map_mut(&mut self) -> &mut HashMap<String, f64> {
        &mut self.shutting
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
    fn tool_row_action(&self, p: &palette::Pal, session: &str, call: &session_view::tools::Call, file: Option<&str>) -> Option<rune::El> {
        tool_open::action(self, p, session, call, file)
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
    fn loading_error(&self, session: &str) -> Option<&str> {
        self.opening.as_ref().filter(|o| o.id == session).and_then(|o| o.error.as_deref())
    }
    fn loading_older(&self, session: &str) -> bool {
        self.pages.loading_older(session)
    }
    fn more_above(&self, session: &str) -> bool {
        self.pages.more_above(session)
    }
    fn watch_scroll_top(&self, session: &str) -> Option<Box<dyn Fn(rune::ScrollPos)>> {
        Some(Box::new(self.pages.watch_top(session)))
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
        self.glows.get(id).copied()
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

fn wall_ms() -> i64 {
    #[cfg(test)]
    if let Some(ms) = interaction_fuzz::wall_override() {
        return ms;
    }
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

    fn load() -> App {
        let mut app = Self::empty();
        let fixture_path = arg("--fixture");
        let server = launch::initial_server();
        let deferred = fixture_path.is_none() && launch::deferred();
        let want_live = fixture_path.is_none() && !demo::requested() && (deferred || flag("--live") || (!headless() && !cfg!(test)));
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
        let mut app = Self::load();
        appearance::apply_args(&mut app);
        app.apply_appearance();
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
        self.tabs = ids.iter().map(|id| Tab::session(id.clone(), "", "")).collect();
        self.running = v["running"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        for r in v["requests"].as_array().into_iter().flatten() {
            if let (Some(s), Some(id)) = (r["sessionID"].as_str(), r["id"].as_str()) {
                self.requests.insert((s.to_owned(), id.to_owned()));
            }
        }
        self.status = "Fixture".into();
        self.open_in = open_in::OpenIn::with(std::sync::Arc::new(open_in::Fixed(vec!["vscode", "cursor", "zed", "terminal", "ghostty", "xcode"])));
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
        let first = if launch::deferred() {
            self.launch.waiting = true;
            self.launch.fallback_fixture = fallback_fixture;
            None
        } else {
            match launch::First::fetch(&server) {
                Ok(f) => Some(f),
                Err(_) if fallback_fixture => return false,
                Err(_) => { self.launch.offline = true; None }
            }
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
        self.layout = layout_state::Shell::at_rest(self.settings.tab_layout);
        self.load_open_in();
        if self.persists() {
            self.files.wrap = files::markdown::WrapPrefs::load();
        }
        self.prompts = prompt_history::PromptHistory::load();
        self.pickers.prefs = model_prefs::load();
        self.restore_ui(&server.base);
        if self.launch.offline { (self.tabs.clear(), self.route = Route::Home); }
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
        if dictation::key(self, ev, tx) || background::key(self, ev, tx) || find::shortcut(self, ev, tx) || commands::key(self, ev, tx) || plugin_host::key(self, ev, tx) {
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
            Action::Tab(offset) => self.select_adjacent(offset, false, tx),
            Action::UnreadTab(offset) => self.select_adjacent(offset, true, tx),
            Action::TabAt(i) => {
                let id = if i == 8 { self.tabs.last() } else { self.tabs.get(i) }.map(|t| t.id.clone());
                if let Some(id) = id {
                    self.select_tab(&id, tx);
                }
            }
            Action::NewSession => self.new_session(tx),
            Action::OpenProject => self.open_project_dialog(tx),
            Action::SelectProject => self.toggle_project_switcher(tx),
            Action::Settings => self.toggle_settings(tx),
            Action::AutoAccept => self.toggle_auto_accept(tx),
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
        matches!(&self.route, Route::Session(id) if self.busy(id))
    }

    /// `+`, ⌘T and New session: a new "New session" draft tab, in the open session's, draft's or
    /// Home project's folder, as Desktop's `openDraft`.
    pub fn new_session(&mut self, tx: &mut Tx) {
        self.close_settings();
        self.close_picker(false);
        self.close_project_switcher(false);
        self.project_state.home_menu_open = false;
        let dir = match &self.route {
            Route::Session(id) => self.session(id).map(|s| s.directory.clone()),
            Route::Home => self
                .project_state
                .home_selected
                .as_deref()
                .and_then(|id| self.project(id))
                .or_else(|| self.used_projects().into_iter().next())
                .map(|p| p.directory.clone()),
            Route::Draft(_) => Some(self.draft_dir().to_owned()),
        };
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
    let mut changed = dictation::poll(s, now, tx);
    changed |= launch::poll(s, tx);
    changed |= updater::poll(s, tx);
    changed |= glitch::poll(s, tx);
    changed |= appearance::poll(s);
    changed |= notifications::poll(s, tx);
    changed |= quote::poll(s, now, tx);
    changed |= turn_errors::poll(s, tx);
    changed |= dropzone::poll(s, tx);
    changed |= s.poll_pages(tx);
    changed |= s.poll_sends();
    changed |= s.poll_open_in();
    let mut arrivals = None;
    let asked = s.dock_face();
    if let Some(inbox) = s.inbox.take() {
        if let Ok(first_in) = inbox.try_recv() {
            arrivals = Some(arrivals::Arrivals::before(s));
            for m in std::iter::once(first_in).chain(inbox.try_iter()) {
                if !matches!(m, Incoming::Status(_) | Incoming::SendFailed { .. }) {
                    reconnect::heard(s);
                }
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
                        s.judge_sends_by_page(&session, &messages);
                        s.check_sends();
                        let cold = s.launch.cold_transcript.as_deref() == Some(session.as_str());
                        if cold {
                            s.launch.cold_transcript = None;
                        }
                        let first = !s.transcripts.contains_key(&session);
                        let t = s.transcripts.entry(session.clone()).or_insert_with(|| Transcript { session: session.clone(), messages: vec![] });
                        attachments::reconcile(&mut s.attach.echoes, t, &messages);
                        let delivered: Vec<String> = messages.iter().map(|m| m.id().to_owned()).collect();
                        t.merge(messages);
                        reconnect::page_in(s, &session);
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
                    Incoming::Status(e) => {
                        if !reconnect::dropped(s, &e, tx) && !rewind::failed(s, &e) && !stop::failed(s, &e) && !permission::failed(s, &e) {
                            subagents::reply::failed(s, &e);
                            status_toast::from_server(s, e);
                        }
                    }
                    Incoming::SendFailed { session, id, error } => {
                        if !s.start_failed(&session, &id, &error, tx) && !s.send_failed(&id, &error) {
                            status_toast::from_server(s, error);
                        }
                    }
                    Incoming::Pending { session, items } => {
                        s.pending.retain(|p| p.session() != session || items.iter().any(|i| i.id() == p.id()));
                        for p in items {
                            s.add_pending(p);
                        }
                    }
                    Incoming::Reconnected => {
                        tx.instant();
                        s.sidebar_inbox.branches.reconnected();
                        reconnect::back(s);
                        s.stream_back();
                        s.check_sends();
                        s.resync();
                    }
                    Incoming::Active(ids) => {
                        tx.instant();
                        s.stops.reconcile(&ids);
                        reconnect::active(s, &ids);
                        // A stop the server hasn't confirmed yet stays stopped.
                        s.running = ids.into_iter().filter(|id| !s.stops.pending(id)).collect();
                    }
                    Incoming::Projects(projects) => {
                        tx.instant();
                        s.projects = projects;
                        s.sort_projects();
                        s.list_unlisted_worktrees();
                    }
                    Incoming::ProjectOpened { location, projects, session } => {
                        s.apply_project_opened(location, projects, session, tx);
                    }
                    Incoming::Forked { from, before, session } => {
                        fork::applied(s, from, before, session, tx);
                    }
                    Incoming::Created(info) if s.starts.has(&info.id) => {
                        s.started(info, tx);
                    }
                    Incoming::Created(info) => {
                        let id = info.id.clone();
                        // A command sent from a draft started it: the draft's tab becomes its tab.
                        s.promote_draft(&id, &info.directory);
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
            if let Some(sid) = data["sessionID"].as_str().filter(|sid| ty == "session.step.started" && s.route == Route::Session(sid.to_string())) {
                s.refresh_fixture_page(sid);
            }
            changed = true;
        }
    }
    if let Some(arrivals) = arrivals {
        arrivals.land(s, now, tx);
    }
    changed |= stop::poll(s, now, tx);
    changed |= working::sync(s);
    changed |= compaction::sync(s);
    changed |= subagents::sync(s) | todos::sync(s, now, tx);
    if s.dock_face() != asked {
        s.morph(tx);
    }
    let glow = knobs::glow_secs(&s.knobs);
    s.glows.retain(|_, at| now - *at < glow);
    if !s.glows.is_empty() {
        tx.wake_after(std::time::Duration::ZERO);
        changed = true;
    }
    changed |= disclosure::poll(s, now, tx);
    changed |= line_reveal::poll(s, now, tx);
    changed |= copied::poll(s, now, tx);
    changed |= diagram::poll(s, now, tx);
    changed |= model_picker::hover::poll(s, now, tx);
    changed |= agent_rows::poll(s, tx);
    changed |= find::poll(s);
    changed |= s.poll_home_search(tx);
    changed |= s.poll_switcher(tx);
    changed |= s.poll_mentions(tx);
    changed |= s.poll_commands();
    changed |= s.poll_rename();
    changed |= s.poll_uploads();
    changed |= s.attach.poll();
    changed |= s.poll_panels(tx);
    changed |= rune_mcp::poll(s, tx);
    changed |= reconnect::poll(s, tx);
    changed |= status_toast::poll(s, now, tx);
    changed |= layout_state::poll(s, now, tx);
    changed |= inbox_model::poll(s, now, tx);
    if s.persists() {
        s.poll_restore();
        if !s.framed {
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
    s.attach.set_waker(waker.clone());
    s.icons.set_waker(waker.clone());
    s.mentions.set_waker(waker.clone());
    s.switcher.set_waker(waker.clone());
    s.home_search.set_waker(waker.clone());
    s.add.slash.set_waker(waker.clone());
    s.review.set_waker(waker.clone());
    s.files.set_waker(waker.clone());
    s.open_in.set_waker(waker.clone());
    s.icons.set_sync(headless());
    s.setup_browser(waker.clone());
    plugin_host::setup(s, waker.clone());
    s.setup_rune_mcp(waker);
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
            // Desktop sets every IBM Plex Mono weight below 600 in its Text cut (`index.css`).
            include_bytes!("../assets/fonts/IBMPlexMono-Text.ttf"),
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
    find::install(history::install(
        rune::App::new(spec)
            .gpu_backend(rune::GpuBackend::Metal)
            .playground(stories::playground())
            .on_ime(|s, ime, _| s.panels_ime(ime))
            .titlebar(rune::Titlebar::new(view::TITLEBAR_H))
            .chrome(rune::Chrome {
                full_size_content: true,
                traffic_lights: Some((25.0, 25.0)),
                drag_regions: true,
            }),
    ))
}

fn main() {
    launch::print_version_if_asked();
    if genui::skill::maybe_cli() {
        return;
    }
    telemetry::launched();
    opencode_kit::client::set_download_dir(dirs::downloads());
    rune_mac::launch();
    rune::prewarm_gpu();
    launch::apply_app_icon();
    #[cfg(feature = "hot")]
    rune::run(AppSpec { families: appearance::launch_families(), ..spec(App::new) });
    #[cfg(not(feature = "hot"))]
    {
        if !headless() {
            launch::defer();
        }
        let app = rune_app(AppSpec { families: appearance::launch_families(), ..spec(App::new) });
        let handle = app.handle();
        let mut motion_log = motion_log::Log::default();
        let diagnostics = diagnostics::Diagnostics::default();
        let (encoded, drawn) = (diagnostics.clone(), diagnostics.clone());
        app.on_ready(move |_| handle.update_if(|s, tx| { launch::ready(s, tx); false }))
            .on_frame_encoded(move |f| {
                encoded.encoded(f);
                telemetry::encoded(f);
            })
            .on_frame_drawn(move |rt| {
                drawn.drawn(rt);
                if let Some(frame) = telemetry::last_frame() { telemetry::drawn(rt.state.painted_session(), frame); }
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
        self.into_runtime_sized((1280.0, 820.0))
    }

    /// [`Self::into_runtime`] in a `size` window.
    pub(crate) fn into_runtime_sized(self, size: (f32, f32)) -> rune::Runtime<Self> {
        let mut rt = rune::Runtime::new(self, view::app, size, rune::Env::new(true, 1.0));
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
