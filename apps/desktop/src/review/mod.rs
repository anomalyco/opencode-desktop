//! The changes engine behind Desktop's review plugin (`packages/gui-plugins/src/review/`), which
//! now runs as Luau (`plugins/review`): each session's diffs for the current source (`git`,
//! `branch` or `turn`) read from the server, parsed off the UI thread into the diff view's state,
//! and the file the preview shows. The plugin reaches it through the `Diff` service and draws it
//! with `ui.diffView` and `ui.fileTree` (`plugin_host::native`).

pub mod chrome;
pub mod data;
mod folders;
pub mod mobile;
mod stories;

use std::collections::HashMap;
use std::time::Duration;

use rune::time::Instant;
use rune::{Latest, Tx, Waker};
use rune_kit::diff::DiffMode;
use rune_kit::{FileNode, TreeModel};

use crate::code_view::{DiffDocState, ExpandMode, FoldEvent};
use crate::{App, Route, Source};
use data::{FetchedDiffs, ReviewFetchSpec, ReviewFileMeta, build_change_tree, collapsed_folders, fetch_live_diffs, fetch_live_file_text, load_fixture_diffs};

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

/// Where a session's changes stand, as Desktop's `gui-plugins/src/review/machine.ts`
/// (`ReviewDiffState`) shows them (`ReviewDiffView`): loading, or landed for a mode and file
/// revision, where a project without a repository has none to show (`no-vcs`). A newer revision
/// reads again while the landed changes stay shown (`refreshing`).
#[derive(Debug)]
pub enum Load {
    /// The mode's changes haven't landed; `focus` is a change to scroll to once they do.
    Loading { focus: Option<(String, Option<crate::tool_open::Anchor>)> },
    Ready { mode: ChangeMode, revision: u64 },
    NoVcs { mode: ChangeMode, revision: u64 },
    /// Beyond Desktop, which shows "No file changes yet" when the read fails: the read failed,
    /// `why` in the app's words (`crate::errors`); the changes it showed before stay.
    Failed { mode: ChangeMode, revision: u64, why: String, offline: bool },
}

impl Load {
    /// The changes landed: what's listed is what there is.
    pub fn ready(&self) -> bool {
        matches!(self, Load::Ready { .. } | Load::NoVcs { .. })
    }

    /// Why the last read failed, when it did.
    pub fn failed(&self) -> Option<&str> {
        match self {
            Load::Failed { why, .. } => Some(why),
            _ => None,
        }
    }

    /// The last read failed for want of the server.
    pub fn offline(&self) -> bool {
        matches!(self, Load::Failed { offline: true, .. })
    }

    pub fn no_git(&self) -> bool {
        matches!(self, Load::NoVcs { .. })
    }

    /// The mode and revision whose changes landed.
    pub fn loaded_revision(&self) -> Option<(ChangeMode, u64)> {
        match *self {
            Load::Loading { .. } => None,
            Load::Ready { mode, revision } | Load::NoVcs { mode, revision } | Load::Failed { mode, revision, .. } => Some((mode, revision)),
        }
    }

    /// Changes of `mode` at `revision` landed; returns the change waiting to be shown.
    fn landed(&mut self, mode: ChangeMode, revision: u64, no_git: bool) -> Option<(String, Option<crate::tool_open::Anchor>)> {
        let next = if no_git { Load::NoVcs { mode, revision } } else { Load::Ready { mode, revision } };
        match std::mem::replace(self, next) {
            Load::Loading { focus } => focus,
            Load::Ready { .. } | Load::NoVcs { .. } | Load::Failed { .. } => None,
        }
    }

    /// The read of `mode` at `revision` failed; a change waiting to be shown waits for the next.
    fn failed_with(&mut self, mode: ChangeMode, revision: u64, why: String, offline: bool) {
        *self = Load::Failed { mode, revision, why, offline };
    }

    /// Another mode: its changes load from scratch.
    fn restart(&mut self) {
        *self = Load::Loading { focus: None };
    }
}

