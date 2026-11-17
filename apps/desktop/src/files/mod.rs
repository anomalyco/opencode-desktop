//! Files panel (`packages/gui-extensions/src/file/`), registered on `panels::PANELS` (`⌘P` opens
//! the palette, `switcher.rs`, which searches the folder's files)
//! and sharing [`CodeViewer`](crate::code_view::CodeViewer) with the Review panel's `code_view` engine.

pub mod code_paths;
pub(crate) mod content;
pub mod data;
pub(crate) mod delimited;
mod table_view;
mod label_find;
mod html;
mod mermaid;
pub mod image;
pub mod link_tip;
mod link_tip_stories;
pub mod links;
pub mod markdown;
pub mod narrow;
pub(crate) mod trace;
pub(crate) mod watch;
pub(crate) mod path_index;
mod markdown_stories;
mod stories;
pub mod video;
pub mod view;
pub(crate) mod viewer;

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use rune::{Key, KeyEvent, Latest, TextEdit, Tx, Waker};
use rune_kit::{FileNode, TreeModel};

use crate::keymap::Bind;
use crate::code_view::CodeDocState;
use crate::panels::SessionPane;
use crate::{App, Source};
use data::{
    OPEN_TAB_ID, OPEN_TAB_KEY, build_workspace_tree, fetch_live_dir,
    fetch_live_find, file_tab_key, file_tab_path, load_fixture_workspace, rank_file_query,
};

pub struct SessionFiles {
    pub docs: HashMap<String, CodeDocState>,
    pub known_files: BTreeSet<String>,
    pub(crate) paths: path_index::LivePaths,
    pub known_dirs: BTreeSet<String>,
    pub expanded_dirs: BTreeSet<String>,
    pub listed_dirs: BTreeSet<String>,
    pub tree: TreeModel<FileNode<bool>>,
    pub recent: Vec<String>,
    pub highlighted_filter_path: Option<String>,
    /// The server's search for the filter's query.
    pub search: Search,
    pub pending_jump: HashMap<String, (u32, u32)>,
    /// Files a link asked for, opened once they load.
    pub link_opens: HashMap<String, LinkOpen>,
    pub md: HashMap<String, markdown::MdView>,
    /// Decoded CSV/TSV documents. A failed or replaced read drops the previous revision.
    pub(crate) tables: HashMap<String, std::sync::Arc<delimited::Table>>,
    table_find: label_find::TableFind,
    html: HashMap<String, std::sync::Arc<html::Page>>,
    html_find: label_find::LabelFind<std::sync::Arc<html::Page>>,
    diagram_find: label_find::DiagramFind,
    diagram: mermaid::Stage,
    /// Each Markdown file's inline code paths, checked (`code_paths`).
    pub code_paths: HashMap<String, code_paths::CodePaths>,
    /// Picture files (and SVGs) as decoded images, with their zoom.
    pub images: HashMap<String, image::FileImage>,
    /// Video files, each with its player and controls.
    pub videos: HashMap<String, video::FileVideo>,
    pub ready: bool,
    /// What the narrow window's Files view shows (`narrow`).
    pub narrow: narrow::NarrowFiles,
}

/// The server's search for the files a filter names, as Desktop's
/// gui-plugins/src/file/search-machine.ts (FileSearchState): idle, searching a query (the
/// last results stay listed meanwhile, as its previous), or that query's results.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Search {
    #[default]
    Idle,
    Searching { query: String, previous: Vec<String> },
    Ready { query: String, files: Vec<String> },
}

impl Search {
    /// The query asked last, answered or not.
    pub fn asked(&self) -> Option<&str> {
        match self {
            Search::Idle => None,
            Search::Searching { query, .. } | Search::Ready { query, .. } => Some(query),
        }
    }

    /// The files found: the last answer, while the next query loads too.
    pub fn found(&self) -> &[String] {
        match self {
            Search::Idle => &[],
            Search::Searching { previous, .. } => previous,
            Search::Ready { files, .. } => files,
        }
    }

