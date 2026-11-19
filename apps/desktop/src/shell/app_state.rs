//! Cohesive domain state clusters embedded on [`crate::App`].
//!
//! Every cluster is a plain non-`Tracked` struct whose individual fields retain their own
//! [`rune::Tracked`] cells, preserving exact per-field reactive tracking granularity while
//! keeping `struct App` compact and navigable.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use crate::{
    add_menu, agent_rows, appearance, attachments, background, browser, connect, copied,
    deep_links, demo, diagnostics, diagram, dictation, disclosure, files, find, fork, glitch,
    jump, language, line_comments, line_reveal, mcp, mentions, message_nav, motion_log,
    new_session, new_worktree, notifications, open_in, open_story, panels, pickers, pinch,
    plugin_host, popup, project_switcher, projects, prompt_history, queue, quote, reconnect,
    review, rewind, rune_mcp, served, session_menu, session_state, status_toast, stories, submit,
    suggestions, switcher, tabs, terminal, text_rendering, tips, todos, tool_open, undo, updater,
    usage, view, working,
};

/// Fixture playback, demo script, and storybook playground state.
#[derive(Default)]
pub struct FixtureRun {
    pub(crate) fixture: rune::Tracked<Value>,
    pub(crate) playback: rune::Tracked<VecDeque<(f64, String, Value)>>,
    /// What the fixture's server stored from the replies it streamed (`served.rs`).
    pub(crate) served: rune::Tracked<served::Served>,
    pub(crate) base_ms: rune::Tracked<i64>,
    pub(crate) open_demo: rune::Tracked<Option<open_story::Demo>>,
    pub demo: rune::Tracked<demo::Demo>,
    /// A storybook (⌘⇧D) story's stage, in the world the playground made for it.
    pub stage: rune::Tracked<Option<stories::Stage>>,
    /// The story's variant (1 to 9 in the playground pick it).
    pub variant: rune::Tracked<usize>,
}

/// Diagnostics, debug bar, live knobs, frame recorder, and motion log state.
#[derive(Default)]
pub struct DiagState {
    /// `--debug`: the status bar with the edge, the source and the last status.
    pub debug: rune::Tracked<bool>,
    pub status: rune::Tracked<String>,
    /// Live look-and-feel sliders (⌘⇧K).
    pub knobs: rune::Tracked<Box<rune_knobs::Knobs>>,
    /// The frame recorder and its scrubber, for animation glitches (⌘⇧⌥R).
    pub glitch: rune::Tracked<glitch::Glitch>,
    /// The live server events taken, for the motion log (`motion_log.rs`).
    pub(crate) motion_events: rune::Tracked<motion_log::Events>,
    /// The frame diagnostics every window reports to.
    pub diagnostics: rune::Tracked<diagnostics::Diagnostics>,
}

/// Modal dialogs, popovers, command/project switchers, menus, toasts, and onboarding tips.
#[derive(Default)]
pub struct DialogStates {
    pub usage: rune::Tracked<usage::Usage>,
    /// Session menus, the title being renamed and Delete's confirmation.
    pub menus: rune::Tracked<Box<session_menu::Menus>>,
    /// The drawn menu or picker open now (off macOS).
    pub popup: rune::Tracked<Box<popup::Popup>>,
    pub switcher: rune::Tracked<Box<switcher::Switcher>>,
    pub project_switcher: rune::Tracked<project_switcher::ProjectSwitcher>,
    /// A server's folder picker, while one is open (`projects::remote_picker`).
    pub remote_picker: rune::Tracked<projects::remote_picker::Picker>,
    /// The connect dialog (`connect`).
    pub connect: rune::Tracked<Box<connect::Connect>>,
    /// The MCP servers' toggles and the MCPs dialog (`mcp`).
    pub mcp: rune::Tracked<Box<mcp::Mcp>>,
    pub fork: rune::Tracked<Box<fork::ForkState>>,
    pub jump: rune::Tracked<message_nav::jump::JumpState>,
    pub rewind: rune::Tracked<rewind::RewindState>,
    pub toast: rune::Tracked<status_toast::StatusToast>,
    /// The paced one-time tips (`tips.rs`).
    pub tips: rune::Tracked<tips::Tips>,
    /// Drafts that start in a new worktree, worktree lists and the branch picker (`new_worktree.rs`).
    pub new_worktree: rune::Tracked<Box<new_worktree::NewWorktree>>,
}

