//! OpenCode Desktop in Rune: OpenCode's home and session pages in Rune, live against the local OpenCode server or
//! from a synthetic fixture. Live, it sends prompts, answers permission requests and questions,
//! stops runs and starts sessions (`--read-only` turns all of that off).
// apps/web builds this crate for the browser; built by it for any other target, it's empty.
#![cfg(not(opencode_web_host))]

/// Counts each thread's allocations for the frame diagnostics and the benchmarks, over mimalloc:
/// the system heap costs a Luau plugin's rebuild (its tables, strings and elements) about a fifth
/// more on Windows.
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static ALLOC: procwatch::alloc::Counting<launch::last_exit::OomGuard<mimalloc::MiMalloc>> = procwatch::alloc::Counting::new(launch::last_exit::OomGuard(mimalloc::MiMalloc));

/// mimalloc commits each medium and large page whole when a thread heap takes it; on demand it
/// commits a page only as blocks are carved from it. Every thread that allocates holds such pages
/// for its size classes, mostly untouched: on Home this is 60-75 MB of commit charge that is never
/// written (private bytes ~205 MB to ~145 MB, the private working set unchanged).
#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
fn commit_pages_on_demand() {
    unsafe extern "C" {
        fn mi_version() -> std::ffi::c_int;
        fn mi_option_set(option: std::ffi::c_int, value: std::ffi::c_long);
    }
    // `mi_option_page_commit_on_demand` is 40 in mimalloc 3.3's `mi_option_t` (mimalloc.h); a
    // later mimalloc may number it otherwise, so it's only set on the version it was read from.
    // SAFETY: plain calls into the linked mimalloc; an option set after start applies to the
    // pages taken from then on.
    unsafe {
        if mi_version() / 100 == 303 {
            mi_option_set(40, 1);
        }
    }
}

/// `App::rest`: mimalloc collects this thread's heap and purges every free page now; on Windows
/// the process heap compacts too.
#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
fn purge_free_pages() {
    unsafe extern "C" {
        fn mi_collect(force: bool);
    }
    // SAFETY: a plain call into the linked mimalloc, on the UI thread.
    unsafe { mi_collect(true) }
    // The process heap (UI Automation, COM, the D3D runtime and driver) keeps freed pages
    // committed; compacting it gives its large free blocks back.
    #[cfg(windows)]
    // SAFETY: the process heap lives as long as the process; HeapCompact takes its lock.
    unsafe {
        windows_sys::Win32::System::Memory::HeapCompact(windows_sys::Win32::System::Memory::GetProcessHeap(), 0);
    }
}

// A browser's requests speak ureq's API too (ureq-web), so the request code is the same there.
#[cfg(target_arch = "wasm32")]
extern crate ureq_web as ureq;

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
mod askpass;
mod attachments;
mod auto_accept;
mod glitch;
mod glyph_masks;
mod auto_accept_story;
mod background;
mod background_stories;
mod browser;
mod capsule_stories;
pub mod code_view;
mod commands;
mod commands_stories;
mod compositor;
mod compaction_stories;
mod connect;
mod context_window;
mod crash_notice;
mod deep_links;
mod demo;
mod demo_script;
mod diagnostics;
mod dictation;
mod dirs;
mod dock_fade_stories;
mod dropzone;
mod egg;
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
mod idle;
mod inbox_model;
mod inbox_order;
mod incoming;
#[cfg(windows)]
mod install;
mod jobs;
mod jump_list;
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
mod page_sign_in;
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
#[path = "quote/card_stories.rs"]
mod quote_card_stories;
mod raster_probe_stories;
mod recently_closed;
mod recent_page;
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
mod wheel;
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
mod session_catalog;
mod transcript_index;
mod fulltext;
mod session_open;
mod session_pages;
mod session_state;
mod session_status;
mod settings;
mod shortcuts;
mod side_stories;
mod slash;
mod sounds;
mod speech;
mod reconnect;
mod status_toast;
mod stop;
mod stop_stories;
mod stories;
mod storage;
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
mod tips;
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
mod errors;
mod undo;
mod updater;
mod usage;
mod view;
mod wall;
use wall::wall_ms;
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

