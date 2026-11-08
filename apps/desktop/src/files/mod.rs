//! Files panel (`packages/gui-extensions/src/file/`), registered on `panels::PANELS` (`⌘P` opens
//! the palette, `switcher.rs`, which searches the folder's files)
//! and sharing [`CodeViewer`](crate::code_view::CodeViewer) with the Review panel's `code_view` engine.

pub mod data;
pub mod link_tip;
mod link_tip_stories;
#[cfg(test)]
mod link_tip_tests;
pub mod links;
#[cfg(test)]
mod links_tests;
pub mod markdown;
mod markdown_stories;
#[cfg(test)]
mod markdown_tests;
mod stories;
#[cfg(test)]
mod tests;
pub mod view;
mod viewer;

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use rune::{Key, KeyEvent, Latest, TextEdit, Tx, Waker};
use rune_kit::{FileNode, TreeModel};
use rune_theme_opencode::icons as ic;

use crate::keymap::Bind;
use crate::code_view::CodeDocState;
use crate::panels::{
    PanelLauncherDesc, PanelRegistration, PanelSessionCtx, PanelTabDesc,
    SessionPane, panel_key,
};
use crate::{App, Source};
use data::{
    FilePayload, OPEN_TAB_ID, OPEN_TAB_KEY, build_workspace_tree, fetch_live_dir, fetch_live_file_payload,
    fetch_live_find, file_tab_key, file_tab_path, load_fixture_workspace, rank_file_query,
};

/// The tree's width until dragged: the same sidebar as Review's.
pub const FILES_SIDEBAR_DEFAULT_W: f32 = crate::review::SIDEBAR_DEFAULT_W;
pub const FILES_SIDEBAR_MIN_W: f32 = crate::review::SIDEBAR_MIN_W;
pub const FILES_SIDEBAR_MAX_W: f32 = crate::review::SIDEBAR_MAX_W;

pub struct SessionFiles {
    pub docs: HashMap<String, CodeDocState>,
    pub known_files: BTreeSet<String>,
    pub known_dirs: BTreeSet<String>,
    pub expanded_dirs: BTreeSet<String>,
    pub listed_dirs: BTreeSet<String>,
    pub tree: TreeModel<FileNode<bool>>,
    pub recent: Vec<String>,
    pub highlighted_filter_path: Option<String>,
    pub search_found: Vec<String>,
    pub search_asked: Option<String>,
    pub pending_jump: HashMap<String, (u32, u32)>,
    /// Files a link asked for, opened once they load.
    pub link_opens: HashMap<String, LinkOpen>,
    pub md: HashMap<String, markdown::MdView>,
    pub ready: bool,
}

/// Where a link asked to land in the file it opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkOpen {
    pub lines: Option<(u32, u32)>,
    pub heading: Option<String>,
}

impl Default for SessionFiles {
    fn default() -> Self {
        Self {
            docs: HashMap::new(),
            known_files: BTreeSet::new(),
            known_dirs: BTreeSet::new(),
            expanded_dirs: BTreeSet::new(),
            listed_dirs: BTreeSet::new(),
            tree: TreeModel::new().key("files-tree"),
            recent: Vec::new(),
            highlighted_filter_path: None,
            search_found: Vec::new(),
            search_asked: None,
            pending_jump: HashMap::new(),
            link_opens: HashMap::new(),
            md: HashMap::new(),
            ready: false,
        }
    }
}

impl SessionFiles {
    pub fn rebuild_tree(&mut self) {
        self.tree = build_workspace_tree(&self.known_files, &self.known_dirs, &self.expanded_dirs);
    }

    pub fn push_recent(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.push(path.to_owned());
        if self.recent.len() > 30 {
            self.recent.remove(0);
        }
    }
}

pub struct FilesPane {
    pub sessions: HashMap<String, SessionFiles>,
    pub empty_doc: CodeDocState,
    pub sidebar_open: bool,
    pub sidebar_w: f32,
    pub sidebar_resized: bool,
    pub sidebar_drag: Option<f32>,
    pub filter: TextEdit,
    pub dirty: bool,
    pub wrap: markdown::WrapPrefs,
    dir_fetch: Latest<(String, String, Vec<(String, bool)>)>,
    content_fetch: Latest<(String, String, FilePayload)>,
    find_fetch: Latest<(String, String, Vec<String>)>,
}

