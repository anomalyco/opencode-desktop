//! The changes engine behind Desktop's review plugin (`packages/gui-plugins/src/review/`), which
//! now runs as Luau (`plugins/review`): each session's diffs for the current source (`git`,
//! `branch` or `turn`) read from the server, parsed off the UI thread into the diff view's state,
//! and the file the preview shows. The plugin reaches it through the `Diff` service and draws it
//! with `ui.diffView` and `ui.fileTree` (`plugin_host::luau::native`).

pub mod chrome;
pub mod data;
pub mod mobile;
mod stories;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use rune::{Latest, Tx, Waker};
use rune_kit::diff::DiffMode;
use rune_kit::{FileNode, TreeModel};

use crate::code_view::{DiffDocState, ExpandMode, FoldEvent};
use crate::{App, Route, Source};
use data::{FetchedDiffs, ReviewFileMeta, build_change_tree, collapsed_folders, fetch_live_diffs, fetch_live_file_text, load_fixture_diffs, parse_diff_list};

/// The review panel's one tab (Desktop's `review:changes`).
pub const REVIEW_KEY: &str = "review:changes";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChangeMode {
    #[default]
    Git,
    Branch,
    Turn,
}

impl ChangeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Git => opencode_i18n::t!("ui.sessionReview.title.git"),
            Self::Branch => opencode_i18n::t!("ui.sessionReview.title.branch"),
            Self::Turn => opencode_i18n::t!("ui.sessionReview.title.lastTurn"),
        }
    }

    pub fn empty_label(self) -> &'static str {
        match self {
            Self::Git => opencode_i18n::t!("plugin.review.empty.git"),
            Self::Branch => opencode_i18n::t!("plugin.review.empty.branch"),
            Self::Turn => opencode_i18n::t!("plugin.review.noChanges"),
        }
    }

    /// The contract's name for it (`DiffSource`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Branch => "branch",
            Self::Turn => "turn",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "git" => Some(Self::Git),
            "branch" => Some(Self::Branch),
            "turn" => Some(Self::Turn),
            _ => None,
        }
    }
}

pub struct SessionReview {
    pub mode: ChangeMode,
    pub diffs: DiffDocState,
    pub meta: Vec<ReviewFileMeta>,
    /// The changed files as the server listed them, newest change first.
    pub listed: Vec<String>,
    pub tree: TreeModel<FileNode<ReviewFileMeta>>,
    /// The file the preview shows: the one picked, else the server's first.
    pub active_path: Option<String>,
    pub ready: bool,
    pub no_git: bool,
    pub branch_current: Option<String>,
    pub branch_default: Option<String>,
    pub loaded_revision: Option<(ChangeMode, u64)>,
    /// A tool step's change to scroll to once the diffs load (`tool_open`).
    pub pending_focus: Option<(String, Option<crate::tool_open::Anchor>)>,
    /// The narrow window's Changes view: its open files (`mobile`).
    pub narrow: mobile::NarrowReview,
    /// Moves with every change a reader can see: the plugin's renders read it.
    pub stamp: u64,
}

impl Default for SessionReview {
    fn default() -> Self {
        Self {
            mode: ChangeMode::Git,
            diffs: DiffDocState::default(),
            meta: Vec::new(),
            listed: Vec::new(),
            tree: TreeModel::new().key("review-tree"),
            active_path: None,
            ready: false,
            no_git: false,
            branch_current: None,
            branch_default: None,
            loaded_revision: None,
            pending_focus: None,
            narrow: mobile::NarrowReview::default(),
            stamp: 0,
        }
    }
}

impl SessionReview {
    pub fn available_modes(&self) -> Vec<ChangeMode> {
        let mut out = vec![ChangeMode::Git];
        if let (Some(cur), Some(def)) = (&self.branch_current, &self.branch_default)
            && !cur.is_empty()
            && !def.is_empty()
            && cur != def
        {
            out.push(ChangeMode::Branch);
        }
        out.push(ChangeMode::Turn);
        out
    }

    pub fn totals(&self) -> (u32, u32) {
        self.meta.iter().fold((0, 0), |(a, d), m| (a + m.additions, d + m.deletions))
    }

