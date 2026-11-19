//! Desktop host adapter for `opencode-feature-review` (`features/review`).

pub use opencode_feature_review::*;

mod stories;

use opencode_code_view::ExpandMode;
use opencode_kernel::{Feature, KernelBuilder};
use opencode_kit::client::Server;
use opencode_kit::wire_error::WireError;
use rune::time::Instant;
use rune::Tx;
use rune_kit::diff::DiffMode;
use serde_json::Value;

use crate::jobs::Apply;
use crate::{App, Route, Source};

impl ReviewHost for App {
    fn review(&self) -> &ReviewPane {
        &self.workspace_panes.review
    }

    fn review_mut(&mut self) -> &mut ReviewPane {
        &mut self.workspace_panes.review
    }

    fn session_directory(&self, sid: &str) -> Option<&str> {
        self.session(sid).map(|s| s.directory.as_str())
    }

    fn session_project_root(&self, sid: &str) -> Option<&str> {
        let info = self.session(sid)?;
        Some(self.project_for_session(info).map_or(info.directory.as_str(), |p| p.directory.as_str()))
    }

    fn live_server(&self) -> Option<&Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server),
            Source::Fixture => None,
        }
    }

    fn fixture_root(&self) -> Option<&Value> {
        Some(&self.fixture_run.fixture)
    }

    fn review_shown(&self) -> bool {
        App::review_shown(self)
    }

    fn should_refresh_on_poll(&self) -> bool {
        App::review_shown(self) || self.fixture_run.stage.is_some()
    }

    fn wrap_lines(&self) -> bool {
        self.settings.mobile_diff_wrap
    }

    fn server_label(&self) -> String {
        crate::servers::runtime::label(crate::servers::runtime::active(self))
    }

    fn format_wire_error(&self, what: &str, error: &WireError) -> String {
        let server = self.server_label();
        crate::errors::UserError::from_wire(what, &server, error).description()
    }

    fn show_review_tab(&mut self, sid: &str, focus_panel: bool, tx: &mut Tx) {
        let saved_focus = (self.composer.focused, self.workspace_panes.side.focus.mode());
        self.show_pane_tab(tx, |s| s.show_side_tab(sid, REVIEW_KEY, None));
        if focus_panel {
            self.workspace_panes.side.focus.focus();
            self.workspace_panes.side.active_pane_focus = crate::panels::tabs::session_tab_pane(
                &self.session_side_mut(sid).tab_state.tabs,
                REVIEW_KEY,
            );
            self.composer.focused = false;
        } else {
            self.composer.focused = saved_focus.0;
            self.workspace_panes.side.focus.restore(saved_focus.1);
        }
    }

    fn open_file_in_files_panel(&mut self, sid: &str, path: &str, tx: &mut Tx) {
        self.open_link(sid, None, path, tx);
    }

    fn spawn_job(&mut self, job: ReviewJob) {
        self.job(move |srv| {
            let reply = execute_job(srv, job);
            Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply
        });
    }

    fn trace_review_asked(&self, sid: &str, revision: u64) {
        crate::files::trace::review_asked(sid, revision);
    }

    fn trace_review_landed(&self, sid: &str, revision: u64, swap: Instant, files: usize) {
        crate::files::trace::review_landed(sid, revision, swap, files);
    }
}

impl App {
    /// Opens the review tab, and the side region with it (Desktop's `show`): added to the
    /// strip when it isn't there, selected, its changes read.
    pub fn show_review(&mut self, sid: &str, tx: &mut Tx) {
        show(self, sid, tx);
    }

    /// The changes show somewhere: the side region open, or a narrow window's Changes view.
    pub fn review_shown(&self) -> bool {
        self.side_open() || (self.narrow() && self.narrow_view().starts_with("review"))
    }

    pub fn is_review_active(&self) -> bool {
        if !(self.side_open() || self.fixture_run.stage.is_some()) {
            return false;
        }
        self.resolve_side_region().and_then(|r| r.active.clone()).as_deref() == Some(REVIEW_KEY)
    }

    pub fn note_review_event(&mut self, ty: &str, data: &Value) -> bool {
        note_event(self, ty, data)
    }

    pub fn ensure_session_review(&mut self, session_id: &str) {
        ensure_session(self, session_id);
    }

    /// Compares `mode` in the routed session's review.
    pub fn set_review_change_mode(&mut self, mode: ChangeMode) {
        set_mode(self, mode);
    }

    pub fn set_session_change_mode(&mut self, sid: &str, mode: ChangeMode) {
        set_session_mode(self, sid, mode);
    }

    pub fn set_review_diff_mode(&mut self, mode: DiffMode) {
        set_diff_mode(self, mode);
    }

    pub fn set_review_expand_mode(&mut self, mode: ExpandMode) {
        set_expand_mode(self, mode);
    }

    /// Loads the whole text of file `f` of the active session's review, which a fold waits on
    /// (`FoldEffect::FetchText`); without a server to ask, the fold gives up at once.
    pub fn request_review_file_text(&mut self, f: usize) {
        request_file_text(self, f);
    }

    pub fn route_supports_review(&self) -> bool {
        matches!(*self.route, Route::Session { .. })
    }

    /// Opens the review of `sid` (the MCP `open_diff` tool, a tool step's change), in `mode` and
    /// `view` when given, at `path` near `line`.
    pub fn open_review_for_session(
        &mut self,
        sid: &str,
        mode: Option<ChangeMode>,
        view: Option<DiffMode>,
        path: Option<&str>,
        line: Option<u32>,
        tx: &mut Tx,
    ) {
        open_for_session(self, sid, mode, view, path, line, tx);
    }

    pub(crate) fn expand_review_dir(&mut self, sid: &str, path: &str, open: bool) {
        expand_folder(self, sid, path, open);
    }
}

/// Kernel feature registration for the Review changes engine.
pub struct ReviewFeature;

impl Feature<App> for ReviewFeature {
    fn id(&self) -> &'static str {
        "feature.review"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_setup("review.waker", |s, w| s.workspace_panes.review.set_waker(w));
    }
}
