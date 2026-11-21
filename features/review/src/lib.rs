//! The changes engine behind Desktop's review plugin (`packages/gui-plugins/src/review/`), which
//! now runs as Luau (`plugins/review`): each session's diffs for the current source (`git`,
//! `branch` or `turn`) read from the server, parsed off the UI thread into the diff view's state,
//! and the file the preview shows. Also provides untracked folder loading (`folders`), the narrow
//! Changes view (`mobile`), focus anchor matching (`focus`), and the file-panel chrome Review and
//! Files share (`chrome`).

#![forbid(unsafe_code)]

pub mod chrome;
mod engine;
mod focus;
mod folders;
pub mod mobile;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use opencode_code_view::data::review::{FetchedDiffs, FetchedVcs, ReviewFileMeta, build_change_tree, collapsed_folders};
use opencode_code_view::{DiffDocState, ExpandMode};
use opencode_kit::client::Server;
use opencode_kit::wire_error::WireError;
use rune::time::Instant;
use rune::{Latest, Tx, Waker};
use rune_kit::diff::{DiffMode, FileDiff};
use rune_kit::{FileNode, TreeModel};
use serde_json::Value;

pub use engine::{
    apply_reply, ensure_session, execute_job, note_branch_updated, note_event, open_for_session, poll,
    prefetch_vcs, refresh, refresh_on_open, request_file_text, set_diff_mode, set_expand_mode, set_mode,
    set_session_mode, show, sync_narrow_wrap,
};
pub use focus::{Anchor, Located, focus_change, locate};
pub use folders::{expand_folder, toggle_folder};
pub use opencode_code_view::data::review as data;
pub use opencode_code_view::data::review::ChangeMode;

/// The review panel's one tab (Desktop's `review:changes`).
pub const REVIEW_KEY: &str = "review:changes";

/// Where a session's changes stand, as Desktop's `gui-plugins/src/review/machine.ts`
/// (`ReviewDiffState`) shows them (`ReviewDiffView`): loading, or landed for a mode and file
/// revision, where a project without a repository has none to show (`no-vcs`). A newer revision
/// reads again while the landed changes stay shown (`refreshing`).
#[derive(Debug)]
pub enum Load {
    /// The mode's changes haven't landed; `focus` is a change to scroll to once they do.
    Loading { focus: Option<(String, Option<Anchor>)> },
    Ready { mode: ChangeMode, revision: u64 },
    NoVcs { mode: ChangeMode, revision: u64 },
    /// Beyond Desktop, which shows "No file changes yet" when the read fails: the read failed,
    /// `why` in the host's words; the changes it showed before stay.
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
    pub(crate) fn landed(&mut self, mode: ChangeMode, revision: u64, no_git: bool) -> Option<(String, Option<Anchor>)> {
        let next = if no_git { Load::NoVcs { mode, revision } } else { Load::Ready { mode, revision } };
        match std::mem::replace(self, next) {
            Load::Loading { focus } => focus,
            Load::Ready { .. } | Load::NoVcs { .. } | Load::Failed { .. } => None,
        }
    }

    /// The read of `mode` at `revision` failed; a change waiting to be shown waits for the next.
    pub(crate) fn failed_with(&mut self, mode: ChangeMode, revision: u64, why: String, offline: bool) {
        *self = Load::Failed { mode, revision, why, offline };
    }

    /// Another mode: its changes load from scratch.
    pub(crate) fn restart(&mut self) {
        *self = Load::Loading { focus: None };
    }
}

pub struct SessionReview {
    pub mode: ChangeMode,
    pub diffs: DiffDocState,
    pub meta: Vec<ReviewFileMeta>,
    pub root_count: usize,
    pub(crate) generation: u64,
    pub(crate) folders: HashMap<String, folders::Listing>,
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

    /// Resets a session's diff state when switching to an uncached mode so the previous mode's
    /// rows do not linger while the new mode loads.
    pub(crate) fn clear_for_mode_switch(&mut self) {
        self.meta.clear();
        self.listed.clear();
        self.folders.clear();
        self.tree = TreeModel::new().key("review-tree");
        self.diffs = DiffDocState::default();
        self.narrow = mobile::NarrowReview::default();
        self.root_count = 0;
        self.active_path = None;
        self.load.restart();
        self.stamp += 1;
    }