    /// Asks for query; false when it was asked already.
    fn ask(&mut self, query: &str) -> bool {
        if self.asked() == Some(query) {
            return false;
        }
        let previous = std::mem::take(self).found().to_vec();
        *self = Search::Searching { query: query.to_owned(), previous };
        true
    }

    /// The answer to query; an answer to an earlier query is stale. True when it landed.
    fn answered(&mut self, query: &str, files: Vec<String>) -> bool {
        if !matches!(self, Search::Searching { query: q, .. } if q == query) {
            return false;
        }
        *self = Search::Ready { query: query.to_owned(), files };
        true
    }
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
            paths: Default::default(),
            known_dirs: BTreeSet::new(),
            expanded_dirs: BTreeSet::new(),
            listed_dirs: BTreeSet::new(),
            tree: TreeModel::new().key("files-tree"),
            recent: Vec::new(),
            highlighted_filter_path: None,
            search: Search::Idle,
            pending_jump: HashMap::new(),
            link_opens: HashMap::new(),
            md: HashMap::new(),
            tables: HashMap::new(),
            table_find: Default::default(),
            html: HashMap::new(),
            html_find: Default::default(),
            diagram_find: Default::default(),
            diagram: Default::default(),
            code_paths: HashMap::new(),
            images: HashMap::new(),
            videos: HashMap::new(),
            ready: false,
            narrow: narrow::NarrowFiles::default(),
        }
    }
}

impl SessionFiles {
    /// A reread starts with no rendered document. Drop the projection at the same time as the
    /// source: two revisions with the same path, dimensions and query can have different hits.
    pub(crate) fn invalidate_document(&mut self, path: &str) {
        self.close_preview(path);
        self.docs.remove(path);
        if let Some(previous) = self.tables.remove(path) { self.table_find.forget(&previous); }
        if let Some(previous) = self.html.remove(path) { self.html_find.forget(&previous); }
        self.md.remove(path);
    }

    /// Only a loaded, shown table is a Find target. A table in the source view keeps CodeViewer's
    /// ordinary selection, scroll and match ranges.
    pub(super) fn shown_table(&self, path: &str) -> Option<&std::sync::Arc<delimited::Table>> {
        if self.md.get(path).and_then(|v| v.rendered) == Some(false) { return None; }
        self.tables.get(path).filter(|_| self.docs.get(path).is_some_and(|d| d.ready() && d.failure().is_none() && !d.binary() && !d.missing()))
    }

    /// (Active index, count) for both the visible bar and its polite announcement.
    pub(crate) fn file_matches(&self, path: &str, doc: &CodeDocState) -> (usize,usize) {
        if let Some(page) = self.shown_html(path) { self.html_find.count(page) }
        else if let Some(table) = self.shown_table(path) { self.table_find.count(table) }
        else if let Some(drawing) = self.shown_diagram(path) { self.diagram_find.count(&drawing) }
        else if markdown::shows_rendered(self, path) { self.md.get(path).map_or((0,0), |v| (v.active,v.matches.len())) }
        else { (doc.active_match_index(),doc.match_count()) }
    }

    pub(crate) fn close_preview(&mut self, path: &str) {
        if let Some(doc) = self.docs.get(path) && let Some(drawing) = self.diagram.current(doc) {
            self.diagram_find.forget(&drawing);
            self.diagram.clear(doc);
        }
        if let Some(table) = self.tables.get(path) { self.table_find.forget(table); }
        if let Some(page) = self.html.get(path) { self.html_find.forget(page); }
        self.md.remove(path);
        if let Some(doc) = self.docs.get_mut(path) { doc.set_find_open(false); }
    }

    fn shown_diagram(&self, path: &str) -> Option<std::rc::Rc<mermaid::Diagram>> {
        if self.md.get(path).and_then(|v| v.rendered) == Some(false) { return None; }
        self.diagram.current(self.docs.get(path)?)
    }

    fn shown_html(&self, path: &str) -> Option<&std::sync::Arc<html::Page>> {
        if !markdown::shows_rendered(self, path) { return None; }
        self.html.get(path)
    }