pub struct SessionReview {
    pub mode: ChangeMode,
    pub diffs: DiffDocState,
    pub meta: Vec<ReviewFileMeta>,
    pub root_count: usize,
    generation: u64,
    folders: HashMap<String, folders::Listing>,
    /// The changed files as the server listed them, newest change first.
    pub listed: Vec<String>,
    pub tree: TreeModel<FileNode<ReviewFileMeta>>,
    /// The file the preview shows: the one picked, else the server's first.
    pub active_path: Option<String>,
    pub load: Load,
    pub branch_current: Option<String>,
    pub branch_default: Option<String>,
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
            root_count: 0,
            generation: 0,
            folders: HashMap::new(),
            listed: Vec::new(),
            tree: TreeModel::new().key("review-tree"),
            active_path: None,
            load: Load::Loading { focus: None },
            branch_current: None,
            branch_default: None,
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

/// Trailing-edge 100 ms debounce on `filesystem.changed`, matching Desktop's
/// `debounce(..., 100)` (`packages/gui-plugins/src/review/model.ts:403-420`), so a burst of
/// agent writes runs one diff read once it settles. Opening the panel, switching source,
/// pressing Retry or ending a turn step asks at once (`Due`).
const FILESYSTEM_DEBOUNCE: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Refresh {
    #[default]
    Idle,
    Due,
    Waiting(Instant),
}

pub enum RefreshEvent {
    Debounced(Instant),
    Immediate,
    Poll(Instant),
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshEffect {
    None,
    WakeNow,
    WakeAfter(Duration),
    Run,
}

impl Refresh {
    pub fn step(&mut self, event: RefreshEvent) -> RefreshEffect {
        match event {
            RefreshEvent::Clear => {
                *self = Self::Idle;
                RefreshEffect::None
            }
            RefreshEvent::Immediate => {
                *self = Self::Due;
                RefreshEffect::WakeNow
            }
            RefreshEvent::Debounced(now) => {
                let was_idle = matches!(self, Self::Idle);
                if !matches!(self, Self::Due) {
                    *self = Self::Waiting(now + FILESYSTEM_DEBOUNCE);
                }
                if was_idle { RefreshEffect::WakeNow } else { RefreshEffect::None }
            }
            RefreshEvent::Poll(now) => match *self {
                Self::Idle => RefreshEffect::None,
                Self::Due => {
                    *self = Self::Idle;
                    RefreshEffect::Run
                }
                Self::Waiting(due) if now < due => RefreshEffect::WakeAfter(due - now),
                Self::Waiting(_) => {
                    *self = Self::Idle;
                    RefreshEffect::Run
                }
            },
        }
    }
}

pub struct ReviewPane {
    pub sessions: HashMap<String, SessionReview>,
    pub empty_diffs: DiffDocState,
    pub diff_mode: DiffMode,
    pub expand_mode: ExpandMode,
    pub refresh: Refresh,
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
            refresh: Refresh::Idle,
            revision: 1,
            // Opening the panel asks at once, while `filesystem.changed` waits out
            // `FILESYSTEM_DEBOUNCE` in `Refresh` (`packages/gui-plugins/src/review/model.ts:403`).
            diff_fetch: Latest::default(),
            file_fetch: Latest::default(),
            asked: None,
        }
    }
}

impl ReviewPane {
    /// The data or the pending live request already covers this document. Merely asking
    /// for the same thing again while rendering must not enqueue a follow-up commit.
    pub(crate) fn has_read(&self, sid: &str, mode: ChangeMode) -> bool {
        self.sessions.get(sid).is_some_and(|sr| sr.mode == mode && sr.load.loaded_revision() == Some((mode, self.revision)))
            || self.asked.as_ref().is_some_and(|(id, m, rev)| id == sid && *m == mode && *rev == self.revision)
    }

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
        if let Some((path, anchor)) = s.apply_fetched_diffs(fetched) {
            s.focus_review_change(&sid, &path, anchor.as_ref(), tx);
        }
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
    match s.review.refresh.step(RefreshEvent::Poll(Instant::now())) {
        RefreshEffect::WakeAfter(delay) => tx.wake_after(delay),
        RefreshEffect::Run => {
            if let Some(sid) = s.active_session_id().map(str::to_owned)
                && (s.review_shown() || s.stage.is_some())
            {
                let live = matches!(*s.source, Source::Live { .. });
                s.ensure_session_review(&sid);
                if !live {
                    changed = true;
                }
            }
        }
        RefreshEffect::None | RefreshEffect::WakeNow => {}
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
        self.resolve_side_region().and_then(|r| r.active.clone()).as_deref() == Some(REVIEW_KEY)
    }