    /// Shows `path` in the preview, alone (Desktop's `SessionReviewFilePreviewV2`), and selects it
    /// in the tree.
    pub fn show(&mut self, path: &str) {
        if self.active_path.as_deref() != Some(path) || self.diffs.only().is_none() {
            self.active_path = Some(path.to_owned());
            self.diffs.show_only(self.diffs.file_index(path));
            self.tree.select(path);
            self.stamp += 1;
        }
    }
}

pub struct ReviewPane {
    pub sessions: HashMap<String, SessionReview>,
    pub empty_diffs: DiffDocState,
    pub diff_mode: DiffMode,
    pub expand_mode: ExpandMode,
    pub dirty: bool,
    pub revision: u64,
    diff_fetch: Latest<FetchedDiffs>,
    file_fetch: Latest<(String, String, Option<String>)>,
    asked: Option<(String, ChangeMode, u64)>,
}

impl Default for ReviewPane {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            empty_diffs: DiffDocState::default(),
            // Desktop's `review.diffStyle` starts split.
            diff_mode: DiffMode::Split,
            expand_mode: ExpandMode::Collapse,
            dirty: false,
            revision: 1,
            // No debounce: opening the panel asks at once, as Desktop does, and a burst of file
            // events only supersedes the request in flight (`Latest` keeps the newest answer).
            diff_fetch: Latest::default(),
            file_fetch: Latest::default(),
            asked: None,
        }
    }
}

impl ReviewPane {
    pub fn set_waker(&mut self, waker: Waker) {
        self.diff_fetch.set_waker(waker.clone());
        self.file_fetch.set_waker(waker);
    }
}

/// Lands the reads in flight; reads again after file events while the changes show.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(fetched) = s.review.diff_fetch.take() {
        tx.instant();
        let sid = fetched.session_id.clone();
        s.apply_fetched_diffs(fetched);
        s.apply_pending_review_focus(&sid, tx);
        changed = true;
    }
    if let Some((session_id, path, content)) = s.review.file_fetch.take()
        && let Some(sr) = s.review.sessions.get_mut(&session_id)
    {
        tx.instant();
        // The ranges waiting on this file's text open as asked, or fold again without it.
        match content {
            Some(text) => {
                sr.diffs.attach_file_text(&path, &text);
            }
            None => sr.diffs.text_failed(&path),
        }
        sr.stamp += 1;
        changed = true;
    }
    if s.review.dirty {
        s.review.dirty = false;
        if let Some(sid) = s.active_session_id().map(str::to_owned)
            && (s.review_shown() || s.stage.is_some())
        {
            s.ensure_session_review(&sid);
            changed = true;
        }
    }
    changed
}

impl App {
    /// Opens the review tab, and the side region with it (Desktop's `show`): added to the
    /// strip when it isn't there, selected, its changes read.
    pub fn show_review(&mut self, sid: &str, tx: &mut Tx) {
        self.show_pane_tab(tx, |s| s.show_side_tab(sid, REVIEW_KEY, None));
        self.side.focused = true;
        self.side.active_pane_focus = crate::panels::tabs::session_tab_pane(&self.session_side_mut(sid).tab_state.tabs, REVIEW_KEY);
        self.composer.focused = false;
        self.ensure_session_review(sid);
    }

    /// The changes show somewhere: the side region open, or a narrow window's Changes view.
    pub fn review_shown(&self) -> bool {
        self.side_open() || (self.narrow() && self.narrow_view().starts_with("review"))
    }

    pub fn is_review_active(&self) -> bool {
        if !(self.side_open() || self.stage.is_some()) {
            return false;
        }
        self.resolve_side_region().and_then(|r| r.active).as_deref() == Some(REVIEW_KEY)
    }

    pub fn note_review_event(&mut self, ty: &str) -> bool {
        if matches!(ty, "filesystem.changed" | "session.execution.succeeded" | "session.step.ended" | "session.revert.committed") {
            self.review.revision += 1;
            if self.review_shown() {
                self.review.dirty = true;
                return true;
            }
        }
        false
    }