    fn previewable(&self, path: &str) -> bool {
        self.docs.get(path).is_some_and(|d| d.ready() && !d.missing() && !d.binary() &&
            (markdown::is_markdown(path) || image::is_svg(path) || self.html.contains_key(path) || self.tables.contains_key(path) || self.diagram.current(d).is_some()))
    }

    pub(crate) fn local_files(&self, q: &str, limit: usize) -> Vec<String> {
        self.paths.query(q, limit)
    }

    /// The known files changed: their index builds again, off the UI thread.
    pub(crate) fn reindex(&mut self, waker: Option<&Waker>) {
        if let Some(w) = waker {
            self.paths.set_waker(w.clone());
        }
        self.paths.changed(self.known_files.iter().cloned().collect());
    }
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
    replies: code_paths::Replies,
    /// Last count read for the active find bar; don't repeat it for every typed character.
    spoken_match: Option<(String, String, usize, usize)>,
    pub sessions: HashMap<String, SessionFiles>,
    pub empty_doc: CodeDocState,
    pub filter: TextEdit,
    pub dirty: bool,
    pub wrap: markdown::WrapPrefs,
    dir_fetch: Latest<(String, String, Vec<(String, bool)>)>,
    /// Each file's reads (`content`).
    pub(crate) reads: content::Reads,
    /// The server watches the open files hold (`watch`).
    pub(crate) watches: watch::Watches,
    /// The file tab shown last, so a tab that comes to show arms its watch again.
    shown: Option<(String, String)>,
    /// Folders listed again after a file in them went, each on its own worker.
    relists: (std::sync::mpsc::Sender<(String, String, Vec<(String, bool)>)>, std::sync::mpsc::Receiver<(String, String, Vec<(String, bool)>)>),
    find_fetch: Latest<(String, String, Vec<String>)>,
    /// Decodes picture files, once one opens.
    images: Option<rune::ImageCache>,
    waker: Option<Waker>,
    /// The zoom a picture's stage acts on while no picture is open.
    pub spare_zoom: rune_kit::StageZoom,
    /// The open picture's stage, as last laid out.
    pub stage_size: std::rc::Rc<std::cell::Cell<[f32; 2]>>,
}

impl Default for FilesPane {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            replies: Default::default(),
            spoken_match: None,
            empty_doc: CodeDocState::default(),
            filter: TextEdit::new(""),
            dirty: false,
            wrap: markdown::WrapPrefs::default(),
            dir_fetch: Latest::default(),
            reads: Default::default(),
            watches: Default::default(),
            shown: None,
            relists: std::sync::mpsc::channel(),
            find_fetch: Latest::default().debounce(Duration::from_millis(100)),
            images: None,
            waker: None,
            spare_zoom: rune_kit::StageZoom::default(),
            stage_size: Default::default(),
        }
    }
}

impl FilesPane {
    pub fn reply_version(&self, sid: &str) -> u64 { self.replies.version(sid) }

    pub fn retain_reply_links(&self, keep: impl Fn(&str) -> bool) { self.replies.retain(keep); }

    pub fn set_waker(&mut self, waker: Waker) {
        self.dir_fetch.set_waker(waker.clone());
        self.reads.set_waker(waker.clone());
        self.find_fetch.set_waker(waker.clone());
        self.waker = Some(waker);
    }

    /// `bytes` decoding on the image cache's workers.
    fn picture(&mut self, bytes: Vec<u8>, svg: bool) -> image::FileImage {
        let waker = self.waker.clone().unwrap_or_else(|| std::sync::Arc::new(|| {}));
        image::FileImage::new(self.images.get_or_insert_with(|| rune::ImageCache::new(waker)), bytes, svg)
    }
}

/// Landing a positive path check changes a visible reply's plain code to a link.
pub(crate) fn poll_reply_links(s: &App, tx: &mut Tx) -> bool {
    if !s.files.replies.land(s.active_session_id()) { return false; }
    rune::Tracked::touch(&s.files);
    tx.instant();
    true
}