    /// Hydrates this session from a cached diff snapshot so reopen, same-directory session
    /// switches, and mode switches paint on frame 1 while a background refresh runs
    /// (`packages/gui-plugins/src/review/model.ts:296, 311, 327-330`, `machine.ts:212-215, 238-240`).
    pub(crate) fn hydrate_from_cache(
        &mut self,
        cached: &CachedReview,
        vcs: Option<&CachedVcs>,
        diff_mode: DiffMode,
        expand_mode: ExpandMode,
        wrap: bool,
    ) {
        self.branch_current = vcs
            .and_then(|v| v.branch_current.clone())
            .or_else(|| cached.branch_current.clone());
        self.branch_default = vcs
            .and_then(|v| v.branch_default.clone())
            .or_else(|| cached.branch_default.clone());
        self.root_count = cached.count;
        self.folders = cached.folders.iter().map(|p| (p.clone(), folders::Listing::default())).collect();
        self.land(cached.files.clone(), cached.meta.clone(), cached.listed.clone(), diff_mode, expand_mode, wrap);
        let no_git = vcs.map_or(cached.no_git, |v| v.lacks_provider && cached.count == 0);
        // Revision 0 marks the hydrated state ready to paint immediately while keeping it older
        // than `ReviewPane.revision` (>= 1) so `ensure_session` still refreshes in the background.
        let _ = self.load.landed(self.mode, 0, no_git);
    }

    /// Takes a new list of changes: the tree keeps the folders the user closed, the preview keeps
    /// its file while it still changed, else shows the server's first (Desktop's `activeFile`).
    pub(crate) fn land(
        &mut self,
        files: Vec<FileDiff>,
        meta: Vec<ReviewFileMeta>,
        listed: Vec<String>,
        diff_mode: DiffMode,
        expand_mode: ExpandMode,
        wrap: bool,
    ) {
        let prev = self.active_path.take();
        self.meta = meta;
        self.tree = build_change_tree(&self.meta, &collapsed_folders(&self.tree));
        self.add_folder_nodes();
        self.diffs.set_mode(diff_mode);
        self.diffs.set_files(files);
        self.narrow.refresh(self.diffs.files());
        self.narrow.set_wrap(wrap);
        self.diffs.set_expand_mode(expand_mode);
        self.listed = listed;
        let next = prev
            .filter(|p| self.tree.get(p).is_some_and(|n| !n.dir))
            .or_else(|| self.listed.first().cloned());
        self.diffs.show_only(None);
        match next {
            Some(path) => self.show(&path),
            None => self.stamp += 1,
        }
        self.stamp += 1;
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

/// Canonical directory key for VCS and diff caches: normalizes separators and folds ASCII case on
/// Windows drive/UNC paths (`opencode_kit::path::windows`).
pub fn dir_key(dir: &str) -> String {
    let trimmed = opencode_kit::path::trim(dir);
    if opencode_kit::path::windows(trimmed) {
        trimmed.replace('\\', "/").to_ascii_lowercase()
    } else {
        trimmed.replace('\\', "/")
    }
}

/// Cached `/api/vcs` metadata for one `(server_base, dir_key)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedVcs {
    pub branch_current: Option<String>,
    pub branch_default: Option<String>,
    pub lacks_provider: bool,
}

/// Cache key mirroring Desktop's query key (`packages/gui-plugins/src/review/model.ts:327-330`):
/// Git/Branch diffs are shared per `(server_base, directory, mode)`; Turn diffs are per session.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReviewCacheKey {
    Vcs { server_base: String, directory: String, mode: ChangeMode },
    Turn { server_base: String, session_id: String },
}