/// Side/bottom workspace surfaces, panes, external openers, and client MCP hub.
#[derive(Default)]
pub struct WorkspacePanes {
    pub side: rune::Tracked<Box<panels::SideState>>,
    pub browser: rune::Tracked<Box<browser::BrowserPane>>,
    pub review: rune::Tracked<Box<review::ReviewPane>>,
    /// The file sidebar the Review and Files panels share (Desktop's `PanelSidebar`).
    pub panel_sidebar: rune::Tracked<plugin_host::native::sidebar::PanelSidebar>,
    pub files: rune::Tracked<Box<files::FilesPane>>,
    pub terminal: rune::Tracked<Box<terminal::TerminalPane>>,
    pub open_in: rune::Tracked<open_in::OpenIn>,
    /// What a tool step last opened in the side region (`tool_open`).
    pub tool_open: rune::Tracked<Option<tool_open::Opened>>,
    pub rune_mcp: rune::Tracked<rune_mcp::WindowMcp>,
}

/// Shell gestures: tab drag/preview/reopen, pinch zoom, sidebar resize, and Home return tab.
#[derive(Default)]
pub struct ShellGestures {
    /// The tab strip's gestures: a press, a drag to a new place, the width a close holds.
    pub tab_drag: rune::Tracked<rune_kit::shell::TabDragState>,
    /// The card a resting pointer shows over a tab (`view::tab_preview`).
    pub tab_preview: rune::Tracked<view::tab_preview::Preview>,
    /// Closed session tabs, newest last, with the index they had (⌘⇧T reopens; `ui.json` keeps them).
    pub closed: rune::Tracked<tabs::Closed>,
    /// When a titlebar tab last closed (ms), for ⇧⌘T (`reopensSideTab`).
    pub tab_closed_ms: rune::Tracked<Option<i64>>,
    /// A pinch's zoom gesture (`pinch.rs`).
    pub zoom_pinch: rune::Tracked<pinch::Pinch>,
    /// The width when a resize drag started.
    pub sidebar_drag: rune::Tracked<view::sidebar::Resize>,
    /// The open tab this window left for Home, independent of tab order or layout.
    pub(crate) home_return_tab: rune::Tracked<Option<String>>,
}

/// Timeline presentation, animations, disclosures, hover state, plan drawer, find, and navigation.
#[derive(Default)]
pub struct TimelineFx {
    /// The disclosures the reader toggled, tool groups springing shut among them (`disclosure`).
    pub expanded: rune::Tracked<disclosure::Disclosures>,
    /// Grow, then show's streaming words, held a line-grow behind (`line_reveal`).
    pub line_hold: rune::Tracked<line_reveal::Hold>,
    /// The copy button showing its tick (`copied.rs`).
    pub copied: rune::Tracked<copied::Copied>,
    /// The diagrams replies show, and the one open full size (`diagram`).
    pub diagram: rune::Tracked<diagram::Diagrams>,
    /// Where each session's timeline stands against its end (`jump.rs`).
    pub ends: rune::Tracked<jump::Ends>,
    /// The subagent call whose row is under the pointer, its chevron slid in (`subagent_card`).
    pub agent_hover: rune::Tracked<Option<String>>,
    /// When each run started, for the Working label's rhythm (`working.rs`).
    pub working: rune::Tracked<working::Working>,
    /// Subagents' latest steps and how their runs ended, for their timeline rows.
    pub agent_rows: rune::Tracked<agent_rows::Live>,
    /// The plan's list over the todo pill is open (`todos.rs`).
    pub todo_open: rune::Tracked<bool>,
    pub todo_ticks: rune::Tracked<todos::Ticks>,
    pub find: rune::Tracked<Box<find::Find>>,
    pub message_nav: rune::Tracked<message_nav::MessageNav>,
}