    pub fn ensure_session_review(&mut self, session_id: &str) {
        let diff_mode = self.review.diff_mode;
        let expand_mode = self.review.expand_mode;
        let rev = self.review.revision;
        let directory = self.session(session_id).map(|i| i.directory.clone()).unwrap_or_default();
        let mode = self.review.sessions.entry(session_id.to_owned()).or_default().mode;

        if self.review.sessions.get(session_id).is_some_and(|sr| sr.loaded_revision == Some((mode, rev))) {
            if let Some(sr) = self.review.sessions.get_mut(session_id) {
                sr.diffs.set_mode(diff_mode);
                sr.diffs.set_expand_mode(expand_mode);
            }
            return;
        }

        match &self.source {
            Source::Fixture => {
                let (files, meta, listed, cur_b, def_b, texts) = load_fixture_diffs(&self.fixture, session_id, mode);
                let sr = self.review.sessions.entry(session_id.to_owned()).or_default();
                sr.branch_current = cur_b;
                sr.branch_default = def_b;
                sr.land(files, meta, listed, diff_mode, expand_mode);
                for (path, text) in texts {
                    sr.diffs.attach_file_text(&path, &text);
                }
                sr.ready = true;
                sr.no_git = false;
                sr.loaded_revision = Some((mode, rev));
            }
            Source::Live { server, .. } => {
                let ask = (session_id.to_owned(), mode, rev);
                if self.review.asked.as_ref() == Some(&ask) {
                    return;
                }
                self.review.asked = Some(ask);
                let srv = server.clone();
                let sid = session_id.to_owned();
                self.review.diff_fetch.request((sid, directory, mode, rev), move |(sid, dir, mode, rev)| fetch_live_diffs(&srv, sid, dir, mode, rev));
            }
        }
    }

    fn apply_fetched_diffs(&mut self, fetched: FetchedDiffs) {
        let diff_mode = self.review.diff_mode;
        let expand_mode = self.review.expand_mode;
        let (files, meta) = parse_diff_list(&fetched.raw_diff);
        let listed = data::listed_paths(&fetched.raw_diff);
        let sr = self.review.sessions.entry(fetched.session_id.clone()).or_default();
        if sr.mode != fetched.mode {
            return;
        }
        sr.branch_current = fetched.branch_current;
        sr.branch_default = fetched.branch_default;
        sr.no_git = fetched.no_git;
        sr.land(files, meta, listed, diff_mode, expand_mode);
        sr.ready = true;
        sr.loaded_revision = Some((fetched.mode, fetched.revision));
    }

