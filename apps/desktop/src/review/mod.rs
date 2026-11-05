//! Changes / Review panel (`packages/gui-extensions/src/review/`), registered on `panels::PANELS`.

pub mod data;
mod stories;
#[cfg(test)]
mod tests;
pub mod view;

use std::collections::HashMap;
use std::time::Duration;

use rune::{Key, KeyEvent, Latest, TextEdit, Tx, Waker};
use rune_kit::diff::{DiffMode, Expand};
use rune_kit::{FileNode, TreeModel};
use rune_theme_opencode::icons as ic;

use crate::code_view::{DiffDocState, ExpandMode};
use crate::panels::{
    CloseButtonMode, PanelRegistration, PanelSessionCtx, PanelTabDesc, SessionPane, ShortcutChord,
    panel_key,
};
use crate::{App, Route, Source};
use data::{
    FetchedDiffs, ReviewFileMeta, build_change_tree, collapsed_folders, fetch_live_diffs,
    fetch_live_file_text, filter_review_files, fixture_diff_count, load_fixture_diffs, parse_diff_list,
};

pub const REVIEW_KEY: &str = "review:changes";
pub const SIDEBAR_DEFAULT_W: f32 = 216.0;
pub const SIDEBAR_MIN_W: f32 = 190.0;
pub const SIDEBAR_MAX_W: f32 = 360.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChangeMode {
    #[default]
    Git,
    Branch,
    Turn,
}

impl ChangeMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Git => "Uncommitted",
            Self::Branch => "Branch",
            Self::Turn => "Last turn",
        }
    }

    pub const fn empty_label(self) -> &'static str {
        match self {
            Self::Git => "No uncommitted changes yet",
            Self::Branch => "No branch changes yet",
            Self::Turn => "No changes",
        }
    }
}

pub struct SessionReview {
    pub mode: ChangeMode,
    pub diffs: DiffDocState,
    pub meta: Vec<ReviewFileMeta>,
    pub tree: TreeModel<FileNode<ReviewFileMeta>>,
    pub active_path: Option<String>,
    pub highlighted_filter_path: Option<String>,
    pub ready: bool,
    pub no_git: bool,
    pub branch_current: Option<String>,
    pub branch_default: Option<String>,
    pub loaded_revision: Option<(ChangeMode, u64)>,
    pub pending_expand: Option<(usize, usize, Expand)>,
    /// A tool step's change to scroll to once the diffs load (`tool_open`).
    pub pending_focus: Option<(String, Option<crate::tool_open::Anchor>)>,
}

