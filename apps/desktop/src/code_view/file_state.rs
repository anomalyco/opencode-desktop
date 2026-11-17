//! State, line selection, and in-file search (`⌘F`) for [`CodeViewer`](super::CodeViewer).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use rune::{Key, KeyEvent, TextEdit, Tx};
use rune_kit::line_selection::{Column, LineEvent, LineSelection, Lines};
use rune_kit::syntax::Paint;

use super::{CHUNK_LINES, ChunkMap, HighlightChunk, LINE_H, SourceLines, lang_for_path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileLineSelection {
    /// 1-based start line.
    pub start: u32,
    /// 1-based end line.
    pub end: u32,
}

impl From<Lines<u32>> for FileLineSelection {
    fn from(l: Lines<u32>) -> FileLineSelection {
        FileLineSelection { start: l.start, end: l.end }
    }
}

impl From<FileLineSelection> for Lines<u32> {
    fn from(s: FileLineSelection) -> Lines<u32> {
        Lines { start: s.start, end: s.end }
    }
}

/// What the document holds: Desktop's `workspaces/files/content-machine.ts` `FileEntryState`
/// (loading, ready, missing), a ready file being text or binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Loading,
    Text { size: usize },
    Binary { size: usize },
    Missing,
}

impl FileLineSelection {
    pub fn ordered(self) -> (u32, u32) {
        (self.start.min(self.end), self.start.max(self.end))
    }

    pub fn contains(self, line: u32) -> bool {
        let (a, b) = self.ordered();
        (a..=b).contains(&line)
    }
}

/// Shared code-document state rendered by [`CodeViewer`](super::CodeViewer).
pub struct CodeDocState {
    path: String,
    source: SourceLines,
    content: Content,
    wrap: bool,
    /// The text ends with a newline: the empty line after it shows numbered.
    trailing: bool,
    digits: u32,
    /// The selected lines and the gesture on the line numbers making them (rune-kit's shared machine).
    lines: LineSelection<u32>,
    chunks: RefCell<ChunkMap<u32>>,
    pub find_input: TextEdit,
    find: DocFind,
    /// The text's folded lines for Find, folded on a worker when Find opens and dropped when
    /// Find closes or the text changes: a keystroke scans it, never folds the file.
    slab: Slab,
    /// `(1-based line, byte_range_in_line)`
    matches: Vec<(u32, Range<usize>)>,
    by_line_matches: HashMap<u32, Vec<(Range<usize>, bool)>>,
    view_w: Cell<f32>,
    /// The rows' laid-out heights, for a drag down the numbers.
    pub(crate) heights: super::RowHeights,
    pub(crate) rev: u64,
    pub(crate) keys_rev: u64,
    /// Why it couldn't be read, when that's what it shows: a headline and the reason in words.
    failure: Option<(String, String)>,
    /// The line (1-based) the keyboard's line cursor is on: the view's one Tab stop, which Up
    /// and Down move (a roving tab index).
    pub(super) cursor: u32,
}

impl Default for CodeDocState {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl std::fmt::Debug for CodeDocState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeDocState")
            .field("path", &self.path)
            .field("lines", &self.source.len())
            .finish()
    }
}

impl CodeDocState {
    pub fn new(path: impl Into<String>, content: &str) -> Self {
        let mut s = Self {
            path: path.into(),
            source: SourceLines::default(),
            content: Content::Loading,
            wrap: false,
            trailing: false,
            digits: 3,
            lines: LineSelection::default(),
            chunks: RefCell::new(HashMap::new()),
            find_input: TextEdit::new(""),
            find: DocFind::Closed,
            slab: Slab::Absent,
            matches: Vec::new(),
            by_line_matches: HashMap::new(),
            view_w: Cell::new(0.0),
            heights: super::RowHeights::default(),
            rev: 1,
            keys_rev: 1,
            failure: None,
            cursor: 1,
        };
        if !content.is_empty() {
            s.set_text(content);
        }
        s
    }

    pub fn loading(path: impl Into<String>) -> Self {
        Self::new(path, "")
    }

    pub fn set_text(&mut self, content: &str) {
        self.failure = None;
        let size = content.len();
        let binary = content.contains('\0');
        self.content = if binary { Content::Binary { size } } else { Content::Text { size } };
        self.source = if binary {
            SourceLines::default()
        } else {
            SourceLines::new(content)
        };
        // Pierre numbers the empty line after a file's last newline too.
        self.trailing = !binary && content.ends_with('\n');
        self.digits = (self.line_count().max(1).ilog10() + 1).max(3);
        self.chunks.borrow_mut().clear();
        self.heights.clear();
        self.slab = Slab::Absent;
        self.prepare_slab();
        self.recompute_matches();
        self.bump_keys();
    }