    pub fn note_review_event(&mut self, ty: &str, data: &serde_json::Value) -> bool {
        if !matches!(
            ty,
            "filesystem.changed"
                | "session.execution.succeeded"
                | "session.step.ended"
                | "session.revert.committed"
        ) {
            return false;
        }
        self.review.revision += 1;
        if !self.review_shown() {
            return false;
        }
        if ty == "filesystem.changed" && matches!(*self.source, Source::Live { .. }) {
            if let Some(event_dir) = data["eventLocation"]["directory"].as_str()
                && let Some(cur_dir) = self.active_session_id().and_then(|sid| self.session(sid)).map(|s| s.directory.as_str())
                && !cur_dir.is_empty()
                && !same_directory(event_dir, cur_dir)
            {
                return false;
            }
            self.review.refresh.step(RefreshEvent::Debounced(Instant::now()));
            return false;
        }
        self.review.refresh.step(RefreshEvent::Immediate) == RefreshEffect::WakeNow
    }

    pub fn ensure_session_review(&mut self, session_id: &str) {
        self.review.refresh.step(RefreshEvent::Clear);
        let diff_mode = self.review.diff_mode;
        let expand_mode = self.review.expand_mode;
        let rev = self.review.revision;
        let directory = self.session(session_id).map(|i| i.directory.clone()).unwrap_or_default();
        let mode = self.review.sessions.entry(session_id.to_owned()).or_default().mode;

        if self.review.sessions.get(session_id).is_some_and(|sr| sr.load.loaded_revision() == Some((mode, rev))) {
            if let Some(sr) = self.review.sessions.get_mut(session_id) {
                sr.diffs.set_mode(diff_mode);
                sr.diffs.set_expand_mode(expand_mode);
            }
            return;
        }

        match &*self.source {
            Source::Fixture => {
                let (files, meta, listed, cur_b, def_b, texts) = load_fixture_diffs(&self.fixture, session_id, mode);
                let sr = self.review.sessions.entry(session_id.to_owned()).or_default();
                sr.branch_current = cur_b;
                sr.branch_default = def_b;
                sr.root_count = meta.len();
                sr.land(files, meta, listed, diff_mode, expand_mode);
                for (path, text) in texts {
                    sr.diffs.attach_file_text(&path, &text);
                }
                let _ = sr.load.landed(mode, rev, false);
            }
            Source::Live { server, .. } => {
                let ask = (session_id.to_owned(), mode, rev);
                if self.review.asked.as_ref() == Some(&ask) {
                    return;
                }
                self.review.asked = Some(ask);
                let (collapsed, active_path, find_query, narrow_open) = self
                    .review
                    .sessions
                    .get(session_id)
                    .map(|sr| {
                        (
                            collapsed_folders(&sr.tree),
                            sr.active_path.clone(),
                            sr.diffs.find_query(),
                            sr.narrow.open_paths(),
                        )
                    })
                    .unwrap_or_default();
                let spec = ReviewFetchSpec {
                    session_id: session_id.to_owned(),
                    directory,
                    mode,
                    revision: rev,
                    diff_mode,
                    expand_mode,
                    collapsed,
                    active_path,
                    find_query,
                    narrow_open,
                };
                let srv = server.clone();
                crate::files::trace::review_asked(session_id, rev);
                self.review.diff_fetch.request(spec, move |spec| fetch_live_diffs(&srv, spec));
            }
        }
    }