/// Lands the folder listings, file loads and finds the panel asked for.
pub(crate) fn poll_files(s: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    for ws in s.files.sessions.values_mut() {
        changed |= ws.paths.poll();
    }
    if let Some((session_id, rel_dir, entries)) = s.files.dir_fetch.take() {
        tx.instant();
        let waker = s.files.waker.clone();
        let ws = s.files.sessions.entry(session_id).or_default();
        ws.listed_dirs.insert(rel_dir);
        let mut added = false;
        for (path, is_dir) in entries {
            if is_dir {
                ws.known_dirs.insert(path);
            } else {
                added |= ws.known_files.insert(path);
            }
        }
        if added {
            ws.reindex(waker.as_ref());
        }
        ws.ready = true;
        ws.rebuild_tree();
        changed = true;
    }
    // A folder listed again after one of its open files went: what's gone leaves the tree.
    while let Ok((session_id, rel_dir, entries)) = s.files.relists.1.try_recv() {
        tx.instant();
        let waker = s.files.waker.clone();
        let Some(ws) = s.files.sessions.get_mut(&session_id) else { continue };
        let parent = |p: &str| p.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
        let listed: BTreeSet<&str> = entries.iter().map(|(p, _)| p.as_str()).collect();
        let before = ws.known_files.len();
        ws.known_files.retain(|f| parent(f) != rel_dir || listed.contains(f.as_str()));
        ws.known_dirs.retain(|d| parent(d) != rel_dir || listed.contains(d.as_str()));
        let mut moved = ws.known_files.len() != before;
        for (path, is_dir) in entries {
            if is_dir {
                ws.known_dirs.insert(path);
            } else {
                moved |= ws.known_files.insert(path);
            }
        }
        if moved {
            ws.reindex(waker.as_ref());
        }
        ws.rebuild_tree();
        changed = true;
    }
    while let Some(landed) = s.files.reads.take() {
        changed |= land_read(s, landed, tx);
    }
    if let Some((session_id, query, found)) = s.files.find_fetch.take() {
        if let Some(ws) = s.files.sessions.get_mut(&session_id)
            && ws.search.answered(&query, found)
        {
            tx.instant();
            changed = true;
        }
    }
    // A picture's size is known once it decodes: its stage lays out again.
    if s.files.sessions.values().any(|ws| ws.images.values().any(image::FileImage::lands)) {
        for ws in s.files.sessions.values_mut() {
            changed |= ws.images.values_mut().fold(false, |any, i| i.landed() | any);
        }
    }
    changed |= video::poll(s, tx);
    if s.files.dirty {
        s.files.dirty = false;
        if let Some(sid) = s.active_session_id().map(str::to_owned) {
            s.ensure_workspace_files(&sid);
            changed = true;
        }
    }
    // A file tab that comes to show arms its watch again (Desktop's tab `focus` syncs it).
    let shown = s.active_file_tab().and_then(|(sid, path)| Some((sid, path?)));
    if shown != s.files.shown {
        s.files.shown = shown.clone();
        if let Some((sid, path)) = shown {
            watch::watch(s, &sid, &path);
        }
    }
    // Sync active file's live find query if open.
    if let Some((sid, Some(path))) = s.active_file_tab()
        && let Some(ws) = s.files.sessions.get_mut(&sid)
        && let Some(doc) = ws.docs.get_mut(&path)
        && doc.find_open()
    {
        if (!ws.html.contains_key(&path) && !ws.tables.contains_key(&path) && ws.diagram.current(doc).is_none()) || ws.md.get(&path).and_then(|v| v.rendered) == Some(false) { changed |= doc.sync_find_query(); }
        changed |= s.sync_rendered_find(&sid, &path, tx);
        let ws = &s.files.sessions[&sid];
        let doc = &ws.docs[&path];
        if tx.assistive_technology() && !doc.find_input.text.is_empty() {
            let (at, count) = ws.file_matches(&path, doc);
            let result = (sid, path, at, count);
            if s.files.spoken_match.as_ref() != Some(&result) {
                let words = if count == 0 { opencode_i18n::t!("session.search.noResults").to_owned() }
                    else { opencode_i18n::t!("rune.a11y.find.match", at = at + 1, count = count) };
                tx.announce(words, rune::LiveRegion::Polite);
                s.files.spoken_match = Some(result);
            }
        } else { s.files.spoken_match = None; }
    } else {
        s.files.spoken_match = None;
    }
    changed
}

