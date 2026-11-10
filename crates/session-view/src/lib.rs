//! Shared OpenCode session timeline (`p255`).
//!
//! Renders an [`opencode_kit::model::Transcript`] in a virtualized, bottom-following timeline with
//! user bubbles (long-prompt folding, `@` mention badges, quote cards, images, hover copy),
//! assistant turns (Markdown, `genui`, Mermaid diagrams, reasoning, folded tool groups, subagent
//! cards, question rows, inline images, turn errors/retries, compactions, notices, streamed line
//! reveal, Working/Copy/Interrupted tail), linear bottom fade, two-layer live overlay scroll bar,
//! and "Jump to latest", with no OpenCode (Rune)-only shell or side-panel assumptions inside.

pub mod agent_start;
pub mod arrivals;
pub mod compaction;
pub mod composer;
pub mod copied;
pub mod detail;
pub mod diagram;
pub mod disclosure;
pub mod dock;
pub mod fixture;
pub mod fold;
pub mod follow;
pub mod genui;
pub mod images;
pub mod jump;
pub mod keymap;
pub mod knobs;
pub mod line_reveal;
pub mod odometer;
pub mod palette;
pub mod pickers;
pub mod popover;
pub mod question_row;
pub mod session_chips;
pub mod sheet;
pub mod sources_chip;
pub mod story;
pub mod style;
pub mod subagent_card;
pub mod themes;
pub mod timeline;
pub mod title;
pub mod tools;
pub mod turn_diff;
pub mod turn_errors;
pub mod turns;
pub mod usage;
pub mod user_parts;
pub mod working;

#[cfg(test)]
mod motion_tests;
#[cfg(test)]
mod reveal_tests;

use std::ops::Range;

use opencode_kit::client::Request;
use opencode_kit::model::{Inbox, InboxItem, ModelRef, ToolStatus, Transcript};
use rune::{Cx, El, ScrollPos, SelectionChange, TextHighlight, Tx, Waker};
use rune_knobs::Knobs;

/// Bundled per-window state for a host that embeds a session timeline.
#[derive(Default)]
pub struct ViewState {
    pub expanded: disclosure::Disclosures,
    pub copied: copied::Copied,
    pub working: working::Working,
    pub compactions: compaction::Compactions,
    pub diagram: diagram::Diagrams,
    pub media: images::MediaStore,
    pub line_hold: line_reveal::Hold,
    pub ends: jump::Ends,
    pub composer: composer::ComposerState,
}

impl ViewState {
    /// Connects the background waker for media decoding.
    pub fn setup(&mut self, waker: Waker, _headless: bool) {
        self.media.set_waker(waker);
    }
}

/// Implements [`TimelineHost`]'s 10 state-accessor pairs by delegating to a [`ViewState`] field.
#[macro_export]
macro_rules! delegate_view_state {
    ($field:ident) => {
        fn expanded(&self) -> &$crate::disclosure::Disclosures { &self.$field.expanded }
        fn expanded_mut(&mut self) -> &mut $crate::disclosure::Disclosures { &mut self.$field.expanded }
        fn copied_state(&self) -> &$crate::copied::Copied { &self.$field.copied }
        fn copied_state_mut(&mut self) -> &mut $crate::copied::Copied { &mut self.$field.copied }
        fn working_state(&self) -> &$crate::working::Working { &self.$field.working }
        fn working_state_mut(&mut self) -> &mut $crate::working::Working { &mut self.$field.working }
        fn compactions_state(&self) -> &$crate::compaction::Compactions { &self.$field.compactions }
        fn compactions_state_mut(&mut self) -> &mut $crate::compaction::Compactions { &mut self.$field.compactions }
        fn diagrams(&self) -> &$crate::diagram::Diagrams { &self.$field.diagram }
        fn diagrams_mut(&mut self) -> &mut $crate::diagram::Diagrams { &mut self.$field.diagram }
        fn media_store(&self) -> &$crate::images::MediaStore { &self.$field.media }
        fn media_store_mut(&mut self) -> &mut $crate::images::MediaStore { &mut self.$field.media }
        fn line_hold(&self) -> &$crate::line_reveal::Hold { &self.$field.line_hold }
        fn line_hold_mut(&mut self) -> &mut $crate::line_reveal::Hold { &mut self.$field.line_hold }
        fn ends(&self) -> &$crate::jump::Ends { &self.$field.ends }
        fn ends_mut(&mut self) -> &mut $crate::jump::Ends { &mut self.$field.ends }
    };
}