impl Default for FilesPane {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            empty_doc: CodeDocState::default(),
            sidebar_open: true,
            sidebar_w: FILES_SIDEBAR_DEFAULT_W,
            sidebar_resized: false,
            sidebar_drag: None,
            filter: TextEdit::new(""),
            dirty: false,
            wrap: markdown::WrapPrefs::default(),
            dir_fetch: Latest::default(),
            content_fetch: Latest::default(),
            find_fetch: Latest::default().debounce(Duration::from_millis(100)),
        }
    }
}

impl FilesPane {
    pub fn set_waker(&mut self, waker: Waker) {
        self.dir_fetch.set_waker(waker.clone());
        self.content_fetch.set_waker(waker.clone());
        self.find_fetch.set_waker(waker);
    }
}

rune_register::register!(crate::panels::PANELS: PanelRegistration = [
    PanelRegistration::new("file", list_tabs, view::render)
        .order(15)
        .legacy(&[("open-file", OPEN_TAB_ID)])
        .command(opencode_i18n::key!("command.file.open"), Bind::FileOpen)
        // `palette.search.placeholder`, as Desktop's `file.open` says (`use-session-commands.tsx:515-522`).
        .describe(opencode_i18n::key!("palette.search.placeholder"))
        .shortcuts(&[(Bind::FileBrowse, browse_files)])
        .slash("open")
        .launcher(launcher_desc, open_files_launcher)
        .on_focus(on_focus_file)
        .on_restore(on_restore_file)
        .poll(poll_files)
        .on_key(key_files),
]);

fn launcher_desc(_s: &App, ctx: &PanelSessionCtx) -> Option<PanelLauncherDesc> {
    if ctx.project_id.is_empty() {
        return None;
    }
    Some(PanelLauncherDesc {
        id: OPEN_TAB_ID,
        title: opencode_i18n::t!("plugin.file.command.open").into(),
        description: Some(opencode_i18n::t!("rune.files.launcher.description").into()),
        icon: &ic::FILE_TREE,
        shortcut: &[Bind::FileBrowse, Bind::FileOpen],
        order: 10,
        opened: false,
    })
}

/// Desktop's `file.browse`: the Files tab, as its launcher card opens it.
fn browse_files(s: &mut App, tx: &mut Tx) {
    open_files_launcher(s, SessionPane::Side, tx);
}

fn open_files_launcher(s: &mut App, pane: SessionPane, tx: &mut Tx) {
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    s.ensure_workspace_files(&sid);
    s.open_side_tab(OPEN_TAB_KEY, Some(pane), true, false, tx);
    s.files.filter.focused = true;
}

fn list_tabs(s: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
    let mut out = Vec::new();
    let ws = s.files.sessions.get(ctx.session_id);
    for id in ctx.open_ids {
        if id == OPEN_TAB_ID {
            out.push(
                PanelTabDesc::new(OPEN_TAB_ID, opencode_i18n::t!("plugin.file.command.open"))
                    .icon(&ic::FILE_TREE)
                    .group("browser")
                    .transient()
                    .undraggable(),
            );
            continue;
        }
        let Some(path) = file_tab_path(id) else { continue };
        let name = opencode_kit::path::basename(&path);
        let missing = ws
            .and_then(|w| w.docs.get(&path))
            .is_some_and(|d| d.missing);
        let title = if missing {
            opencode_i18n::t!("plugin.file.tab.notFound", name = name)
        } else {
            name.to_owned()
        };
        let mut desc = PanelTabDesc::new(id.clone(), title)
            .icon(rune_theme_opencode::file_icons::file(name))
            .file(path)
            .group("browser")
            .fallback();
        if missing {
            desc = desc.missing(true);
        }
        out.push(desc);
    }
    out
}

fn on_focus_file(s: &mut App, session_id: &str, tab_id: &str, _restored: bool) {
    s.ensure_workspace_files(session_id);
    if let Some(path) = file_tab_path(tab_id) {
        s.ensure_file_loaded(session_id, &path);
    }
}

fn on_restore_file(
    s: &mut App,
    session_id: &str,
    tab_id: &str,
    _data: Option<&str>,
    _tx: &mut Tx,
) {
    let key = panel_key("file", tab_id);
    s.open_side_tab_for_session(session_id, &key, Some(SessionPane::Side), false, false);
    if let Some(path) = file_tab_path(tab_id) {
        s.ensure_file_loaded(session_id, &path);
    }
}