// A static, not a const: a const's bytes are copied into every codegen unit that names it.
static FIXTURE: &str = include_str!("../assets/fixture.json");

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
    pub source: rune::Tracked<Source>,
    inbox: rune::Tracked<Option<Receiver<Incoming>>>,
    /// The live server events taken, for the motion log (`motion_log.rs`).
    motion_events: rune::Tracked<motion_log::Events>,
    // Window-local, consumed once per poll; never a view dependency.
    tail_change: events::TailChange,
    pages: rune::Tracked<session_pages::State>,
    // Created by the first Save, never before Frame 1. A failed write wakes the window once.
    output_errors: Option<(Sender<String>, Receiver<String>)>,
    opening: rune::Tracked<Option<session_pages::Opening>>,
    open_demo: rune::Tracked<Option<open_story::Demo>>,
    /// The server's projects and sessions. This and the other server data (transcripts, running,
    /// stops, requests, pending, inboxes) are the server's copy, which every window of the server
    /// holds in turn (`windows::peer`).
    pub projects: rune::Tracked<Vec<Project>>,
    pub sessions: rune::Tracked<Vec<SessionInfo>>,
    pub recent_page: rune::Tracked<recent_page::Page>,
    pub recent_reload: recent_page::Reload,
    catalog: session_catalog::Catalog,
    session_search: switcher::search_index::SessionSearch,
    /// The palette's and Home's questions to the message text kept on this device.
    fulltext: fulltext::Surfaces,
    pub route: rune::Tracked<Route>,
    /// The open tab this window left for Home, independent of tab order or layout.
    home_return_tab: rune::Tracked<Option<String>>,
    /// Where ⌘[ and ⌘] go (`history.rs`).
    pub nav: rune::Tracked<history::History>,
    pub transcripts: rune::Tracked<HashMap<String, Transcript>>,
    transcript_edits: events::TranscriptEdits,
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
    pub sidebar_drag: rune::Tracked<Option<f32>>,
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
    jobs: rune::Tracked<jobs::Jobs>,
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
    framed: rune::Tracked<bool>,
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
    base_ms: rune::Tracked<i64>,
    clock: rune::Tracked<f64>,
    /// When the jump list was last looked at (`jump_list`).
    jump_list_at: f64,
    /// How many times the view has run, and the count the last poll saw: state that a poll
    /// derives from other state (panel tabs, plugins' watched signals) can only have moved when
    /// the view ran since, or when an earlier poll this frame changed something.
    pub view_runs: std::cell::Cell<u64>,
    polled_runs: u64,
    /// The window's timeline places and opened rows as last written (`restore::places`).
    pub(crate) timeline_kept: restore::places::Kept,
    /// Each session's assistant steps that were open when its event stream dropped: their text
    /// grows again only from whole text (`Transcript::apply_caught`, the catching-up barrier).
    pub(crate) stream_gaps: HashMap<String, std::collections::HashSet<String>>,
    fixture: rune::Tracked<Value>,
    playback: rune::Tracked<VecDeque<(f64, String, Value)>>,
    /// What the fixture's server stored from the replies it streamed (`served.rs`).
    served: rune::Tracked<served::Served>,
    background: rune::Tracked<background::Background>,
    sends: rune::Tracked<u32>,
    waker: rune::Tracked<Option<Waker>>,
    launch: rune::Tracked<launch::Launch>,
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