/// Host state and callbacks required by interactive timeline parts and overlays.
pub trait TimelineHost: Sized + 'static {
    fn knobs(&self) -> &Knobs;
    fn clock(&self) -> f64;
    fn now_ms(&self) -> i64 {
        0
    }
    fn reduced_motion(&self) -> bool {
        false
    }
    fn zoom(&self) -> f32 {
        1.0
    }

    fn active_session_id(&self) -> Option<&str>;
    fn transcript(&self, _session: &str) -> Option<&Transcript> {
        None
    }
    fn with_transcript<R>(&self, session: &str, f: impl FnOnce(&Transcript) -> R) -> Option<R> {
        self.transcript(session).map(f)
    }
    fn session_directory(&self, _session: &str) -> Option<&str> {
        None
    }
    fn session_title(&self, _session: &str) -> Option<String> {
        None
    }
    /// Hosts with a local outbox can exclude undelivered bubbles from run status.
    fn timeline_busy(&self, transcript: &opencode_kit::model::Transcript) -> bool { transcript.busy() }

    fn is_session_live(&self, session: &str) -> bool {
        self.with_transcript(session, Transcript::busy).unwrap_or(false)
    }
    fn is_session_stopped(&self, _session: &str) -> bool {
        false
    }
    /// Whether session `session` itself waits on a question or a permission, which hides its
    /// Working (`message-timeline.tsx:339`).
    fn session_asking(&self, _session: &str) -> bool {
        false
    }
    fn session_revert(&self, _session: &str) -> Option<&str> {
        None
    }
    fn for_each_tracked_session(&self, mut f: impl FnMut(&str)) {
        if let Some(id) = self.active_session_id() {
            f(id);
        }
    }

    /// Whether a plugin renders fenced code blocks of `language` (Desktop's `CodeBlock`): a
    /// reply's ```` ```ui ```` outlines draw as parts only while one does.
    fn code_block_lang(&self, language: &str) -> bool {
        language == "ui"
    }
    /// The plugin's view of one such block, `code` its source between the fences, `key` its
    /// place in the reply; `native` is the host's own view of it (an outline), which the plugin
    /// places (`ui.genUi`), and `version` changes whenever `native` would. None keeps the
    /// block as code.
    #[allow(clippy::too_many_arguments)]
    fn code_block(&self, _p: &palette::Pal, _language: &str, _key: &str, _code: &str, _streaming: bool, _version: u64, native: El) -> Option<El> {
        Some(native)
    }

    fn expanded(&self) -> &disclosure::Disclosures;
    fn expanded_mut(&mut self) -> &mut disclosure::Disclosures;

    fn copied_state(&self) -> &copied::Copied;
    fn copied_state_mut(&mut self) -> &mut copied::Copied;

    fn working_state(&self) -> &working::Working;
    fn working_state_mut(&mut self) -> &mut working::Working;

    fn compactions_state(&self) -> &compaction::Compactions;
    fn compactions_state_mut(&mut self) -> &mut compaction::Compactions;

    fn diagrams(&self) -> &diagram::Diagrams;
    fn diagrams_mut(&mut self) -> &mut diagram::Diagrams;

    fn media_store(&self) -> &images::MediaStore;
    fn media_store_mut(&mut self) -> &mut images::MediaStore;

    fn line_hold(&self) -> &line_reveal::Hold;
    fn line_hold_mut(&mut self) -> &mut line_reveal::Hold;

    fn ends(&self) -> &jump::Ends;
    fn ends_mut(&mut self) -> &mut jump::Ends;

    fn fold_user_messages(&self) -> bool {
        true
    }
    fn open_link(&mut self, _url: &str, _tx: &mut Tx) {}
    /// A right-click in the timeline on `at`: a message, an image, or an image in a message.
    /// By default an image gets its own items (`images::menu`) and a message nothing.
    fn context_menu(&mut self, at: images::menu::MenuAt, press: rune::PointerPress, tx: &mut Tx) {
        images::menu::open_default(self, at, press, tx);
    }
    /// Opens the file at `path` in its default app (an image's Open in Preview).
    fn open_file(&mut self, path: std::path::PathBuf) {
        images::menu::open_with_system(&path);
    }
    /// Shows the file at `path` selected in the file manager (Desktop's `revealPath`); false
    /// when it couldn't.
    fn reveal_file(&mut self, path: &std::path::Path) -> bool {
        images::menu::reveal_with_system(path)
    }
    /// The output the host read from shell shell while its console shows (`tools::shell_follow`),
    /// which the console shows in place of its call's.
    fn shell_output(&self, _shell: &str) -> Option<&str> {
        None
    }
    /// Changes whenever any [`TimelineHost::shell_output`] does, so the rows showing them build again.
    fn shell_output_version(&self) -> u64 {
        0
    }
    fn reply_link_help(_url: &str) -> Option<String> {
        None
    }
    /// A reply's Markdown as the host wants it drawn, e.g. its own ids made links; `None` draws
    /// it as written. Find highlights index the text as written, so a host that rewrites
    /// shouldn't also use find.
    fn rewrite_markdown(_src: &str) -> Option<String> {
        None
    }
    /// Extra hover buttons for a finished code block in `language` holding `source`, drawn left
    /// of its Copy button (a host's plugins' code block actions). Markdown draws code blocks
    /// without the host's state, so these come from what the host registered up front.
    fn code_block_actions(_p: &palette::Pal, _language: Option<&str>, _source: &str) -> Vec<El> {
        Vec::new()
    }
    fn send_request(&self, _req: Request) {}
    fn can_switch_model(&self) -> bool {
        false
    }
    fn switch_model(&mut self, _tx: &mut Tx) {}
    fn model_name(&self, model: &ModelRef) -> String {
        if model.id.is_empty() {
            model.provider.clone()
        } else {
            model.id.clone()
        }
    }
    /// A button beside call `call`'s row in session `session`: it opens what the call touched in
    /// the host's side panel (a read file at its lines); `None` for none. A press on the row
    /// itself opens the call inline. Rest its glyph hidden with `group_opacity(0.0, 1.0)`, so it
    /// shows while the tool is hovered and stays pressable. An edit card's file rows take none,
    /// as Desktop's `accordion-trigger` has none.
    fn tool_row_action(&self, _p: &palette::Pal, _session: &str, _call: &tools::Call) -> Option<El> {
        None
    }
    /// Included in a tool row's memo key: what [`Self::tool_row_action`] shows for call `id` can
    /// change without the call changing.
    fn tool_row_action_version(&self, _id: &str) -> u64 {
        0
    }

    fn can_open_subagent(&self) -> bool {
        false
    }
    fn resolve_subagent<'a>(&'a self, _parent: &str, session: Option<&'a str>, _title: &str) -> Option<&'a str> {
        session
    }
    fn subagent_run(&self, _parent: &str, _call: &str, _child: Option<&str>, status: &ToolStatus) -> subagent_card::Run {
        subagent_card::Run::from_status(status)
    }
    fn prefetch_subagent(&self, _child: &str) {}
    fn open_subagent(&mut self, _parent: &str, _child: &str, _title: &str, _agent: &str, _tx: &mut Tx) {}
    /// Whether a session mention's badge in a prompt opens that session ([`open_session`]).
    ///
    /// [`open_session`]: TimelineHost::open_session
    fn can_open_session(&self) -> bool {
        false
    }
    fn open_session(&mut self, _id: &str, _tx: &mut Tx) {}
    /// What session `id`, named in a reply or a prompt, stands for: `None` while the host doesn't
    /// know it, and for good when the server doesn't either, so it stays text. A host that
    /// looks one up remembers asking, as this runs on every build of the row.
    fn session_ref(&self, _id: &str) -> Option<session_chips::SessionRef> {
        None
    }
    /// A right-click on the chip of session `id` (`session_chips`): its menu at `press`.
    fn session_chip_menu(&mut self, _id: &str, _press: rune::PointerPress, _tx: &mut Tx) {}
    /// Whether [`reveal_shell`] can take the timeline to a shell's call.
    ///
    /// [`reveal_shell`]: TimelineHost::reveal_shell
    fn can_reveal_shell(&self) -> bool {
        false
    }
    /// Scrolls the open session's timeline to the call that ran shell `shell` (its
    /// `metadata.shellID`), as Desktop's `revealPart` does for a finished shell's notice.
    fn reveal_shell(&mut self, _shell: &str, _tx: &mut Tx) {}

    fn loading_error(&self, _session: &str) -> Option<&str> {
        None
    }
    fn loading_older(&self, _session: &str) -> bool {
        false
    }
    fn more_above(&self, _session: &str) -> bool {
        false
    }
    #[allow(clippy::type_complexity)]
    fn watch_scroll_top(&self, _session: &str) -> Option<Box<dyn Fn(ScrollPos)>> {
        None
    }
    fn on_timeline_selection_change(_s: &mut Self, _change: &SelectionChange, _tx: &mut Tx) {}

    fn find_row_version(&self, _row_key: &str) -> u64 {
        0
    }
    fn find_text_highlights(&self, _p: &palette::Pal, _row_key: &str) -> Vec<TextHighlight> {
        Vec::new()
    }
    fn find_markdown_highlights(&self, _p: &palette::Pal, _row_key: &str) -> Vec<(&[Range<usize>], rune_kit::Highlight)> {
        Vec::new()
    }
    #[allow(clippy::type_complexity)]
    fn find_row_hits(&self, _row_key: &str) -> Option<(&[Vec<Range<usize>>], Option<usize>)> {
        None
    }

    fn user_glow_at(&self, _id: &str) -> Option<f64> {
        None
    }
    fn user_menu_open_on(&self, _id: &str) -> bool {
        false
    }
    fn quote_more_open(&self, id: &str) -> bool {
        self.expanded().contains(&format!("quote-more:{id}"))
    }
    fn open_quote_more(&mut self, id: String) {
        self.expanded_mut().insert(format!("quote-more:{id}"));
    }
    /// The button in a prompt's top trailing corner (`timeline::user_menu_button`), which opens
    /// its actions; `pending` for a steer the server hasn't delivered. `None` for none.
    fn user_menu(&self, p: &palette::Pal, _session: &str, id: &str, body: &str, _pending: bool) -> Option<El> {
        timeline::default_user_menu(self, p, id, body)
    }
    /// Delivery status belongs below its own bubble, even inside a run of user messages.
    fn user_status(&self, _p: &palette::Pal, _id: &str) -> Option<El> { None }
    /// Included in the user-row memo key; a receipt can change without changing the message.
    fn user_status_version(&self, _id: &str) -> u64 { 0 }

    fn inbox_for(&self, _session: &str) -> Option<&Inbox> {
        None
    }
    fn queued_compaction(&self, _session: &str) -> Option<&str> {
        None
    }
    fn pending_steer_label(&self, item: &InboxItem) -> String {
        item.text.clone()
    }
    fn turn_tail_extra(&self, _p: &palette::Pal, _session: &str, _busy: bool) -> Option<El> {
        None
    }

    /// Settings › Preferences › Timeline detail: what of a turn's work shows, and how.
    fn timeline_detail(&self) -> detail::Detail {
        detail::Detail::default()
    }
    /// Settings › Preferences › Changed files: a finished turn lists the files it changed.
    fn turn_diffs(&self) -> bool {
        false
    }
    /// The diff of session `session`'s run from prompt `from`, for Changed files; a host loads
    /// it when first asked.
    fn turn_diff(&self, _session: &str, _from: &str) -> turn_diff::State<'_> {
        turn_diff::State::Failed
    }
    /// Loads that diff again after a failure.
    fn retry_turn_diff(&mut self, _session: &str, _from: &str) {}
    /// The key of the pane the timeline scrolls in, header and composer included: Desktop's
    /// timeline viewport runs under both, and Jump to latest shows past its height from the end.
    /// Rune measures it as the timeline scrolls, so no view reads it. None: the timeline's own.
    fn timeline_frame(&self) -> Option<&'static str> {
        None
    }
    /// Settings › Preferences › Background activity: where the pills over the composer sit.
    fn lane_align(&self) -> jump::Align {
        jump::Align::Start
    }
    /// Settings › Preferences › Timeline width: the column the timeline draws at.
    fn content_width(&self) -> f32 {
        style::CONTENT_W
    }
}