impl Default for SessionReview {
    fn default() -> Self {
        Self {
            mode: ChangeMode::Git,
            diffs: DiffDocState::default(),
            meta: Vec::new(),
            tree: TreeModel::new().key("review-tree"),
            active_path: None,
            highlighted_filter_path: None,
            ready: false,
            no_git: false,
            branch_current: None,
            branch_default: None,
            loaded_revision: None,
            pending_expand: None,
            pending_focus: None,
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
        self.meta
            .iter()
            .fold((0, 0), |(a, d), m| (a + m.additions, d + m.deletions))
    }
}

pub struct ReviewPane {
    pub sessions: HashMap<String, SessionReview>,
    pub empty_diffs: DiffDocState,
    pub sidebar_open: bool,
    pub sidebar_w: f32,
    pub sidebar_resized: bool,
    pub sidebar_drag: Option<f32>,
    pub mode_menu_open: bool,
    pub diff_mode: DiffMode,
    pub expand_mode: ExpandMode,
    pub filter: TextEdit,
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
            sidebar_open: true,
            sidebar_w: SIDEBAR_DEFAULT_W,
            sidebar_resized: false,
            sidebar_drag: None,
            mode_menu_open: false,
            diff_mode: DiffMode::Unified,
            expand_mode: ExpandMode::Collapse,
            filter: TextEdit::new(""),
            dirty: false,
            revision: 1,
            diff_fetch: Latest::default().debounce(Duration::from_millis(100)),
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

rune_register::register!(crate::panels::PANELS: PanelRegistration = [
    PanelRegistration::new("review", list_tabs, view::render)
        .order(5)
        .transient(true)
        .legacy(&[("review", "changes")])
        .wide(is_split_wide)
        .command("Review changes", ShortcutChord::ctrl_cmd('r', "⌃⌘R"))
        .launcher(no_launcher, open_review_command)
        .on_enter(on_enter_review)
        .on_focus(on_focus_review)
        .poll(poll_review)
        .on_key(key_review),
]);

fn no_launcher(_s: &App, _ctx: &PanelSessionCtx) -> Option<crate::panels::PanelLauncherDesc> {
    None
}

fn open_review_command(s: &mut App, _pane: SessionPane, tx: &mut Tx) {
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    s.side.mounted = true;
    s.session_side_mut(&sid).open = true;
    s.select_side_tab(&panel_key("review", "changes"), tx);
    s.ensure_session_review(&sid);
}

fn is_split_wide(s: &App) -> bool {
    s.review.diff_mode == DiffMode::Split
}

fn list_tabs(s: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    if ctx.project_id.is_empty() {
        return Vec::new();
    }
    let count = match s.review.sessions.get(ctx.session_id) {
        Some(r) => r.meta.len(),
        None if matches!(s.source, Source::Fixture) => fixture_diff_count(&s.fixture, ctx.session_id),
        None => 0,
    };
    let title = if count > 0 {
        format!("Files Changed {count}")
    } else {
        "Review".to_owned()
    };
    vec![
        PanelTabDesc::new("changes", title)
            .icon(&ic::CODE_LINES)
            .pinned()
            .closable(CloseButtonMode::None)
            .fallback(),
    ]
}

fn on_enter_review(s: &mut App, session_id: &str, _tab_id: &str) {
    s.ensure_session_review(session_id);
}

fn on_focus_review(s: &mut App, session_id: &str, _tab_id: &str, _restored: bool) {
    s.ensure_session_review(session_id);
}

fn poll_review(s: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(fetched) = s.review.diff_fetch.take() {
        tx.instant();
        let sid = fetched.session_id.clone();
        s.apply_fetched_diffs(fetched);
        s.apply_pending_review_focus(&sid, tx);
        changed = true;
    }
    if let Some((session_id, path, content)) = s.review.file_fetch.take() {
        if let Some(text) = content
            && let Some(sr) = s.review.sessions.get_mut(&session_id)
        {
            tx.instant();
            sr.diffs.attach_file_text(&path, &text);
            if let Some((f, gap, how)) = sr.pending_expand.take() {
                let _ = sr.diffs.expand(f, gap, how);
            }
            changed = true;
        }
    }
    if s.review.dirty {
        s.review.dirty = false;
        if let Some(sid) = s.active_session_id().map(str::to_owned)
            && (s.side_open() || s.stage.is_some())
        {
            s.ensure_session_review(&sid);
            changed = true;
        }
    }
    changed
}

fn key_review(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !s.is_review_active() {
        return false;
    }
    // `⌘\` toggles the file tree sidebar inside the Review panel (`fileTree.toggle`).
    let is_cmd_backslash = ev.mods.cmd
        && !ev.mods.alt
        && !ev.mods.ctrl
        && !ev.mods.shift
        && (matches!(&ev.key, Key::Command('\\'))
            || matches!(&ev.key, Key::Text(t) if t == "\\"));
    if is_cmd_backslash {
        tx.instant();
        s.review.sidebar_open = !s.review.sidebar_open;
        return true;
    }

    if s.review.filter.focused {
        let Some(sid) = s.active_session_id().map(str::to_owned) else {
            return false;
        };
        let query = s.review.filter.text.clone();
        if ev.key == Key::Escape {
            tx.instant();
            if !query.is_empty() {
                s.review.filter = TextEdit::new("");
                s.review.filter.focused = true;
            } else {
                s.review.filter.focused = false;
            }
            return true;
        }
        let all_paths: Vec<String> = s
            .review
            .sessions
            .get(&sid)
            .map(|r| r.meta.iter().map(|m| m.path.clone()).collect())
            .unwrap_or_default();
        let filtered = filter_review_files(&all_paths, &query);
        if !query.trim().is_empty() && !filtered.is_empty() {
            match &ev.key {
                Key::Down | Key::Up => {
                    tx.instant();
                    let cur = s
                        .review
                        .sessions
                        .get(&sid)
                        .and_then(|r| r.highlighted_filter_path.as_deref())
                        .and_then(|p| filtered.iter().position(|x| x == p));
                    let delta: isize = if ev.key == Key::Down { 1 } else { -1 };
                    let next_idx = match cur {
                        Some(i) => (i as isize + delta).clamp(0, filtered.len() as isize - 1) as usize,
                        None => 0,
                    };
                    if let Some(sr) = s.review.sessions.get_mut(&sid) {
                        sr.highlighted_filter_path = Some(filtered[next_idx].clone());
                    }
                    return true;
                }
                Key::Enter => {
                    let pick = s
                        .review
                        .sessions
                        .get(&sid)
                        .and_then(|r| r.highlighted_filter_path.clone())
                        .filter(|p| filtered.contains(p))
                        .or_else(|| filtered.first().cloned());
                    if let Some(path) = pick {
                        s.focus_review_file(&path, tx);
                        return true;
                    }
                }
                _ => {}
            }
        }
        return false;
    }

    if s.composer.focused || !s.side.focused {
        return false;
    }

    if ev.key == Key::Escape && s.review.mode_menu_open {
        tx.instant();
        s.review.mode_menu_open = false;
        return true;
    }

    let Some(sid) = s.active_session_id().map(str::to_owned) else {
        return false;
    };
    let handled = s
        .review
        .sessions
        .get_mut(&sid)
        .is_some_and(|sr| sr.diffs.handle_key(ev, tx));
    if handled {
        s.sync_review_tree_to_diff(&sid, tx);
    }
    handled
}

impl App {
    pub fn is_review_active(&self) -> bool {
        if !(self.side_open() || self.stage.is_some()) {
            return false;
        }
        self.resolve_side_region()
            .and_then(|r| r.active)
            .as_deref()
            == Some(REVIEW_KEY)
    }