fn poll_files(s: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some((session_id, rel_dir, entries)) = s.files.dir_fetch.take() {
        tx.instant();
        let ws = s.files.sessions.entry(session_id).or_default();
        ws.listed_dirs.insert(rel_dir);
        for (path, is_dir) in entries {
            if is_dir {
                ws.known_dirs.insert(path);
            } else {
                ws.known_files.insert(path);
            }
        }
        ws.ready = true;
        ws.rebuild_tree();
        changed = true;
    }
    if let Some((session_id, path, payload)) = s.files.content_fetch.take() {
        tx.instant();
        let wrap = s.files.wrap;
        let ws = s.files.sessions.entry(session_id.clone()).or_default();
        let doc = ws.docs.entry(path.clone()).or_insert_with(|| wrap.doc(&path, ""));
        match payload {
            FilePayload::Text(text) => doc.set_text(&text),
            FilePayload::Binary(sz) => doc.set_binary(sz),
            FilePayload::Missing => doc.set_missing(),
        }
        if let Some((start, end)) = ws.pending_jump.remove(&path) {
            let scroll_key = format!("file-code-{session_id}-{path}-scroll");
            doc.scroll_to_lines(&scroll_key, start, end, tx);
        }
        if let Some(link) = ws.link_opens.remove(&path) {
            s.finish_file_link(&session_id, &path, link, tx);
        }
        changed = true;
    }
    if let Some((session_id, query, found)) = s.files.find_fetch.take() {
        if let Some(ws) = s.files.sessions.get_mut(&session_id)
            && ws.search_asked.as_deref() == Some(query.as_str())
        {
            tx.instant();
            ws.search_found = found;
            changed = true;
        }
    }
    if s.files.dirty {
        s.files.dirty = false;
        if let Some(sid) = s.active_session_id().map(str::to_owned) {
            s.ensure_workspace_files(&sid);
            changed = true;
        }
    }
    // Sync active file's live find query if open.
    if let Some((sid, Some(path))) = s.active_file_tab()
        && let Some(ws) = s.files.sessions.get_mut(&sid)
        && let Some(doc) = ws.docs.get_mut(&path)
        && doc.find_open
    {
        changed |= doc.sync_find_query();
        changed |= s.sync_rendered_find(&sid, &path);
    }
    changed
}

fn key_files(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some((sid, active_path)) = s.active_file_tab() else {
        return false;
    };

    // `⌘\` toggles the workspace file tree sidebar.
    let is_cmd_backslash = Bind::TreeToggle.matches_tx(ev, tx);
    if is_cmd_backslash && active_path.is_some() {
        tx.instant();
        s.files.sidebar_open = !s.files.sidebar_open;
        return true;
    }

    if s.files.filter.focused {
        let q = s.files.filter.text.clone();
        if ev.key == Key::Escape {
            tx.instant();
            if !q.is_empty() {
                s.files.filter = TextEdit::new("");
                s.files.filter.focused = true;
            } else {
                s.files.filter.focused = false;
            }
            return true;
        }
        let filtered = s.filtered_sidebar_files(&sid);
        if !q.trim().is_empty() && !filtered.is_empty() {
            match &ev.key {
                Key::Down | Key::Up => {
                    tx.instant();
                    let cur = s
                        .files
                        .sessions
                        .get(&sid)
                        .and_then(|w| w.highlighted_filter_path.as_deref())
                        .and_then(|p| filtered.iter().position(|x| x == p));
                    let delta: isize = if ev.key == Key::Down { 1 } else { -1 };
                    let next_idx = match cur {
                        Some(i) => (i as isize + delta).clamp(0, filtered.len() as isize - 1) as usize,
                        None => 0,
                    };
                    if let Some(ws) = s.files.sessions.get_mut(&sid) {
                        ws.highlighted_filter_path = Some(filtered[next_idx].clone());
                    }
                    return true;
                }
                Key::Enter => {
                    s.submit_files_filter(tx);
                    return true;
                }
                _ => {}
            }
        }
        return false;
    }

    if s.composer.focused || !s.side.focused {
        return false;
    }

    if let Some(path) = active_path {
        let scroll_key = format!("file-code-{sid}-{path}-scroll");
        if let Some(ws) = s.files.sessions.get_mut(&sid)
            && markdown::shows_rendered(ws, &path)
            && let Some(doc) = ws.docs.get(&path)
        {
            // The rendered text keeps its own selection and matches.
            if doc.find_open && doc.find_input.focused && ev.key == Key::Enter {
                s.file_find_step(&sid, &path, !ev.mods.shift, tx);
                return true;
            }
            if Bind::CopySelection.matches_tx(ev, tx) {
                return false;
            }
        }
        if let Some(ws) = s.files.sessions.get_mut(&sid)
            && let Some(doc) = ws.docs.get_mut(&path)
        {
            return doc.handle_key(&scroll_key, ev, tx);
        }
    }
    false
}