/// Full timeline region: virtualized message stack, linear bottom fade, live overlay scroll bar,
/// and floating `pills` + "Jump to latest" button.
pub fn timeline_area<S: TimelineHost>(s: &S, p: &palette::Pal, id: &str, t: Option<&Transcript>, pills: Vec<El>) -> El {
    style::set_content_w(s.content_width());
    let lane = jump::lane(!pills.is_empty());
    jump::over(s, p, id, timeline::timeline(s, p, id, t, lane), pills)
}

/// Read-only session timeline region (`timeline_area` with no composer activity pills).
pub fn read_only<S: TimelineHost>(s: &S, p: &palette::Pal, id: &str, t: Option<&Transcript>) -> El {
    diagram::prepare(s);
    timeline_area(s, p, id, t, Vec::new())
}

/// Modal overlays opened from timeline rows (full-size image viewer, Mermaid diagram lightbox and
/// Execute's View code dialog).
pub fn overlays<S: TimelineHost>(s: &S, p: &palette::Pal, cx: &mut Cx<S>) -> Vec<El> {
    let mut out = Vec::new();
    out.extend(images::viewer(s, p, cx));
    out.extend(diagram::view(s, p, cx));
    out.extend(tools::details_overlay(s, p, cx));
    out
}

/// Advances all shared timeline animations and timers for one frame.
pub fn poll<S: TimelineHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    changed |= turn_errors::poll(s, tx);
    changed |= turns::poll(s, tx);
    changed |= working::sync(s);
    changed |= compaction::sync(s);
    changed |= disclosure::poll(s, now, tx);
    changed |= line_reveal::poll(s, now, tx);
    changed |= copied::poll(s, now, tx);
    changed |= diagram::poll(s, now, tx);
    changed |= images::poll(s, now, tx);
    changed
}

/// All ⌘⇧K knob groups owned by `session-view`.
pub const KNOB_GROUPS: &[rune_knobs::Group] = &[
    knobs::TIMELINE_GROUP,
    disclosure::GROUP,
    knobs::WORKING,
    compaction::GROUP,
    copied::GROUP,
];