    pub fn note_review_event(&mut self, ty: &str) -> bool {
        if matches!(
            ty,
            "filesystem.changed"
                | "session.execution.succeeded"
                | "session.step.ended"
                | "session.revert.committed"
        ) {
            self.review.revision += 1;
            if self.side_open() {
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
        let directory = self
            .session(session_id)
            .map(|i| i.directory.clone())
            .unwrap_or_default();
        let mode = self
            .review
            .sessions
            .entry(session_id.to_owned())
            .or_default()
            .mode;

        if self
            .review
            .sessions
            .get(session_id)
            .is_some_and(|sr| sr.loaded_revision == Some((mode, rev)))
        {
            if let Some(sr) = self.review.sessions.get_mut(session_id) {
                sr.diffs.set_mode(diff_mode);
                sr.diffs.set_expand_mode(expand_mode);
            }
            return;
        }

        match &self.source {
            Source::Fixture => {
                let (files, meta, cur_b, def_b, texts) =
                    load_fixture_diffs(&self.fixture, session_id, mode);
                let sr = self.review.sessions.entry(session_id.to_owned()).or_default();
                sr.branch_current = cur_b;
                sr.branch_default = def_b;
                sr.meta = meta.clone();
                sr.tree = build_change_tree(&meta, &collapsed_folders(&sr.tree));
                sr.active_path = meta.first().map(|m| m.path.clone());
                sr.diffs.set_mode(diff_mode);
                sr.diffs.set_files(files);
                for (path, text) in texts {
                    sr.diffs.attach_file_text(&path, &text);
                }
                sr.diffs.set_expand_mode(expand_mode);
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
                self.review
                    .diff_fetch
                    .request((sid, directory, mode, rev), move |(sid, dir, mode, rev)| {
                        fetch_live_diffs(&srv, sid, dir, mode, rev)
                    });
            }
        }
    }

    fn apply_fetched_diffs(&mut self, fetched: FetchedDiffs) {
        let diff_mode = self.review.diff_mode;
        let expand_mode = self.review.expand_mode;
        let (files, meta) = parse_diff_list(&fetched.raw_diff);
        let sr = self
            .review
            .sessions
            .entry(fetched.session_id.clone())
            .or_default();
        if sr.mode != fetched.mode {
            return;
        }
        let prev_active = sr.active_path.clone();
        sr.branch_current = fetched.branch_current;
        sr.branch_default = fetched.branch_default;
        sr.no_git = fetched.no_git;
        sr.meta = meta.clone();
        sr.tree = build_change_tree(&meta, &collapsed_folders(&sr.tree));
        sr.diffs.set_mode(diff_mode);
        sr.diffs.set_files(files);
        sr.diffs.set_expand_mode(expand_mode);
        sr.active_path = prev_active
            .filter(|p| sr.meta.iter().any(|m| &m.path == p))
            .or_else(|| sr.meta.first().map(|m| m.path.clone()));
        if let Some(act) = &sr.active_path {
            sr.tree.select(act);
        }
        sr.ready = true;
        sr.loaded_revision = Some((fetched.mode, fetched.revision));
    }

    pub fn set_review_change_mode(&mut self, mode: ChangeMode) {
        self.review.mode_menu_open = false;
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        if let Some(sr) = self.review.sessions.get_mut(&sid)
            && sr.mode != mode
        {
            sr.mode = mode;
            sr.loaded_revision = None;
            sr.ready = false;
            self.review.asked = None;
        }
        self.ensure_session_review(&sid);
    }

    pub fn set_review_diff_mode(&mut self, mode: DiffMode) {
        if self.review.diff_mode == mode {
            return;
        }
        self.review.diff_mode = mode;
        for sr in self.review.sessions.values_mut() {
            sr.diffs.set_mode(mode);
        }
    }

    pub fn set_review_expand_mode(&mut self, mode: ExpandMode) {
        if self.review.expand_mode == mode {
            return;
        }
        self.review.expand_mode = mode;
        if mode == ExpandMode::Expand
            && let Some(sid) = self.active_session_id().map(str::to_owned)
        {
            self.request_missing_review_texts(&sid);
        }
        for sr in self.review.sessions.values_mut() {
            sr.diffs.set_expand_mode(mode);
        }
    }

    fn request_missing_review_texts(&mut self, session_id: &str) {
        let Source::Live { server, .. } = &self.source else { return };
        let directory = self
            .session(session_id)
            .map(|i| i.directory.clone())
            .unwrap_or_default();
        let Some(sr) = self.review.sessions.get(session_id) else { return };
        let missing = (0..sr.diffs.files().len())
            .find(|&f| !sr.diffs.has_full_text(f))
            .map(|f| sr.diffs.files()[f].path().to_owned());
        if let Some(path) = missing {
            let srv = server.clone();
            let sid = session_id.to_owned();
            self.review
                .file_fetch
                .request((sid, directory, path), move |(sid, dir, path)| {
                    let content = fetch_live_file_text(&srv, &dir, &path);
                    (sid, path, content)
                });
        }
    }

    pub fn request_review_gap_expand(&mut self, f: usize, gap: usize, how: Expand) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let path = {
            let Some(sr) = self.review.sessions.get_mut(&sid) else { return };
            let Some(fd) = sr.diffs.files().get(f) else { return };
            sr.pending_expand = Some((f, gap, how));
            fd.path().to_owned()
        };
        if let Source::Live { server, .. } = &self.source {
            let directory = self
                .session(&sid)
                .map(|i| i.directory.clone())
                .unwrap_or_default();
            let srv = server.clone();
            self.review
                .file_fetch
                .request((sid, directory, path), move |(sid, dir, path)| {
                    let content = fetch_live_file_text(&srv, &dir, &path);
                    (sid, path, content)
                });
        }
    }

    pub fn submit_review_filter(&mut self, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let query = self.review.filter.text.clone();
        if query.trim().is_empty() {
            return;
        }
        let all_paths: Vec<String> = self
            .review
            .sessions
            .get(&sid)
            .map(|r| r.meta.iter().map(|m| m.path.clone()).collect())
            .unwrap_or_default();
        let filtered = filter_review_files(&all_paths, &query);
        let pick = self
            .review
            .sessions
            .get(&sid)
            .and_then(|r| r.highlighted_filter_path.clone())
            .filter(|p| filtered.contains(p))
            .or_else(|| filtered.first().cloned());
        if let Some(path) = pick {
            self.focus_review_file(&path, tx);
        }
    }

    pub fn focus_review_file(&mut self, path: &str, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let Some(sr) = self.review.sessions.get_mut(&sid) else { return };
        sr.active_path = Some(path.to_owned());
        sr.tree.reveal(path, tx);
        if let Some(idx) = sr.diffs.file_index(path) {
            sr.diffs.scroll_to_file(idx, tx);
        }
    }

    pub fn sync_review_tree_to_diff(&mut self, session_id: &str, tx: &mut Tx) {
        let Some(sr) = self.review.sessions.get_mut(session_id) else { return };
        let idx = sr.diffs.visible_file();
        if let Some(fd) = sr.diffs.files().get(idx) {
            let path = fd.path().to_owned();
            if sr.active_path.as_deref() != Some(path.as_str()) {
                sr.active_path = Some(path.clone());
                // Scrolling follows in the tree without opening folders the user closed.
                if sr.tree.visible_row(&path).is_some() {
                    sr.tree.reveal(&path, tx);
                } else {
                    sr.tree.select(&path);
                }
            }
        }
    }

    pub fn route_supports_review(&self) -> bool {
        matches!(self.route, Route::Session(_))
    }

    pub fn open_review_for_session(
        &mut self,
        sid: &str,
        mode: Option<ChangeMode>,
        view: Option<DiffMode>,
        path: Option<&str>,
        line: Option<u32>,
        tx: &mut Tx,
    ) {
        let saved_focus = (self.composer.focused, self.side.focused, self.side.dock_focused);
        tx.instant();
        if let Some(v) = view {
            self.set_review_diff_mode(v);
        }
        if let Some(m) = mode {
            let sr = self.review.sessions.entry(sid.to_owned()).or_default();
            if sr.mode != m {
                sr.mode = m;
                sr.loaded_revision = None;
                sr.ready = false;
                self.review.asked = None;
            }
        }
        self.side.mounted = true;
        {
            let st = self.session_side_mut(sid);
            st.open = true;
            st.tab_state.tabs.active = Some(REVIEW_KEY.to_owned());
            st.tab_state.tabs.selected.insert(SessionPane::Side, REVIEW_KEY.to_owned());
        }
        self.side.adding.retain(|n| !(n.session_id == sid && n.pane == SessionPane::Side));
        self.ensure_session_review(sid);
        if let Some(p) = path {
            let anchor = line.map(|n| crate::tool_open::Anchor {
                side: rune_kit::diff::Side::New,
                text: String::new(),
                near: Some(n.max(1)),
            });
            if self.review.sessions.get(sid).is_some_and(|sr| !sr.ready) {
                if let Some(sr) = self.review.sessions.get_mut(sid) {
                    sr.pending_focus = Some((p.to_owned(), anchor));
                }
            } else {
                self.focus_review_change(sid, p, anchor.as_ref(), tx);
            }
        }
        (self.composer.focused, self.side.focused, self.side.dock_focused) = saved_focus;
    }
}