    /// A read's text, split (and folded and searched for Find when it was open) off the UI
    /// thread (`Prepared::new`), in place of what shows: as Desktop's reload, the selected
    /// lines and the scroll stay, and Find keeps its query and lights the first match again
    /// (`pierre/file-find-machine.ts`, `refresh.requested` with `reset`), without scrolling.
    pub fn refresh(&mut self, prepared: Prepared) {
        self.failure = None;
        self.content = Content::Text { size: prepared.source.text().len() };
        self.trailing = prepared.source.text().ends_with('\n');
        self.source = prepared.source;
        self.digits = (self.line_count().max(1).ilog10() + 1).max(3);
        self.cursor = self.cursor.min(self.lines().max(1));
        self.chunks.borrow_mut().clear();
        self.heights.clear();
        self.find.restart();
        match prepared.find {
            Some(find) if self.find_open() => {
                let same = find.query == self.find.query();
                self.slab = Slab::Ready(find.slab);
                if same {
                    self.matches = find.matches;
                    self.rebuild_line_match_index();
                } else {
                    // The query changed during the read: found again as a keystroke finds.
                    self.recompute_matches();
                }
            }
            _ => {
                self.slab = Slab::Absent;
                self.prepare_slab();
                self.recompute_matches();
            }
        }
        self.bump_keys();
    }

    /// What `refresh` needs for Find: the query its matches are for, when Find is open.
    pub fn find_query(&self) -> Option<String> {
        self.find_open().then(|| self.find.query().to_owned())
    }

    pub fn set_binary(&mut self, size_bytes: usize) {
        self.failure = None;
        self.content = Content::Binary { size: size_bytes };
        self.slab = Slab::Absent;
        self.source = SourceLines::default();
        self.trailing = false;
        self.bump_keys();
    }

    pub fn set_missing(&mut self) {
        self.failure = None;
        self.content = Content::Missing;
        self.slab = Slab::Absent;
        self.source = SourceLines::default();
        self.trailing = false;
        self.bump_keys();
    }

    pub fn content(&self) -> Content {
        self.content
    }

    /// Loaded: text, binary or missing.
    /// Its read failed (not a missing file): shown as `what`, `why`; a reopen reads it again.
    pub fn set_failed(&mut self, what: String, why: String) {
        self.set_missing();
        self.failure = Some((what, why));
    }

    /// The failed read's headline and reason, when it failed.
    pub fn failure(&self) -> Option<&(String, String)> {
        self.failure.as_ref()
    }

    pub fn ready(&self) -> bool {
        self.content != Content::Loading
    }

    pub fn missing(&self) -> bool {
        self.content == Content::Missing
    }

    pub fn binary(&self) -> bool {
        matches!(self.content, Content::Binary { .. })
    }

    /// The loaded file's size in bytes (0 while loading or missing).
    pub fn size_bytes(&self) -> usize {
        match self.content {
            Content::Text { size } | Content::Binary { size } => size,
            Content::Loading | Content::Missing => 0,
        }
    }

    fn bump_keys(&mut self) {
        self.rev += 1;
        static KEYS: AtomicU64 = AtomicU64::new(1);
        self.keys_rev = KEYS.fetch_add(1, Ordering::Relaxed);
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The whole file.
    pub fn text(&self) -> &str {
        self.source.text()
    }

    /// Up, Down, Home, End, Page Up and Page Down on a focused line: the cursor moves, takes
    /// focus and comes into view.
    pub(super) fn line_key(&mut self, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
        if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.shift {
            return false;
        }
        let last = self.line_count().max(1) as u32;
        let at = self.cursor.clamp(1, last);
        let to = match ev.key {
            rune::Key::Down => (at + 1).min(last),
            rune::Key::Up => at.saturating_sub(1).max(1),
            rune::Key::Home => 1,
            rune::Key::End => last,
            rune::Key::PageDown => (at + 20).min(last),
            rune::Key::PageUp => at.saturating_sub(20).max(1),
            _ => return false,
        };
        tx.instant();
        self.cursor = to;
        tx.scroll_into_view(format!("cl{to}"));
        tx.focus(format!("cl{to}"), true);
        true
    }

    pub fn line_count(&self) -> usize {
        self.lines() as usize
    }

    fn lines(&self) -> u32 {
        self.source.len() + u32::from(self.trailing)
    }

    pub fn line(&self, n: u32) -> Option<&str> {
        self.source.line(n).or((self.trailing && n == self.lines()).then_some(""))
    }

    pub fn digits(&self) -> u32 {
        self.digits
    }

    pub fn columns(&self) -> u32 {
        self.source.columns()
    }

    pub fn wrap(&self) -> bool {
        self.wrap
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.heights.clear();
            self.rev += 1;
        }
    }