    /// Compares `mode` in the routed session's review.
    pub fn set_review_change_mode(&mut self, mode: ChangeMode) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.set_session_change_mode(&sid, mode);
    }

    pub fn set_session_change_mode(&mut self, sid: &str, mode: ChangeMode) {
        let sr = self.review.sessions.entry(sid.to_owned()).or_default();
        if sr.mode != mode {
            sr.mode = mode;
            sr.loaded_revision = None;
            sr.ready = false;
            sr.stamp += 1;
            self.review.asked = None;
        }
        self.ensure_session_review(sid);
    }

    pub fn set_review_diff_mode(&mut self, mode: DiffMode) {
        if self.review.diff_mode == mode {
            return;
        }
        self.review.diff_mode = mode;
        for sr in self.review.sessions.values_mut() {
            sr.diffs.set_mode(mode);
            sr.stamp += 1;
        }
    }

    pub fn set_review_expand_mode(&mut self, mode: ExpandMode) {
        let (next, event) = self.review.expand_mode.step(mode);
        self.review.expand_mode = next;
        let Some(event) = event else { return };
        if event == FoldEvent::Unfold
            && let Some(sid) = self.active_session_id().map(str::to_owned)
        {
            self.request_missing_review_texts(&sid);
        }
        for sr in self.review.sessions.values_mut() {
            sr.diffs.set_expand_mode(next);
            sr.stamp += 1;
        }
    }

    fn request_missing_review_texts(&mut self, session_id: &str) {
        let Source::Live { server, .. } = &self.source else { return };
        let directory = self.session(session_id).map(|i| i.directory.clone()).unwrap_or_default();
        let Some(sr) = self.review.sessions.get(session_id) else { return };
        let shown = sr.diffs.only().map_or_else(|| (0..sr.diffs.files().len()).collect::<Vec<_>>(), |f| vec![f]);
        let missing = shown.into_iter().find(|&f| !sr.diffs.has_full_text(f)).map(|f| sr.diffs.files()[f].path().to_owned());
        if let Some(path) = missing {
            let srv = server.clone();
            let sid = session_id.to_owned();
            self.review.file_fetch.request((sid, directory, path), move |(sid, dir, path)| {
                let content = fetch_live_file_text(&srv, &dir, &path);
                (sid, path, content)
            });
        }
    }

    /// Loads the whole text of file `f` of the active session's review, which a fold waits on
    /// (`FoldEffect::FetchText`); without a server to ask, the fold gives up at once.
    pub fn request_review_file_text(&mut self, f: usize) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let Some(path) = self.review.sessions.get(&sid).and_then(|sr| sr.diffs.files().get(f)).map(|fd| fd.path().to_owned()) else { return };
        let Source::Live { server, .. } = &self.source else {
            if let Some(sr) = self.review.sessions.get_mut(&sid) {
                sr.diffs.text_failed(&path);
            }
            return;
        };
        let directory = self.session(&sid).map(|i| i.directory.clone()).unwrap_or_default();
        let srv = server.clone();
        self.review.file_fetch.request((sid, directory, path), move |(sid, dir, path)| {
            let content = fetch_live_file_text(&srv, &dir, &path);
            (sid, path, content)
        });
    }

    pub fn route_supports_review(&self) -> bool {
        matches!(self.route, Route::Session(_))
    }

    /// Opens the review of `sid` (the MCP `open_diff` tool, a tool step's change), in `mode` and
    /// `view` when given, at `path` near `line`.
    pub fn open_review_for_session(&mut self, sid: &str, mode: Option<ChangeMode>, view: Option<DiffMode>, path: Option<&str>, line: Option<u32>, tx: &mut Tx) {
        let saved_focus = (self.composer.focused, self.side.focused);
        if let Some(v) = view {
            self.set_review_diff_mode(v);
        }
        if let Some(m) = mode {
            let sr = self.review.sessions.entry(sid.to_owned()).or_default();
            if sr.mode != m {
                sr.mode = m;
                sr.loaded_revision = None;
                sr.ready = false;
                sr.stamp += 1;
                self.review.asked = None;
            }
        }
        self.show_pane_tab(tx, |s| s.show_side_tab(sid, REVIEW_KEY, None));
        self.ensure_session_review(sid);
        if let Some(p) = path {
            let anchor = line.map(|n| crate::tool_open::Anchor { side: rune_kit::diff::Side::New, text: String::new(), near: Some(n.max(1)) });
            if self.review.sessions.get(sid).is_some_and(|sr| !sr.ready) {
                if let Some(sr) = self.review.sessions.get_mut(sid) {
                    sr.pending_focus = Some((p.to_owned(), anchor));
                }
            } else {
                self.focus_review_change(sid, p, anchor.as_ref(), tx);
            }
        }
        (self.composer.focused, self.side.focused) = saved_focus;
    }
}

impl SessionReview {
    /// Takes a new list of changes: the tree keeps the folders the user closed, the preview keeps
    /// its file while it still changed, else shows the server's first (Desktop's `activeFile`).
    fn land(&mut self, files: Vec<rune_kit::diff::FileDiff>, meta: Vec<ReviewFileMeta>, listed: Vec<String>, diff_mode: DiffMode, expand_mode: ExpandMode) {
        let prev = self.active_path.take();
        self.meta = meta;
        self.tree = build_change_tree(&self.meta, &collapsed_folders(&self.tree));
        self.diffs.set_mode(diff_mode);
        self.diffs.set_files(files);
        self.narrow.refresh(self.diffs.files());
        self.diffs.set_expand_mode(expand_mode);
        self.listed = listed.into_iter().filter(|p| self.meta.iter().any(|m| &m.path == p)).collect();
        let next = prev.filter(|p| self.meta.iter().any(|m| &m.path == p)).or_else(|| self.listed.first().cloned()).or_else(|| self.meta.first().map(|m| m.path.clone()));
        self.diffs.show_only(None);
        match next {
            Some(path) => self.show(&path),
            None => self.stamp += 1,
        }
        self.stamp += 1;
    }
}