    /// Lands a read; returns the change a step asked to show once it did.
    fn apply_fetched_diffs(&mut self, fetched: FetchedDiffs) -> Option<(String, Option<crate::tool_open::Anchor>)> {
        let swap = Instant::now();
        let diff_mode = self.review.diff_mode;
        let expand_mode = self.review.expand_mode;
        let prepared = match fetched.changes {
            Ok(prepared) => prepared,
            Err(e) => {
                let what = opencode_i18n::t!("rune.review.readFailed").to_owned();
                let server = crate::servers::runtime::label(crate::servers::runtime::active(self));
                let offline = matches!(e.kind, opencode_kit::wire_error::WireKind::Unreachable(_) | opencode_kit::wire_error::WireKind::TimedOut);
                let why = crate::errors::UserError::from_wire(&what, &server, &e).description();
                let sr = self.review.sessions.entry(fetched.session_id.clone()).or_default();
                if sr.mode == fetched.mode {
                    sr.load.failed_with(fetched.mode, fetched.revision, why, offline);
                    sr.stamp += 1;
                }
                return None;
            }
        };
        let sr = self.review.sessions.entry(fetched.session_id.clone()).or_default();
        if sr.mode != fetched.mode {
            crate::code_view::prepared::drop_off_ui(prepared);
            return None;
        }
        sr.branch_current = fetched.branch_current;
        sr.branch_default = fetched.branch_default;
        let reopen: Vec<_> = prepared.folders.iter().filter(|p| sr.tree.is_expanded(p)).cloned().collect();
        sr.generation += 1;
        let mut next_diffs = prepared.diffs.into_state();
        next_diffs.set_viewport([0.0, 0.0, sr.diffs.view_width(), 0.0]);
        next_diffs.carry_find_from(&sr.diffs);
        next_diffs.set_mode(diff_mode);
        next_diffs.set_expand_mode(expand_mode);
        next_diffs.keep_keys_rev(&sr.diffs);
        let old_folders = std::mem::replace(
            &mut sr.folders,
            prepared.folders.into_iter().map(|p| (p, folders::Listing::default())).collect(),
        );
        sr.root_count = prepared.count;
        let old_meta = std::mem::replace(&mut sr.meta, prepared.meta);
        let old_listed = std::mem::replace(&mut sr.listed, prepared.listed);
        let old_tree = std::mem::replace(&mut sr.tree, prepared.tree);
        let old_diffs = std::mem::replace(&mut sr.diffs, next_diffs);
        let old_narrow = std::mem::replace(&mut sr.narrow, mobile::NarrowReview::from_prepared(prepared.narrow));
        sr.active_path = prepared.active_path;
        sr.stamp += 1;
        let file_count = sr.diffs.files().len();
        if !old_diffs.files().is_empty() || !old_meta.is_empty() {
            let old_diffs = old_diffs.into_prepared();
            let old_narrow = old_narrow.into_prepared();
            crate::code_view::prepared::drop_off_ui((old_diffs, old_meta, old_listed, old_tree, old_folders, old_narrow));
        }
        let focus = sr.load.landed(fetched.mode, fetched.revision, fetched.no_git);
        for folder in reopen { self.expand_review_dir(&fetched.session_id, &folder, true); }
        crate::files::trace::review_landed(&fetched.session_id, fetched.revision, swap, file_count);
        focus
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
            sr.load.restart();
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
        let Source::Live { server, .. } = &*self.source else { return };
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
        let Source::Live { server, .. } = &*self.source else {
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
        matches!(*self.route, Route::Session { .. })
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
                sr.load.restart();
                sr.stamp += 1;
                self.review.asked = None;
            }
        }
        self.show_pane_tab(tx, |s| s.show_side_tab(sid, REVIEW_KEY, None));
        self.ensure_session_review(sid);
        if let Some(p) = path {
            let anchor = line.map(|n| crate::tool_open::Anchor { side: rune_kit::diff::Side::New, text: String::new(), near: Some(n.max(1)) });
            if let Some(Load::Loading { focus }) = self.review.sessions.get_mut(sid).map(|sr| &mut sr.load) {
                *focus = Some((p.to_owned(), anchor));
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
        self.add_folder_nodes();
        self.diffs.set_mode(diff_mode);
        self.diffs.set_files(files);
        self.narrow.refresh(self.diffs.files());
        self.diffs.set_expand_mode(expand_mode);
        self.listed = listed;
        let next = prev.filter(|p| self.tree.get(p).is_some_and(|n| !n.dir)).or_else(|| self.listed.first().cloned());
        self.diffs.show_only(None);
        match next {
            Some(path) => self.show(&path),
            None => self.stamp += 1,
        }
        self.stamp += 1;
    }
}

fn same_directory(a: &str, b: &str) -> bool {
    opencode_kit::path::contains(a, b) && opencode_kit::path::contains(b, a)
}