impl ReviewCacheKey {
    pub fn for_target(server_base: &str, directory: &str, session_id: &str, mode: ChangeMode) -> Self {
        match mode {
            ChangeMode::Turn => Self::Turn { server_base: server_base.to_owned(), session_id: session_id.to_owned() },
            ChangeMode::Git | ChangeMode::Branch => Self::Vcs {
                server_base: server_base.to_owned(),
                directory: dir_key(directory),
                mode,
            },
        }
    }
}

/// Last landed diff snapshot for one [`ReviewCacheKey`], used to paint reopen, same-directory
/// session switches, and mode switches immediately while refreshing in the background.
#[derive(Clone)]
pub struct CachedReview {
    pub files: Vec<FileDiff>,
    pub meta: Vec<ReviewFileMeta>,
    pub listed: Vec<String>,
    pub folders: Vec<String>,
    pub count: usize,
    pub no_git: bool,
    pub branch_current: Option<String>,
    pub branch_default: Option<String>,
    pub seq: u64,
}

const REVIEW_CACHE_CAP: usize = 8;

pub struct ReviewPane {
    pub sessions: HashMap<String, SessionReview>,
    pub empty_diffs: DiffDocState,
    pub diff_mode: DiffMode,
    pub expand_mode: ExpandMode,
    pub refresh: Refresh,
    pub revision: u64,
    pub(crate) diff_fetch: Latest<FetchedDiffs>,
    pub(crate) vcs_fetch: Latest<(String, String, Option<FetchedVcs>)>,
    pub(crate) file_fetch: Latest<(String, String, Option<String>)>,
    pub(crate) asked: Option<(String, ChangeMode, u64)>,
    pub(crate) vcs_asked: Option<(String, String)>,
    pub(crate) vcs_cache: HashMap<(String, String), CachedVcs>,
    pub(crate) cache: HashMap<ReviewCacheKey, CachedReview>,
    pub(crate) cache_seq: u64,
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
            vcs_fetch: Latest::default(),
            file_fetch: Latest::default(),
            asked: None,
            vcs_asked: None,
            vcs_cache: HashMap::new(),
            cache: HashMap::new(),
            cache_seq: 0,
        }
    }
}

impl ReviewPane {
    /// The data or the pending live request already covers this document. Merely asking
    /// for the same thing again while rendering must not enqueue a follow-up commit.
    pub fn has_read(&self, sid: &str, mode: ChangeMode) -> bool {
        self.sessions
            .get(sid)
            .is_some_and(|sr| sr.mode == mode && sr.load.loaded_revision() == Some((mode, self.revision)))
            || self.asked.as_ref().is_some_and(|(id, m, rev)| id == sid && *m == mode && *rev == self.revision)
    }

    pub fn set_waker(&mut self, waker: Waker) {
        self.diff_fetch.set_waker(waker.clone());
        self.vcs_fetch.set_waker(waker.clone());
        self.file_fetch.set_waker(waker);
    }

    pub(crate) fn store_cache(&mut self, key: ReviewCacheKey, mut entry: CachedReview) {
        self.cache_seq += 1;
        entry.seq = self.cache_seq;
        self.cache.insert(key, entry);
        while self.cache.len() > REVIEW_CACHE_CAP {
            if let Some(oldest) = self.cache.iter().min_by_key(|(_, v)| v.seq).map(|(k, _)| k.clone()) {
                self.cache.remove(&oldest);
            } else {
                break;
            }
        }
    }

    /// Drops cached diffs and VCS records for sessions and directories no open tab or route keeps
    /// (`App::rest`), keeping memory bounded when windows rest.
    pub fn prune_cache(&mut self, keep_sessions: &HashSet<&str>, keep_dirs: &HashSet<String>) {
        self.cache.retain(|k, _| match k {
            ReviewCacheKey::Turn { session_id, .. } => keep_sessions.contains(session_id.as_str()),
            ReviewCacheKey::Vcs { directory, .. } => keep_dirs.contains(directory),
        });
        self.vcs_cache.retain(|(_, dir), _| keep_dirs.contains(dir));
    }
}