/// A read that came back (`content`): shown in place of what showed, keeping where the view is,
/// unless a newer read superseded it. A refresh that fails keeps the text that shows, with no
/// error, and reads again on the next ask (Desktop's `stale`); a failure or a missing file that
/// a later change already outdated isn't shown at all: the next read is on its way.
fn land_read(s: &mut App, landed: content::Landed, tx: &mut Tx) -> bool {
    let (sid, path) = (landed.sid.clone(), landed.path.clone());
    let again = match s.files.reads.land(&landed) {
        content::ReadEffect::Show => None,
        content::ReadEffect::ShowThenStart { attempt } => Some(attempt),
        _ => return false,
    };
    if let Some(attempt) = again {
        s.start_read(&sid, &path, attempt);
    }
    let body = landed.body;
    let shows_text = s.files.sessions.get(&sid).and_then(|w| w.docs.get(&path)).is_some_and(|d| d.ready() && d.failure().is_none() && !d.missing() && !d.binary());
    if !body.content() && (again.is_some() || (shows_text && matches!(body, content::Body::Failed(_)))) {
        if again.is_none() {
            s.files.reads.outdate(&sid, &path);
        }
        return false;
    }
    tx.instant();
    let swap = rune::time::Instant::now();
    let server_label = crate::servers::runtime::label(crate::servers::runtime::active(s));
    let wrap = s.files.wrap;
    let picture = match &body {
        content::Body::Image(bytes) => Some(s.files.picture(bytes.clone(), false)),
        content::Body::Svg(_, bytes) => Some(s.files.picture(bytes.clone(), true)),
        _ => None,
    };
    let ws = s.files.sessions.entry(sid.clone()).or_default();
    match picture {
        // A picture read again keeps its zoom.
        Some(mut p) => {
            if let Some(old) = ws.images.remove(&path) { p.zoom = old.zoom; }
            ws.images.insert(path.clone(), p);
        }
        None => { ws.images.remove(&path); }
    }
    ws.md.remove(&path);
    if let Some(previous) = ws.tables.remove(&path) { ws.table_find.forget(&previous); }
    if let Some(previous) = ws.html.remove(&path) { ws.html_find.forget(&previous); }
    let doc = ws.docs.entry(path.clone()).or_insert_with(|| wrap.doc(&path, ""));
    let lines = match body {
        content::Body::Text(text) | content::Body::Svg(text, _) => { let n = text.lines(); doc.refresh(text); n }
        content::Body::Html(text, document) => {
            let n = text.lines();
            doc.refresh(text);
            ws.html.insert(path.clone(), document);
            n
        }
        content::Body::Table(text, document) => {
            let n = text.lines();
            doc.refresh(text);
            ws.tables.insert(path.clone(), document);
            n
        }
        content::Body::Binary(sz) => { doc.set_binary(sz); 0 }
        content::Body::Image(bytes) => { doc.set_binary(bytes.len()); 0 }
        content::Body::Missing => { doc.set_missing(); 0 }
        content::Body::Failed(wire) => {
            let what = opencode_i18n::t!("rune.file.readFailed", path = opencode_kit::path::basename(&path)).to_owned();
            let words = crate::errors::UserError::from_wire(&what, &server_label, &wire).description();
            doc.set_failed(what, words);
            0
        }
    };
    let bytes = doc.size_bytes();
    if let Some((start, end)) = ws.pending_jump.remove(&path) {
        let scroll_key = format!("file-code-{sid}-{path}-scroll");
        doc.scroll_to_lines(&scroll_key, start, end, tx);
    }
    if let Some(link) = ws.link_opens.remove(&path) {
        s.finish_file_link(&sid, &path, link, tx);
    }
    s.check_code_paths(&sid, &path);
    trace::landed(&sid, &path, landed.attempt, landed.times, swap, bytes, lines);
    true
}