impl App {
    /// Returns `(session_id, Some(file_path) | None /* file:open */)` when a `file:*` tab is active & visible.
    pub fn active_file_tab(&self) -> Option<(String, Option<String>)> {
        if !(self.side_open() || self.stage.is_some()) {
            return None;
        }
        let reg = self.resolve_side_region()?;
        let act = reg.active.as_deref()?;
        let id = act.strip_prefix("file:")?;
        if id == OPEN_TAB_ID {
            Some((reg.session_id, None))
        } else {
            file_tab_path(id).map(|p| (reg.session_id, Some(p)))
        }
    }

    pub fn ensure_workspace_files(&mut self, session_id: &str) {
        if self
            .files
            .sessions
            .get(session_id)
            .is_some_and(|w| w.ready)
        {
            return;
        }
        match &self.source {
            Source::Fixture => {
                let (files, contents) = load_fixture_workspace(&self.fixture);
                let wrap = self.files.wrap;
                let ws = self.files.sessions.entry(session_id.to_owned()).or_default();
                ws.known_files = files;
                ws.expanded_dirs.insert("src".into());
                ws.expanded_dirs.insert("src/net".into());
                for (path, text) in contents {
                    ws.docs
                        .entry(path.clone())
                        .or_insert_with(|| wrap.doc(&path, &text));
                }
                ws.ready = true;
                ws.rebuild_tree();
            }
            Source::Live { server, .. } => {
                let ws = self.files.sessions.entry(session_id.to_owned()).or_default();
                if ws.listed_dirs.contains("") {
                    return;
                }
                ws.listed_dirs.insert(String::new());
                let directory = self
                    .session(session_id)
                    .map(|s| s.directory.clone())
                    .unwrap_or_default();
                let srv = server.clone();
                let sid = session_id.to_owned();
                self.files
                    .dir_fetch
                    .request((sid, directory), move |(sid, dir)| {
                        let entries = fetch_live_dir(&srv, &dir, "");
                        (sid, String::new(), entries)
                    });
            }
        }
    }