impl App {
    fn empty() -> App {
        let home = dirs::home();
        App {
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

    fn new() -> App {
        // A harness run that writes keeps a real window's rows, so its tabs and drafts come back.
        Self::new_window(if harness_writes() { windows::first_saved() } else { windows::Window::unsaved() })
    }

    /// A hot build's one window: the profile's first window, whose tabs and page it reads and
    /// saves as the app's own first window does.
    #[cfg(feature = "hot")]
    fn hot() -> App {
        Self::new_window(windows::first_saved())
    }

    /// `new`, for window `window`: its own tabs and page.
    fn new_window(window: windows::Window) -> App {
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

    fn load_fixture(&mut self, src: &str) {
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
            self.files.wrap = files::markdown::WrapPrefs::load();
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

    fn sort_projects(&mut self) {
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

    /// `id` and its loaded parents, up to 32 steps.
    /// `App::on_rest`: the window rests (10 s without a frame, or out of sight). What it holds
    /// for sessions no tab, route or run shows goes: their transcripts, their pages in the
    /// reader's cache, and the images only those held. A session opened again is read again,
    /// as one opened for the first time is. Nothing on screen changes.
    pub(crate) fn rest(s: &mut App, _: &mut rune::Tx) -> bool {
        let shown: HashSet<&str> = s.tabs.iter().map(|t| t.id.as_str()).chain(match &*s.route { Route::Session { id, .. } => Some(id.as_str()), _ => None }).chain(s.running.iter().map(String::as_str)).collect();
        let kept = |id: &str| s.ancestors(id).any(|a| shown.contains(a));
        let gone: Vec<String> = s.transcripts.keys().filter(|id| !kept(id)).cloned().collect();
        let review_gone: Vec<String> = s.review.sessions.keys().filter(|id| !kept(id)).cloned().collect();
        let files_gone: Vec<String> = s.files.sessions.keys().filter(|id| !kept(id)).cloned().collect();
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
        s.rest_session_search();
        // No session tab or draft and not on a session: pictures keep their encoded bytes, not
        // their pixels.
        if s.tabs.is_empty() && !matches!(&*s.route, Route::Session { .. }) {
            s.attach.store.rest_pixels();
            // The built-ins a session started sleep again, as at launch (`plugin_host::dormant`).
            if matches!(&*s.route, Route::Home) { plugin_host::dormant::sleep_unused(s); }
        }
        plugin_host::memory::collect_garbage(s);
        #[cfg(feature = "alloc-sites")]
        plugin_host::memory::publish_vms(s);
        // What all that freed: mimalloc purges freed pages only when some thread next allocates,
        // and an app at rest doesn't. Forced, it gives them back now.
        #[cfg(not(target_arch = "wasm32"))]
        purge_free_pages();
        false
    }

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

/// On a touch screen (`touch_controls()`), only a direct tap inside a text field or a hardware key
/// (`tx.by_keys()`) puts focus in the composer; opening a session or tab, leaving Settings or a
/// dialog, or tapping a toolbar button must not pop up the OS keyboard.
pub(crate) fn auto_focus_composer(tx: &Tx) -> bool {
    !session_view::style::touch_controls() || tx.by_keys()
}

fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    session_view::scroller::set_device_scale(windows::view_scale(s));
    *s.clock = now;
    s.sync_history();
    *s.now_ms = if s.live() { wall_ms() } else { *s.base_ms + (now * 1000.0) as i64 };
    demo::poll(s);
    // The morph loop is for watching: the composer it lands on doesn't take the keys.
    if s.stage.is_some_and(|st| st.watch()) {
        s.composer.focused = false;
    }
    // The view ran since the last poll: state the polls below derive from other state may have moved.
    let viewed = s.view_runs.get() != s.polled_runs;
    s.polled_runs = s.view_runs.get();
    // Another window changed the session data this one shows.
    let mut changed = windows::peer::behind(s);
    changed |= session_menu::unread::poll(s);
    changed |= session_menu::preferences::poll(s);
    changed |= s.poll_places(now, tx);
    s.poll_undo(now, tx);
    reconnect::audit(s);
    popup::poll(s, tx);
    // The servers the window isn't checked out on: their launches, replies and streams.
    changed |= servers::runtime::poll(s, tx);
    changed |= servers::health::poll(s, tx);
    changed |= projects::home::poll(s, viewed || changed, tx);
    changed |= project_switcher::poll(s);
    // A project's image the icon worker found (or lost): the tiles that read the icons draw again.
    if rune::Tracked::untracked(&s.icons).take_found() {
        rune::Tracked::touch(&s.icons);
        changed = true;
    }
    changed |= dictation::poll(s, now, tx);
    changed |= launch::poll(s, tx);
    launch::trace::poll(tx);
    changed |= updater::poll(s, tx);
    #[cfg(feature = "office")]
    {
        changed |= office::poll(s, tx);
    }
    changed |= glitch::poll(s, tx);
    changed |= egg::poll(s, now, tx);
    changed |= appearance::poll(s);
    text_rendering::poll(s, tx);
    changed |= language::poll(s, tx);
    changed |= settings::search::poll(s, tx);
    settings::pages::servers::unavailable::poll(s, tx);
    changed |= notifications::poll(s, tx);
    changed |= speech::poll(s, tx);
    // A shortcut from the composer that moved the keyboard into the side region (its command
    // may run a frame later) leaves the selected tab's label muted (`SideState::quiet`).
    let composing = s.composer.focused;
    if s.side.was_composing && !composing && s.side.focused && !s.side.pressed && s.terminal_focus().is_none() {
        s.side.quiet = true;
    }
    // Written only when it changes: a write to the side state rebuilds every region reading it.
    if (s.side.was_composing, s.side.pressed) != (composing, false) {
        (s.side.was_composing, s.side.pressed) = (composing, false);
    }
    changed |= deep_links::drain(s, tx);
    changed |= quote::poll(s, now, tx);
    changed |= line_comments::poll(s, tx);
    changed |= message_nav::poll(s, tx);
    changed |= turn_errors::poll(s, tx);
    changed |= dropzone::poll(s, tx);
    changed |= s.poll_pages(tx);
    while let Some(why) = s.output_errors.as_ref().and_then(|(_, rx)| rx.try_recv().ok()) {
        crate::status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), why);
        changed = true;
    }
    changed |= s.poll_context_window();
    changed |= s.poll_sends();
    changed |= s.tick_outbox(tx);
    changed |= s.poll_open_in();
    changed |= s.poll_jobs(tx);
    changed |= files::poll_reply_links(s, tx);
    let asked = s.dock_face();
    changed |= incoming::poll(s, tx);
    if s.playback.front().is_some_and(|e| e.0 <= now + 1e-9) {
        while s.playback.front().is_some_and(|e| e.0 <= now + 1e-9) {
            let (at, ty, mut data) = s.playback.pop_front().unwrap();
            if ty.starts_with("session.execution.") || ty == "session.archived" {
                data["eventCreated"] = (*s.base_ms + (at * 1000.0) as i64).into();
            }
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
            if let Some(sid) = data["sessionID"].as_str().filter(|sid| ty == "session.step.started" && s.route == Route::here(sid.to_string())) {
                s.refresh_fixture_page(sid);
            }
            s.live_event(ty, data);
            changed = true;
        }
    }
    changed |= stop::poll(s, now, tx);
    changed |= s.revert_timeouts(tx);
    changed |= fork::poll(s, tx);
    changed |= message_nav::jump::poll(s, tx);
    s.queue_timers(tx);
    s.start_timers(tx);
    changed |= submit::poll(s, tx);
    changed |= working::sync(s);
    changed |= subagents::shells::poll(s, tx) | todos::sync(s, now, tx) | settings::pages::worktrees::poll(s, tx);
    if s.dock_face() != asked {
        s.morph(tx);
    }
    changed |= disclosure::poll(s, now, tx);
    changed |= session_view::timeline::projection_poll(s, tx);
    changed |= line_reveal::poll(s, now, tx);
    changed |= copied::poll(s, now, tx);
    changed |= diagram::poll(s, now, tx);
    changed |= session_view::images::poll(s, now, tx);
    changed |= model_picker::hover::poll(s, now, tx);
    changed |= agent_rows::poll(s, tx);
    changed |= background::poll(s, tx);
    changed |= find::poll(s, tx);
    changed |= s.poll_home_search(tx);
    // Ingest local answers before settling the list: its rows, highlight and empty state must
    // all see the same sources. Previously a full-text landing changed the rows after this poll.
    let local_answers = s.poll_session_search(now, tx) | s.poll_fulltext();
    changed |= s.poll_switcher(tx, local_answers);
    changed |= s.poll_mentions(tx);
    changed |= s.poll_commands(tx);
    changed |= s.poll_suggestions(tx);
    changed |= s.poll_rename(now, tx);
    changed |= s.poll_branch_picker(now, tx);
    changed |= s.poll_delete(now, tx);
    changed |= s.poll_uploads(tx);
    if s.attach.poll() {
        rune::Tracked::touch(&s.attach);
        changed = true;
    }
    changed |= s.poll_panels(viewed || changed, tx);
    changed |= rune_mcp::poll(s, tx);
    changed |= s.poll_turn_diffs();
    changed |= settings::pages::project::poll(s, tx);
    changed |= settings::pages::about::poll(s, now, tx);
    changed |= settings::pages::projects::poll(s);
    changed |= reconnect::poll(s, tx);
    changed |= session_state::poll(s, tx);
    changed |= tips::poll(s, tx);
    changed |= status_toast::poll(s, now, tx);
    changed |= layout_state::poll(s, now, tx);
    changed |= narrow::poll(s, now, tx);
    changed |= inbox_model::poll(s, now, tx);
    changed |= view::tab_preview::poll(s, now, tx);
    changed |= session_status::poll(s, now, tx);
    // Each draft keeps its own composer in any window, saved or not (`restore::show_draft`).
    changed |= s.show_draft();
    if s.persists() {
        changed |= s.tell_storage_notice();
        changed |= s.hydrate_restore();
        changed |= crash_notice::poll(s);
        s.poll_restore();
        s.poll_preview(false);
        changed |= s.poll_copies(tx);
        changed |= s.poll_line_comments();
        crate::windows::reopen::poll(s);
        crate::windows::reopen::poll_asked(s);
        s.prompts.hydrate((*s.waker).clone());
        s.poll_window();
        s.poll_taskbar();
        jump_list::poll(s);
        // A window with its own id saves its frame itself (`windows`), as AppKit's one autosave
        // name can't tell windows apart; a hot build's one window isn't among them.
        if !*s.framed && (s.window.id.is_empty() || cfg!(feature = "hot")) {
            *s.framed = rune_mac::remember_frame("OpenCodeRuneMain");
        }
    }
    s.finish_tail_change(tx);
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
    *s.waker = Some(waker.clone());
    storage::on_ready(waker.clone());
    // The outbox was opened before the window had a waker: its stored prompts wake it when they're in.
    s.listen_outbox();
    windows::peer::set_waker(s, waker.clone());
    s.attach.set_waker(waker.clone());
    s.icons.set_waker(waker.clone());
    s.mentions.set_waker(waker.clone());
    s.quote.mention_fetch.set_waker(waker.clone());
    s.switcher.set_waker(waker.clone());
    s.home_search.set_waker(waker.clone());
    s.session_search.set_waker(waker.clone());
    s.find.set_waker(waker.clone());
    s.add.slash.set_waker(waker.clone());
    s.review.set_waker(waker.clone());
    s.files.set_waker(waker.clone());
    s.open_in.set_waker(waker.clone());
    s.turn_diffs.set_waker(waker.clone());
    s.icons.set_sync(headless());
    s.setup_browser(waker.clone());
    rune::marks::mark("browser.setup");
    // Only the plugins this launch's first screen shows start before its first frame (`first_screen`).
    telemetry::startup("launch.plugins", || plugin_host::setup(s, waker.clone()));
    rune::marks::mark("plugins.started");
    s.setup_rune_mcp(waker);
    rune::marks::mark("mcp.setup");
    if !s.playback.is_empty() {
        s.wake_at_playback();
    }
    launch::setup(s);
}

/// The app's fonts. Desktop sets every IBM Plex Mono weight below 600 in its Text cut and the rest
/// in its Bold cut (`index.css`), which a Markdown heading in a diff takes, and italic Plex in its
/// Text Italic cut (inline code in emphasis). JetBrains Mono is desktop's terminal face
/// (`terminalDefault`), Nerd Font glyphs included for prompts.
#[cfg(not(target_arch = "wasm32"))]
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/InterVariable.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Text.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-TextItalic.ttf"),
    include_bytes!("../assets/fonts/CommitMono-400-Regular.otf"),
    include_bytes!("../assets/fonts/CommitMono-700-Regular.otf"),
    include_bytes!("../assets/fonts/CommitMono-400-Italic.otf"),
    include_bytes!("../assets/fonts/CommitMono-700-Italic.otf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Bold.ttf"),
];
/// In a browser, only the fonts the default appearance draws in: the page fetches Plex's italic and
/// Commit Mono once its first frame is up (`LATE_FONTS`), and JetBrains Mono, the terminal's (4.8 MB), when a
/// terminal first attaches (`TERMINAL_FONTS`).
#[cfg(target_arch = "wasm32")]
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/InterVariable.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Text.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf"),
];
/// The fonts a browser fetches after its first frame, which the web build serves from `fonts/`
/// beside the page (`scripts/web.ps1`).
#[cfg(target_arch = "wasm32")]
pub(crate) const LATE_FONTS: &[&str] = &[
    "fonts/IBMPlexMono-TextItalic.ttf",
    "fonts/CommitMono-400-Regular.otf",
    "fonts/CommitMono-700-Regular.otf",
    "fonts/CommitMono-400-Italic.otf",
    "fonts/CommitMono-700-Italic.otf",
];
/// The terminal's face, which a page fetches when its first terminal attaches (`terminal::fetch_terminal_font`).
#[cfg(target_arch = "wasm32")]
pub(crate) const TERMINAL_FONTS: &[&str] = &["fonts/JetBrainsMonoNerdFontMono-Regular.ttf", "fonts/JetBrainsMonoNerdFontMono-Bold.ttf"];

/// The app's spec: `init` makes its state (`App::new`, or `App::empty` in tests).
pub(crate) fn spec(init: fn() -> App) -> AppSpec<App> {
    rune::text::set_face_weights(appearance::FACE_WEIGHTS);
    // Desktop's base.css sets `text-wrap-style: pretty` on every element.
    rune::text::set_pretty_wrap(true);
    AppSpec {
        scenes: stories::scenes(),
        fonts: FONTS,
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
    // Office pages draw their Vello scenes into effect slots, and the new-session Easter egg its
    // pins and lodger; a window without one draws as Rune's default frame does.
    let app = app.compositor(compositor::AppCompositor::default());
    menubar::install(
        app
            .gpu_backend(rune::GpuBackend::Metal)
            .playground(stories::playground())
            .on_ime(|s, ime, _| s.panels_ime(ime))
            .on_open_url(deep_links::open)
            // The page's server's copy of the session data moves into the window whose turn it is.
            .turn_scope(servers::runtime::enter, servers::runtime::leave)
            .on_modifiers(view::home_modifiers)
            .on_visibility(visibility)
            // Tab reaches every control on a Mac whatever its Keyboard navigation setting says,
            // as official's page does in Chromium: with the setting off (macOS's default), shipped
            // 2.2.7 Tabs through Home, Search sessions, New session, a reply's buttons and the
            // prompt, where AppKit's rule left only the prompt and the tab strip.
            .tab_visits_controls(true)
            .titlebar(rune::Titlebar::new(view::TITLEBAR_H))
            .chrome(rune::Chrome {
                full_size_content: true,
                traffic_lights: Some((20.0, 25.0)),
                drag_regions: true,
            }),
    )
}

/// A window shown or hidden: hidden, what it changed lands at once, without waiting for it
/// (Desktop flushes its stores on `visibilitychange` to hidden, `renderer/platform/storage.ts`).
fn visibility(s: &mut App, shown: bool, tx: &mut Tx) {
    if shown { s.reload_recent_on_focus(); }
    status_toast::visibility(s, shown, tx);
    egg::visibility(s, shown, tx);
    if !shown && s.persists() {
        s.poll_restore();
        s.poll_preview(true);
        storage::flush(opencode_storage::FlushReason::Hide, false);
    }
}

/// The browser build's entry (apps/web).
#[cfg(target_arch = "wasm32")]
#[path = "../../web/src/web.rs"]
mod web;

#[cfg(not(any(target_arch = "wasm32", feature = "hot")))]
thread_local! {
    /// The launch's frame diagnostics, for `quit_last` to drain.
    static QUIT_DIAGNOSTICS: std::cell::RefCell<Option<diagnostics::Diagnostics>> = const { std::cell::RefCell::new(None) };
}

/// The windows' last step on the way out (`rune::fast_exit`), on the UI thread once every window's
/// quit hook ran: the database's last write, the diagnostics' queue, an install-on-quit's helper
/// (it must exist before this process ends), the launch trace, then a training build's profile.
/// Rune's fast exit raises every thread to normal priority before the process ends itself, so an
/// idle one can't hold it.
#[cfg(not(any(target_arch = "wasm32", feature = "hot")))]
fn quit_last() {
    // Landed and closed: the file's lock goes before the update helper takes over.
    storage::close();
    rune::marks::mark("storage.flushed");
    // Nothing of the app's is left to lose: whatever the rest waits on, the process ends within
    // 2 s, saying what it waited for. A training build (`scripts/pgo.ps1`) writes ~50 MB of counts
    // last; on a loaded machine that outlasted 2 s and left truncated, unmergeable files.
    rune::exit_deadline(std::time::Duration::from_secs(if cfg!(pgo_generate) { 120 } else { 2 }));
    // A launch handed on while the app quit claims it now, unless an update replaces it.
    if !updater::installs_on_quit() {
        rune::release_instance();
    }
    QUIT_DIAGNOSTICS.with(|d| if let Some(d) = d.borrow_mut().take() { d.shutdown() });
    rune::marks::mark("diagnostics.stopped");
    updater::finish_quit();
    rune::marks::mark("quit.update_handed_off");
    launch::trace::finish(windows::opened());
    launch::write_profile();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    commit_pages_on_demand();
    #[cfg(feature = "alloc-sites")]
    plugin_host::memory::register_reports();
    // A debug build COM starts for a proof's scratch toast activator (`LocalServer32`, whose
    // command line is fixed in the registry and whose environment is the user's) takes its
    // scratch root from the command line: it never runs without one. Release builds ignore it.
    #[cfg(debug_assertions)]
    if let Some(root) = std::env::args().find_map(|a| a.strip_prefix("--dirs-root=").map(str::to_owned)) {
        // SAFETY: the first thing `main` does, before any other thread exists.
        #[allow(unsafe_code)]
        unsafe { std::env::set_var("RUNE_DIRS_ROOT", root) };
    }
    launch::trace::begin();
    telemetry::launched();
    launch::attach_console();
    askpass::run_if_asked();
    #[cfg(windows)]
    updater::apply::run_if_asked();
    #[cfg(windows)]
    install::run_if_asked();
    launch::print_version_if_asked();
    if plugin_host::types::from_args() {
        return;
    }
    if genui::skill::maybe_cli() {
        return;
    }
    // The app's state, read on its own thread beside the instance claim (`storage::start`). A
    // launch while the app runs hands its links (`deep_links`) to it and ends here.
    storage::start();
    if !headless() && !rune::single_instance(&launch::instance_id()) {
        return launch::trace::finish(0);
    }
    storage::claimed();
    launch::last_exit::init_hooks();
    notifications::take_activation(rune::extra_args());
    glyph_masks::start();
    // The first view's theme tokens (OpenCode's kit and the default theme's palette, both
    // appearances from one parse), built while the window is made rather than in its first frame.
    let _ = std::thread::Builder::new().name("theme-warm".into()).spawn(|| {
        for dark in [true, false] {
            session_view::palette::kit(session_view::palette::pal(dark));
        }
    });
    opencode_kit::client::set_download_dir(dirs::downloads());
    // Where the browser engine lives follows this app's data root (no I/O until a page opens).
    #[cfg(any(target_os = "macos", windows))]
    if let Some(engine) = dirs::engine() {
        browser_pane::installer::set_home(engine);
    }
    telemetry::startup("launch.platform", rune_mac::launch);
    telemetry::startup("launch.notifications", notifications::identify);
    telemetry::startup("launch.gpu", rune::prewarm_gpu);
    // Windows: no GPU before the first frame. Rune paints it on the CPU and makes the window's GPU
    // on a below-normal thread once it's on screen, then hands over. Warming the GPU here put its
    // driver loads beside the launch's: the first frame came 8 ms and the first input 15 ms later
    // (p50, visible desktop), for a hand-over ~50 ms sooner that nobody sees.
    telemetry::startup("launch.icon", launch::apply_app_icon);
    #[cfg(feature = "hot")]
    rune_app(AppSpec { families: appearance::launch_families(), ..spec(App::hot) }).run();
    #[cfg(not(feature = "hot"))]
    {
        let diagnostics = diagnostics::Diagnostics::default();
        if !headless() {
            launch::defer();
            rune::marks::mark("windows.launch");
            // Quitting ends the process right after `quit_last`: no window, GPU device or allocation
            // is torn down (on Windows that was the graphics drivers' detach, hundreds of ms).
            QUIT_DIAGNOSTICS.with(|d| *d.borrow_mut() = Some(diagnostics.clone()));
            rune::fast_exit(quit_last);
            // Every window saved at the last quit, each with its own tabs (`windows`).
            windows::launch(diagnostics);
            quit_last();
            return;
        }
        // The harness's `--realtime` probes launch as a window does: the server's first reads
        // wait for `on_ready`, which only a realtime run calls (after its first frame).
        if harness_writes() && std::env::args().any(|a| a == "--realtime") {
            launch::defer();
        }
        let app = rune_app(AppSpec { families: appearance::launch_families(), ..spec(App::new) });
        let handle = app.handle();
        let mut motion_log = motion_log::Log::default();
        let (encoded, drawn) = (diagnostics.clone(), diagnostics.clone());
        let app = app
            .on_ready(move |_| {
                storage::presented();
                let wake = launch::inbox_waker(handle.clone());
                handle.update_if(move |s, tx| { s.launch.inbox_wake = Some(wake); launch::ready(s, tx) });
            })
            .on_frame_encoded(move |f| {
                encoded.encoded(f);
                telemetry::encoded(f);
                files::trace::frame(f);
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
            .on_rest(App::rest);
        app.run();
        storage::close();
        diagnostics.shutdown();
        // A headless run's marks too (`--launch-trace`): the harness's realtime probes, such as the
        // browser engine's timing.
        launch::trace::finish(1);
    }
}