/// Keys for the open file while a Files tab shows: its find, selection and copy.
pub(crate) fn key_files(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some((sid, active_path)) = s.active_file_tab() else {
        return false;
    };

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
            if doc.find_open() && doc.find_input.focused && !doc.find_input.is_composing() && ev.key == Key::Enter {
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
            Some((reg.session_id.clone(), None))
        } else {
            file_tab_path(id).map(|p| (reg.session_id.clone(), Some(p)))
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
        match &*self.source {
            Source::Fixture => {
                let (files, contents) = load_fixture_workspace(&self.fixture);
                let wrap = self.files.wrap;
                let waker = self.files.waker.clone();
                let ws = self.files.sessions.entry(session_id.to_owned()).or_default();
                ws.known_files = files;
                ws.reindex(waker.as_ref());
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
                matches!(*self.source, Source::Live { .. }) && !ws.listed_dirs.contains(dir_path)
            }
        };
        if need_fetch && let Source::Live { server, .. } = &*self.source {
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

    /// Lists `sid`'s folder `dir` again (a file in it went), on a worker of its own, so it never
    /// takes the place of a folder the person just opened.
    pub(crate) fn relist_workspace_dir(&mut self, sid: &str, dir: &str) {
        let Source::Live { server, .. } = &*self.source else { return };
        let (server, sid, rel) = (server.clone(), sid.to_owned(), dir.to_owned());
        let directory = self.session(&sid).map(|s| s.directory.clone()).unwrap_or_default();
        let (sender, waker) = (self.files.relists.0.clone(), self.files.waker.clone());
        let _ = rune::task::spawn("file-relist", move || {
            let Some(entries) = data::try_fetch_live_dir(&server, &directory, &rel) else { return };
            if sender.send((sid, rel, entries)).is_ok()
                && let Some(w) = waker
            {
                w();
            }
        });
    }

    pub fn ensure_file_loaded(&mut self, session_id: &str, path: &str) {
        self.ensure_workspace_files(session_id);
        let already = self
            .files
            .sessions
            .get(session_id)
            .and_then(|w| w.docs.get(path))
            // One still loading asks again (it joins the read in flight), one whose read failed
            // reads again, and one a change left stale reads again.
            .is_some_and(|d| d.ready() && d.failure().is_none())
            && !self.files.reads.stale(session_id, path);
        if already {
            return;
        }
        match &*self.source {
            Source::Fixture => {
                let (_, contents) = load_fixture_workspace(&self.fixture);
                let wrap = self.files.wrap;
                let waker = self.files.waker.clone();
                let ws = self.files.sessions.entry(session_id.to_owned()).or_default();
                if ws.known_files.insert(path.to_owned()) { ws.reindex(waker.as_ref()); }
                let mut doc = wrap.doc(path, "");
                ws.invalidate_document(path);
                if let Some(txt) = contents.get(path) {
                    doc.set_text(txt);
                    if let Some(table) = delimited::Table::parse(path, txt) { ws.tables.insert(path.to_owned(), table.into()); }
                    if let Some(page) = html::Page::parse(path, txt) { ws.html.insert(path.to_owned(), page); }
                } else {
                    doc.set_missing();
                }
                ws.docs.insert(path.to_owned(), doc);
                ws.rebuild_tree();
            }
            Source::Live { server, .. } => {
                let wrap = self.files.wrap;
                let directory = self
                    .session(session_id)
                    .map(|s| s.directory.clone())
                    .unwrap_or_default();
                // A video streams into its player where it lies: nothing is read first. One that
                // can't play is read as any other file, and shows as the binary file it is.
                let failed = self.files.sessions.get(session_id).and_then(|w| w.videos.get(path)).is_some_and(video::FileVideo::failed);
                if video::is_video(path) && !failed {
                    let source = if server.is_local() {
                        std::path::Path::new(&directory).join(path).to_string_lossy().into_owned()
                    } else {
                        server.stream_url(&format!("/api/fs/read/{}?location%5Bdirectory%5D={}", crate::review::data::encode_fs_path(path), crate::switcher::encode(&directory)))
                    };
                    let clip = video::FileVideo::new(&source, self.files.waker.clone());
                    let ws = self.files.sessions.entry(session_id.to_owned()).or_default();
                    let mut doc = wrap.doc(path, "");
                    doc.set_binary(0);
                    ws.docs.insert(path.to_owned(), doc);
                    ws.videos.insert(path.to_owned(), clip);
                    return;
                }
                // A stale file keeps its text while it reads again; a failed one shows loading.
                let ws = self.files.sessions.entry(session_id.to_owned()).or_default();
                if !ws.docs.get(path).is_some_and(|d| d.ready() && d.failure().is_none()) {
                    ws.docs.insert(path.to_owned(), wrap.doc(path, ""));
                }
                self.read_file(session_id, path, false);
            }
        }
    }

    /// Reads `path` for `sid` unless a read runs already (`content`); `force` reads even when it
    /// has loaded (it changed on disk, or while the server was away). Live servers only.
    pub(crate) fn read_file(&mut self, sid: &str, path: &str, force: bool) {
        if !matches!(*self.source, Source::Live { .. }) {
            return;
        }
        let loaded = self.files.sessions.get(sid).and_then(|w| w.docs.get(path)).is_some_and(|d| d.ready());
        if let Some(attempt) = self.files.reads.ask(sid, path, force, loaded) {
            self.start_read(sid, path, attempt);
        }
    }

    pub(crate) fn start_read(&mut self, sid: &str, path: &str, attempt: u64) {
        let Source::Live { server, .. } = &*self.source else { return };
        let server = server.clone();
        let directory = self.session(sid).map(|s| s.directory.clone()).unwrap_or_default();
        let find = self.files.sessions.get(sid).and_then(|w| w.docs.get(path)).and_then(|d| d.find_query());
        self.files.reads.start(&server, &directory, sid, path, attempt, find);
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
        watch::watch(self, sid, path);
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
        ws.narrow = ws.narrow.step(narrow::NarrowFilesEvent::Opened);
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
                && doc.ready()
            {
                let scroll_key = format!("file-code-{sid}-{path}-scroll");
                doc.scroll_to_lines(&scroll_key, start, end, tx);
            } else {
                ws.pending_jump.insert(path.to_owned(), (start, end));
            }
        }
    }

    pub fn filtered_sidebar_files(&self, session_id: &str) -> Vec<String> {
        self.filtered_files(session_id, &self.files.filter.text)
    }

    /// The files of `session_id`'s workspace whose path holds `query`, best first: the server's
    /// finds, then the files the tree has listed.
    pub fn filtered_files(&self, session_id: &str, query: &str) -> Vec<String> {
        let q = query.trim();
        let Some(ws) = self.files.sessions.get(session_id) else {
            return Vec::new();
        };
        let mut pool: Vec<String> = ws.search.found().to_vec();
        let (clean, _) = data::parse_path_and_line(q);
        let local = ws.local_files(clean.trim(), 100);
        if pool.is_empty() { return local; }
        for f in local {
            if !pool.contains(&f) {
                pool.push(f);
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
        let q = self.files.filter.text.clone();
        self.request_file_search(&sid, &q);
    }

    /// Asks the server for `session_id`'s files that match `query`, once per query.
    pub fn request_file_search(&mut self, session_id: &str, query: &str) {
        let (sid, q) = (session_id.to_owned(), query.trim().to_owned());
        if q.is_empty() {
            return;
        }
        let Source::Live { server, .. } = &*self.source else { return };
        let ws = self.files.sessions.entry(sid.clone()).or_default();
        if !ws.search.ask(&q) {
            return;
        }
        let directory = self
            .session(&sid)
            .map(|s| s.directory.clone())
            .unwrap_or_default();
        let srv = server.clone();
        self.files
            .find_fetch
            .request((sid, directory, q), move |(sid, dir, q)| {
                let found = fetch_live_find(&srv, &dir, &q, 100);
                (sid, q, found)
            });
    }
}