    pub fn selection(&self) -> Option<FileLineSelection> {
        self.lines.selected().map(FileLineSelection::from)
    }

    /// Selects `sel`, ending any press on the line numbers.
    pub fn set_selection(&mut self, sel: Option<FileLineSelection>) {
        if self.selection() != sel || self.lines.tracking() {
            self.step_lines(LineEvent::Set(sel.map(Lines::from)));
        }
    }

    /// Steps the line-selection gesture; a change of the selected lines redraws the rows.
    fn step_lines(&mut self, event: LineEvent<u32>) {
        let before = self.lines.selected();
        let _ = self.lines.step(event, |&n| Some(n));
        if self.lines.selected() != before {
            self.rev += 1;
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection()?;
        let (a, b) = sel.ordered();
        let mut out = Vec::with_capacity((b - a + 1) as usize);
        for n in a..=b {
            out.push(self.source.line(n).unwrap_or(""));
        }
        Some(out.join("\n"))
    }

    /// A press on line `line`'s number (rune-kit's shared gesture, as Pierre's line numbers).
    pub(crate) fn press_line(&mut self, line: u32, shift: bool) {
        self.step_lines(LineEvent::Press { at: line, column: Column::Numbers, shift });
    }

    /// The held pointer is `dy` points below the top of line `start_line`'s row (the drag's own
    /// coordinates): the line under it, by the rows' laid-out heights (a wrapped line, or one
    /// with comments under it, is taller than a line).
    pub(crate) fn drag_to_offset(&mut self, start_line: u32, mut dy: f32) {
        if self.source.is_empty() {
            return;
        }
        let last = self.lines();
        let mut at = start_line.clamp(1, last);
        let height = |n: u32| self.heights.get(&format!("cl{n}")).unwrap_or(LINE_H);
        while dy < 0.0 && at > 1 {
            at -= 1;
            dy += height(at);
        }
        while at < last {
            let h = height(at);
            if dy < h {
                break;
            }
            dy -= h;
            at += 1;
        }
        self.step_lines(LineEvent::Move { at });
    }

    /// The one selected line, while a press on its number waits to see whether it drags.
    pub(crate) fn unselecting(&self) -> Option<FileLineSelection> {
        match self.lines {
            LineSelection::Unselecting { line } => Some(line.into()),
            _ => None,
        }
    }

    /// A click on line `line`'s code: it selects that line, or clears it when it was the
    /// selection (`selection-machine.ts`, the `text` column).
    pub(crate) fn click_code(&mut self, line: u32) {
        self.step_lines(LineEvent::Press { at: line, column: Column::Text, shift: false });
        self.step_lines(LineEvent::Release);
    }

    pub(crate) fn release_line(&mut self) {
        self.step_lines(LineEvent::Release);
    }

    pub(crate) fn cancel_line(&mut self) {
        self.step_lines(LineEvent::Cancel);
    }

    pub fn scroll_to_line(&mut self, scroll_key: &str, line: u32, select: bool, tx: &mut Tx) {
        if self.source.is_empty() {
            return;
        }
        let clamped = line.clamp(1, self.lines());
        if select {
            self.set_selection(Some(FileLineSelection { start: clamped, end: clamped }));
        }
        let top_line = clamped.saturating_sub(3).max(1);
        tx.instant();
        tx.scroll_to_y(scroll_key.to_owned(), (top_line - 1) as f32 * LINE_H);
    }

    /// [`Self::scroll_to_line`] at `start`, selecting `start..=end`.
    pub fn scroll_to_lines(&mut self, scroll_key: &str, start: u32, end: u32, tx: &mut Tx) {
        self.scroll_to_line(scroll_key, start, true, tx);
        let lines = self.lines();
        if let Some(sel) = self.selection() {
            self.set_selection(Some(FileLineSelection { end: end.clamp(sel.start, lines.max(sel.start)), ..sel }));
        }
    }

    pub fn find_open(&self) -> bool {
        matches!(self.find, DocFind::Open { .. })
    }

    /// The find bar opens (focused) or closes (its query gone with it): Pierre's `file-find`.
    pub fn set_find_open(&mut self, open: bool) {
        let event = if open { DocFindEvent::Open } else { DocFindEvent::Close };
        if self.find.step(event) {
            self.slab = Slab::Absent;
            self.prepare_slab();
            self.find_input = TextEdit::new("");
            self.recompute_matches();
        }
        self.find_input.focused = open;
    }

    pub fn sync_find_query(&mut self) -> bool {
        if !self.find.step(DocFindEvent::Query(&self.find_input.text)) {
            return false;
        }
        self.recompute_matches();
        true
    }

    /// Open on a text: its slab starts folding on a worker, ready before a person's first key.
    fn prepare_slab(&mut self) {
        if self.find_open() && !self.source.is_empty() && matches!(self.slab, Slab::Absent) {
            self.slab = Slab::fold(&self.source);
        }
    }

    fn recompute_matches(&mut self) {
        self.matches.clear();
        let query = self.find.query();
        if !query.is_empty() && !self.source.is_empty() {
            let slab = self.slab.ready(&self.source);
            slab.find(query, |m| self.matches.push((m.row as u32 + 1, m.bytes)));
        }
        self.rebuild_line_match_index();
    }

    fn rebuild_line_match_index(&mut self) {
        self.by_line_matches.clear();
        let active = self.find.active();
        for (i, (line, range)) in self.matches.iter().enumerate() {
            self.by_line_matches.entry(*line).or_default().push((range.clone(), i == active));
        }
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn active_match_index(&self) -> usize {
        self.find.active()
    }

    pub fn step_match(&mut self, scroll_key: &str, forward: bool, tx: &mut Tx) {
        if !self.find.step(DocFindEvent::Step { forward, count: self.matches.len() }) {
            return;
        }
        self.rebuild_line_match_index();
        let line = self.matches[self.find.active()].0;
        self.scroll_to_line(scroll_key, line, false, tx);
    }
    pub(crate) fn line_matches(&self, line: u32) -> &[(Range<usize>, bool)] {
        self.by_line_matches.get(&line).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn highlights(&self, line: u32) -> Vec<(Range<usize>, Paint)> {
        let Some(lang) = lang_for_path(&self.path) else {
            return Vec::new();
        };
        let c = line.saturating_sub(1) / CHUNK_LINES;
        let cached = self.chunks.borrow().get(&c).cloned();
        let chunk = cached.unwrap_or_else(|| {
            let first_n = c * CHUNK_LINES + 1;
            let last_n = ((c + 1) * CHUNK_LINES).min(self.source.len().max(1));
            let built = HighlightChunk::lines(Some(lang), first_n, last_n, |m| self.source.line(m).unwrap_or(""));
            let mut cache = self.chunks.borrow_mut();
            if cache.len() > 512 {
                cache.clear();
            }
            cache.insert(c, built.clone());
            built
        });
        chunk.line_highlights((line.saturating_sub(1) % CHUNK_LINES) as usize)
    }

    pub fn handle_key(&mut self, scroll_key: &str, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if crate::keymap::Bind::Find.matches_tx(ev, tx) {
            tx.instant();
            self.set_find_open(true);
            self.find_input.set_selection(0, self.find_input.text.len());
            return true;
        }
        // Enter and Escape commit or cancel an input method's text first (IME).
        if self.find_open() && self.find_input.focused {
            if self.find_input.is_composing() {
                return false;
            }
            match &ev.key {
                Key::Escape => {
                    tx.instant();
                    self.set_find_open(false);
                    return true;
                }
                Key::Enter => {
                    self.sync_find_query();
                    self.step_match(scroll_key, !ev.mods.shift, tx);
                    return true;
                }
                _ => return false,
            }
        }
        if ev.key == Key::Escape {
            if self.find_open() {
                tx.instant();
                self.set_find_open(false);
                return true;
            }
            if self.selection().is_some() {
                tx.instant();
                self.set_selection(None);
                return true;
            }
        }
        if crate::keymap::Bind::CopySelection.matches_tx(ev, tx)
            && let Some(text) = self.selected_text()
        {
            tx.copy(text);
            return true;
        }
        false
    }

    pub(crate) fn set_viewport(&self, r: [f32; 4]) {
        self.view_w.set(r[2]);
    }

    pub(crate) fn view_width(&self) -> f32 {
        self.view_w.get()
    }
}

/// Find in a file: Pierre's `createFileFind` (`session-ui/src/pierre/file-find.ts`), closed or
/// open on the query its matches are for and the lit match.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum DocFind {
    #[default]
    Closed,
    Open { query: String, active: usize },
}

pub(crate) enum DocFindEvent<'a> {
    Open,
    Close,
    /// The field holds `0`: a new query lights its first match.
    Query(&'a str),
    /// Enter (or Shift+Enter) among `count` matches, wrapping.
    Step { forward: bool, count: usize },
}

impl DocFind {
    /// True when the matches must be found again (or, for a step, lit again).
    pub(crate) fn step(&mut self, event: DocFindEvent) -> bool {
        match (&mut *self, event) {
            (DocFind::Closed, DocFindEvent::Open) => {
                *self = DocFind::Open { query: String::new(), active: 0 };
                true
            }
            (DocFind::Open { .. }, DocFindEvent::Close) => {
                *self = DocFind::Closed;
                true
            }
            (DocFind::Open { query, active }, DocFindEvent::Query(text)) if query != text => {
                *query = text.to_owned();
                *active = 0;
                true
            }
            (DocFind::Open { active, .. }, DocFindEvent::Step { forward, count }) if count > 0 => {
                *active = if forward { (*active + 1) % count } else { (*active + count - 1) % count };
                true
            }
            _ => false,
        }
    }

    /// The text changed under an open Find: its query stays, the first match lights.
    fn restart(&mut self) {
        if let DocFind::Open { active, .. } = self {
            *active = 0;
        }
    }

    pub(super) fn query(&self) -> &str {
        match self {
            DocFind::Open { query, .. } => query,
            DocFind::Closed => "",
        }
    }

    pub(crate) fn active(&self) -> usize {
        match self {
            DocFind::Open { active, .. } => *active,
            DocFind::Closed => 0,
        }
    }
}

/// A file's text made ready for [`CodeDocState::refresh`] on the worker that read it, so the
/// UI thread does nothing proportional to the file's size: its lines split and, while Find is
/// open, its folded copy and the matches for Find's query.
pub struct Prepared {
    source: SourceLines,
    find: Option<PreparedFind>,
}

struct PreparedFind {
    query: String,
    slab: opencode_search::SlabFind,
    matches: Vec<(u32, Range<usize>)>,
}

impl Prepared {
    /// `find`: Find's query when it's open (`CodeDocState::find_query`).
    pub fn new(text: String, find: Option<String>) -> Prepared {
        let source = SourceLines::from_string(text);
        let find = find.map(|query| {
            let slab = Slab::build(&source);
            let mut matches = Vec::new();
            if !query.is_empty() {
                slab.find(&query, |m| matches.push((m.row as u32 + 1, m.bytes)));
            }
            PreparedFind { query, slab, matches }
        });
        Prepared { source, find }
    }
}

/// Find's folded copy of the text: none yet, folding on a worker (native) or ready.
enum Slab {
    Absent,
    #[cfg(not(target_arch = "wasm32"))]
    Folding(std::sync::mpsc::Receiver<opencode_search::SlabFind>),
    Ready(opencode_search::SlabFind),
}

impl Slab {
    fn build(source: &SourceLines) -> opencode_search::SlabFind {
        let mut slab = opencode_search::SlabFind::default();
        (1..=source.len()).for_each(|n| slab.push(source.line(n).unwrap_or("")));
        slab
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fold(source: &SourceLines) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let source = source.clone();
        let spawned = std::thread::Builder::new().name("file-find-fold".into()).spawn(move || {
            let _ = tx.send(Slab::build(&source));
        });
        match spawned {
            Ok(_) => Slab::Folding(rx),
            Err(_) => Slab::Absent,
        }
    }

    /// The browser has no worker for it: the first query folds.
    #[cfg(target_arch = "wasm32")]
    fn fold(_: &SourceLines) -> Self {
        Slab::Absent
    }

    /// The slab for `source`, waiting for the worker if a query came first (or folding here when
    /// none was started).
    fn ready(&mut self, source: &SourceLines) -> &opencode_search::SlabFind {
        let slab = match std::mem::replace(self, Slab::Absent) {
            Slab::Ready(slab) => slab,
            #[cfg(not(target_arch = "wasm32"))]
            Slab::Folding(rx) => rx.recv().unwrap_or_else(|_| Slab::build(source)),
            Slab::Absent => Slab::build(source),
        };
        *self = Slab::Ready(slab);
        let Slab::Ready(slab) = self else { unreachable!("set just above") };
        slab
    }
}