    pub fn toggle_workspace_dir(&mut self, dir_path: &str) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let need_fetch = {
            let ws = self.files.sessions.entry(sid.clone()).or_default();
            if ws.expanded_dirs.contains(dir_path) {
                ws.expanded_dirs.remove(dir_path);
                ws.rebuild_tree();
                false
            } else {
                ws.expanded_dirs.insert(dir_path.to_owned());
                ws.rebuild_tree();
                matches!(self.source, Source::Live { .. }) && !ws.listed_dirs.contains(dir_path)
            }
        };
        if need_fetch && let Source::Live { server, .. } = &self.source {
            let directory = self
                .session(&sid)
                .map(|s| s.directory.clone())
                .unwrap_or_default();
            let srv = server.clone();
            let rel = dir_path.to_owned();
            self.files
                .dir_fetch
                .request((sid, directory, rel), move |(sid, dir, rel)| {
                    let entries = fetch_live_dir(&srv, &dir, &rel);
                    (sid, rel, entries)
                });
        }
    }

    pub fn ensure_file_loaded(&mut self, session_id: &str, path: &str) {
        self.ensure_workspace_files(session_id);
        let already = self
            .files
            .sessions
            .get(session_id)
            .and_then(|w| w.docs.get(path))
            .is_some();
        if already {
            return;
        }
        match &self.source {
            Source::Fixture => {
                let (_, contents) = load_fixture_workspace(&self.fixture);
                let wrap = self.files.wrap;
                let ws = self.files.sessions.entry(session_id.to_owned()).or_default();
                ws.known_files.insert(path.to_owned());
                let mut doc = wrap.doc(path, "");
                if let Some(txt) = contents.get(path) {
                    doc.set_text(txt);
                } else {
                    doc.set_missing();
                }
                ws.docs.insert(path.to_owned(), doc);
                ws.rebuild_tree();
            }
            Source::Live { server, .. } => {
                let wrap = self.files.wrap;
                let ws = self.files.sessions.entry(session_id.to_owned()).or_default();
                ws.docs.insert(path.to_owned(), wrap.doc(path, ""));
                let directory = self
                    .session(session_id)
                    .map(|s| s.directory.clone())
                    .unwrap_or_default();
                let srv = server.clone();
                let sid = session_id.to_owned();
                let p = path.to_owned();
                self.files
                    .content_fetch
                    .request((sid, directory, p), move |(sid, dir, p)| {
                        let payload = fetch_live_file_payload(&srv, &dir, &p);
                        (sid, p, payload)
                    });
            }
        }
    }

    pub fn open_workspace_file(
        &mut self,
        path: &str,
        preview: bool,
        line: Option<u32>,
        tx: &mut Tx,
    ) {
        self.open_workspace_file_at(path, preview, line.map(|n| (n, n)), tx);
    }

    /// Opens `path` with lines `start..=end` selected and scrolled into view.
    pub fn open_workspace_file_at(
        &mut self,
        path: &str,
        preview: bool,
        lines: Option<(u32, u32)>,
        tx: &mut Tx,
    ) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        self.open_workspace_file_for_session(&sid, path, preview, lines, true, tx);
    }

    pub fn open_workspace_file_for_session(
        &mut self,
        sid: &str,
        path: &str,
        preview: bool,
        lines: Option<(u32, u32)>,
        steal_focus: bool,
        tx: &mut Tx,
    ) {
        let saved_focus = (self.composer.focused, self.side.focused);
        self.ensure_file_loaded(sid, path);
        let tab_key = file_tab_key(path);
        // Remove transient `file:open` tab once a concrete file is picked (`file/renderer.tsx:128`).
        if let Some(st) = self.side.sessions.get_mut(sid) {
            st.tab_state.tabs.all.retain(|k| k != OPEN_TAB_KEY);
        }
        self.show_pane_tab(tx, |s| s.open_side_tab_for_session(sid, &tab_key, Some(SessionPane::Side), preview, false));
        if !steal_focus {
            (self.composer.focused, self.side.focused) = saved_focus;
        }
        let ws = self.files.sessions.entry(sid.to_owned()).or_default();
        ws.push_recent(path);
        // Ensure parent folders in the tree are expanded so the active file highlights in context.
        let parts: Vec<&str> = path.split('/').collect();
        for i in 0..parts.len().saturating_sub(1) {
            ws.expanded_dirs.insert(parts[..=i].join("/"));
        }
        ws.rebuild_tree();
        ws.tree.select(path);

        if let Some((start, end)) = lines {
            if let Some(doc) = ws.docs.get_mut(path)
                && doc.ready
            {
                let scroll_key = format!("file-code-{sid}-{path}-scroll");
                doc.scroll_to_lines(&scroll_key, start, end, tx);
            } else {
                ws.pending_jump.insert(path.to_owned(), (start, end));
            }
        }
    }

    pub fn filtered_sidebar_files(&self, session_id: &str) -> Vec<String> {
        let q = self.files.filter.text.trim();
        let Some(ws) = self.files.sessions.get(session_id) else {
            return Vec::new();
        };
        let mut pool: Vec<String> = ws.search_found.clone();
        for f in &ws.known_files {
            if !pool.contains(f) {
                pool.push(f.clone());
            }
        }
        rank_file_query(&pool, q)
    }

    pub fn submit_files_filter(&mut self, tx: &mut Tx) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let filtered = self.filtered_sidebar_files(&sid);
        let pick = self
            .files
            .sessions
            .get(&sid)
            .and_then(|w| w.highlighted_filter_path.clone())
            .filter(|p| filtered.contains(p))
            .or_else(|| filtered.first().cloned());
        if let Some(path) = pick {
            self.files.filter.focused = false;
            self.open_workspace_file(&path, false, None, tx);
        }
    }

    pub fn request_sidebar_file_search(&mut self) {
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        let q = self.files.filter.text.trim().to_owned();
        if q.is_empty() {
            return;
        }
        let Source::Live { server, .. } = &self.source else { return };
        let ws = self.files.sessions.entry(sid.clone()).or_default();
        if ws.search_asked.as_deref() == Some(q.as_str()) {
            return;
        }
        ws.search_asked = Some(q.clone());
        let directory = self
            .session(&sid)
            .map(|s| s.directory.clone())
            .unwrap_or_default();
        let srv = server.clone();
        self.files
            .find_fetch
            .request((sid, directory, q), move |(sid, dir, q)| {
                let found = fetch_live_find(&srv, &dir, &q);
                (sid, q, found)
            });
    }
}