/// Off-thread server work scheduled through [`ReviewHost::spawn_job`].
#[derive(Clone, Debug)]
pub enum ReviewJob {
    /// Lists an untracked folder's entries from `/api/vcs/diff?...&untracked=folders`.
    /// Execute with [`execute_job`] against the live [`Server`] and deliver with [`apply_reply`].
    ReadFolder {
        session_id: String,
        path: String,
        mode: ChangeMode,
        generation: u64,
        url: String,
    },
}

/// Result of a [`ReviewJob`], applied back on the UI thread via [`apply_reply`].
#[derive(Debug)]
pub enum ReviewReply {
    FolderLoaded {
        session_id: String,
        path: String,
        mode: ChangeMode,
        generation: u64,
        result: Result<Value, String>,
    },
}

/// Object-safe host boundary for the Review changes engine, untracked folder loader,
/// and narrow Changes view.
pub trait ReviewHost: opencode_kernel::AppEnv {
    /// Review pane state (tracked read).
    fn review(&self) -> &ReviewPane;
    /// Review pane state (tracked write — callers only invoke when mutating).
    fn review_mut(&mut self) -> &mut ReviewPane;

    /// Working directory of session `sid`, if loaded.
    fn session_directory(&self, sid: &str) -> Option<&str>;
    /// Project root directory for session `sid` used to resolve untracked folder paths.
    /// Defaults to [`Self::session_directory`].
    fn session_project_root(&self, sid: &str) -> Option<&str> {
        self.session_directory(sid)
    }

    /// Live OpenCode server handle when connected, or `None` in fixture mode.
    fn live_server(&self) -> Option<&Server>;
    /// Fixture JSON root used when [`Self::live_server`] is `None`.
    fn fixture_root(&self) -> Option<&Value> {
        None
    }

    /// Whether the changes surface is currently visible (side panel open or narrow Changes view).
    fn review_shown(&self) -> bool;
    /// Whether a due refresh on [`poll`] should fetch changes (defaults to [`Self::review_shown`];
    /// desktop also refreshes while a Storybook stage is active).
    fn should_refresh_on_poll(&self) -> bool {
        self.review_shown()
    }

    /// Whether narrow-window diffs wrap long lines (`Settings › Wrap lines`).
    fn wrap_lines(&self) -> bool {
        true
    }

    /// Human-readable label of the active server for error descriptions.
    fn server_label(&self) -> String {
        String::new()
    }
    /// Formats a failed review diff read into a user-facing description.
    fn format_wire_error(&self, what: &str, error: &WireError) -> String {
        let server = self.server_label();
        if server.is_empty() {
            format!("{what}: {}", error.kind)
        } else {
            format!("{what} ({server}): {}", error.kind)
        }
    }

    /// Opens the review side tab for session `sid`. When `focus_panel` is `true` ([`show`]),
    /// focus moves into the side panel; when `false` ([`open_for_session`]), existing composer
    /// and side focus is preserved.
    fn show_review_tab(&mut self, sid: &str, focus_panel: bool, tx: &mut Tx) {
        let _ = (sid, focus_panel, tx);
    }

    /// Opens workspace file `path` from the narrow Changes view's "Open file" button.
    fn open_file_in_files_panel(&mut self, sid: &str, path: &str, tx: &mut Tx) {
        let _ = (sid, path, tx);
    }

    /// Schedules an off-thread [`ReviewJob`] against the live server; execute with [`execute_job`]
    /// and deliver the result on the UI thread with [`apply_reply`].
    fn spawn_job(&mut self, job: ReviewJob) {
        let _ = job;
    }

    /// Optional instrumentation hook called when a live diff read is dispatched.
    fn trace_review_asked(&self, sid: &str, revision: u64) {
        let _ = (sid, revision);
    }

    /// Optional instrumentation hook called when a live diff read lands on the UI thread.
    fn trace_review_landed(&self, sid: &str, revision: u64, swap: Instant, files: usize) {
        let _ = (sid, revision, swap, files);
    }

    /// Notification hook called after review state changes for `sid`.
    fn on_review_changed(&mut self, sid: &str) {
        let _ = sid;
    }
}
