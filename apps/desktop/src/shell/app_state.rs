//! Cohesive domain state clusters embedded on [`crate::App`].
//!
//! Every cluster is a plain non-`Tracked` struct whose individual fields retain their own
//! [`rune::Tracked`] cells, preserving exact per-field reactive tracking granularity while
//! keeping `struct App` compact and navigable.

use std::collections::VecDeque;

use serde_json::Value;

use crate::{
    appearance, attachments, background, browser, copied, deep_links, demo, diagnostics, diagram,
    disclosure, glitch, jump, language, line_reveal, motion_log, new_session, open_story, pickers,
    pinch, plugin_host, reconnect, served, session_state, stories, submit, tabs, text_rendering,
    tool_open, undo, view, working,
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

/// Host-owned workspace panes and plugin panel sidebar.
#[derive(Default)]
pub struct WorkspacePanes {
    pub browser: rune::Tracked<Box<browser::BrowserPane>>,
    /// The file sidebar the Review and Files panels share (Desktop's `PanelSidebar`).
    pub panel_sidebar: rune::Tracked<plugin_host::native::sidebar::PanelSidebar>,
    /// What a tool step last opened in the side region (`tool_open`).
    pub tool_open: rune::Tracked<Option<tool_open::Opened>>,
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

/// Timeline presentation, animations, disclosures, hover state, and working indicator.
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
    /// This window's display math: its wake when an equation finishes, and its say in when cached
    /// bitmaps go (`rune_kit::math::Client`).
    pub math: rune_kit::math::Client,
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
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) framed: rune::Tracked<bool>,
    pub icons: rune::Tracked<project_icon::Icons>,
}

/// Composer per-server attachments/pickers (`servers::runtime::Half` swap) and toast undo stacks.
#[derive(Default)]
pub struct ComposerExt {
    /// The composer's images, and sent prompts waiting on the server's copy.
    pub attach: rune::Tracked<Box<attachments::Attach>>,
    pub pickers: rune::Tracked<Box<pickers::Pickers>>,
    /// What the window's toasts offer back (`undo`).
    pub undos: rune::Tracked<undo::Undos>,
}

/// Held sends, fixture send counter, and optimistic session starts.
#[derive(Default)]
pub struct QueueState {
    /// A send held while a session opens (`submit.rs`).
    pub submit: rune::Tracked<submit::Submit>,
    pub(crate) sends: rune::Tracked<u32>,
    /// Sessions shown before the server has them (`new_session.rs`).
    pub starts: rune::Tracked<new_session::Starts>,
}

/// Connectivity and deep-link services: deep links, reconnect links, and session availability.
#[derive(Default)]
pub struct PlatformServices {
    /// Links the system handed the app, until they open (`deep_links`).
    pub deep_links: rune::Tracked<deep_links::DeepLinks>,
    /// Every server's link: live, in doubt or down (`reconnect`).
    pub server_links: rune::Tracked<reconnect::Links>,
    /// Whether each open session can show, by (server, session) (`session_state`).
    pub session_states: rune::Tracked<session_state::States>,
}