/// Theme, motion, text rendering, background, locale, project icons, and window frame state.
#[derive(Default)]
pub struct AppearanceState {
    pub reduced_motion: rune::Tracked<bool>,
    /// Where `reduced_motion` comes from (`appearance.rs`).
    pub motion: rune::Tracked<appearance::Motion>,
    /// The mode Appearance › Text rendering draws with (`text_rendering.rs`).
    pub text_rendering: rune::Tracked<text_rendering::Rendering>,
    pub(crate) background: rune::Tracked<background::Background>,
    /// The locale this window's view last built in (`language`).
    pub language: rune::Tracked<language::Applied>,
    /// Whether AppKit is saving the window frame yet (`rune_mac::remember_frame`).
    pub(crate) framed: rune::Tracked<bool>,
    pub icons: rune::Tracked<project_icon::Icons>,
}

/// Composer auxiliary inputs: history, attachments, quotes, comments, pickers, mentions, slash,
/// suggestions, voice dictation, and toast undo stacks.
#[derive(Default)]
pub struct ComposerExt {
    /// Sent prompts, for ↑ and ↓ in the composer (`prompt_history.rs`).
    pub prompts: rune::Tracked<prompt_history::PromptHistory>,
    /// The composer's images, and sent prompts waiting on the server's copy.
    pub attach: rune::Tracked<Box<attachments::Attach>>,
    /// The Quote button over a timeline selection, and the quotes waiting in the composer.
    pub quote: rune::Tracked<Box<quote::Quotes>>,
    /// Comments on lines of files, from Files and Changes (`line_comments`).
    pub line_comments: rune::Tracked<Box<line_comments::LineComments>>,
    pub pickers: rune::Tracked<Box<pickers::Pickers>>,
    /// The composer's `@` popover and what it lists.
    pub mentions: rune::Tracked<Box<mentions::Mentions>>,
    /// Session ids named in replies, looked up once (`mentions::refs`).
    pub session_refs: rune::Tracked<mentions::refs::Lookups>,
    /// The + menu, the `/` command popover and shell mode.
    pub add: rune::Tracked<add_menu::AddMenu>,
    /// The composer's suggestion lists and prompt recall (`suggestions/machine.rs`).
    pub suggest: rune::Tracked<suggestions::Suggest>,
    /// Hex dictation into the composer (⇧⌘M or the mic).
    pub dictation: rune::Tracked<dictation::Dictation>,
    /// What the window's toasts offer back (`undo`).
    pub undos: rune::Tracked<undo::Undos>,
}

/// Prompt queue mutations, drag/rewrite runs, held sends, and optimistic session starts.
#[derive(Default)]
pub struct QueueState {
    /// Each session's queue changes this window asked for (`queue::machine`).
    pub ops: rune::Tracked<HashMap<String, queue::machine::Queue>>,
    /// A queued prompt dragged to a new place (`queue::drag`).
    pub drag: rune::Tracked<queue::drag::Drag>,
    /// The queue rewrites in flight and how far each got (`queue::run`).
    pub runs: rune::Tracked<queue::run::Runs>,
    /// A send held while a session opens (`submit.rs`).
    pub submit: rune::Tracked<submit::Submit>,
    pub(crate) sends: rune::Tracked<u32>,
    /// Sessions shown before the server has them (`new_session.rs`).
    pub starts: rune::Tracked<new_session::Starts>,
}

/// Platform and connectivity services: notifications, deep links, updater, reconnect links, and
/// session availability states.
#[derive(Default)]
pub struct PlatformServices {
    /// System notifications for finished runs, permission requests and errors.
    pub notify: rune::Tracked<notifications::Notifier>,
    /// Links the system handed the app, until they open (`deep_links`).
    pub deep_links: rune::Tracked<deep_links::DeepLinks>,
    pub updater: rune::Tracked<updater::Updater>,
    /// Every server's link: live, in doubt or down (`reconnect`).
    pub server_links: rune::Tracked<reconnect::Links>,
    /// Whether each open session can show, by (server, session) (`session_state`).
    pub session_states: rune::Tracked<session_state::States>,
}
