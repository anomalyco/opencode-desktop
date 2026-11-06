//! State, line selection, and in-file search (`⌘F`) for [`CodeViewer`](super::CodeViewer).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use rune::{Key, KeyEvent, Modifier, TextEdit, Tx};
use rune_kit::syntax::Paint;

use super::{
    CHUNK_LINES, ChunkMap, HighlightChunk, LINE_H, SourceLines, lang_for_path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileLineSelection {
    /// 1-based start line.
    pub start: u32,
    /// 1-based end line.
    pub end: u32,
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
    pub ready: bool,
    pub missing: bool,
    pub binary: bool,
    pub size_bytes: usize,
    wrap: bool,
    digits: u32,
    selection: Option<FileLineSelection>,
    selecting: bool,
    selection_at_press: Option<FileLineSelection>,
    unselect: bool,
    shift: bool,
    chunks: RefCell<ChunkMap<u32>>,
    pub find_open: bool,
    pub find_input: TextEdit,
    find_query_applied: String,
    /// `(1-based line, byte_range_in_line)`
    matches: Vec<(u32, Range<usize>)>,
    by_line_matches: HashMap<u32, Vec<(Range<usize>, bool)>>,
    active_match: usize,
    view_w: Cell<f32>,
    pub(crate) rev: u64,
    pub(crate) keys_rev: u64,
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
            ready: false,
            missing: false,
            binary: false,
            size_bytes: 0,
            wrap: false,
            digits: 3,
            selection: None,
            selecting: false,
            selection_at_press: None,
            unselect: false,
            shift: false,
            chunks: RefCell::new(HashMap::new()),
            find_open: false,
            find_input: TextEdit::new(""),
            find_query_applied: String::new(),
            matches: Vec::new(),
            by_line_matches: HashMap::new(),
            active_match: 0,
            view_w: Cell::new(0.0),
            rev: 1,
            keys_rev: 1,
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
        self.size_bytes = content.len();
        self.binary = content.contains('\0');
        self.missing = false;
        self.ready = true;
        self.source = if self.binary {
            SourceLines::default()
        } else {
            SourceLines::new(content)
        };
        self.digits = (self.source.len().max(1).ilog10() + 1).max(3);
        self.chunks.borrow_mut().clear();
        self.recompute_matches();
        self.bump_keys();
    }

    pub fn set_binary(&mut self, size_bytes: usize) {
        self.size_bytes = size_bytes;
        self.binary = true;
        self.missing = false;
        self.ready = true;
        self.source = SourceLines::default();
        self.bump_keys();
    }

    pub fn set_missing(&mut self) {
        self.missing = true;
        self.binary = false;
        self.ready = true;
        self.source = SourceLines::default();
        self.bump_keys();
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

    pub fn line_count(&self) -> usize {
        self.source.len() as usize
    }

    pub fn line(&self, n: u32) -> Option<&str> {
        self.source.line(n)
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
            self.rev += 1;
        }
    }

    pub fn selection(&self) -> Option<FileLineSelection> {
        self.selection
    }

    pub fn set_selection(&mut self, sel: Option<FileLineSelection>) {
        if self.selection != sel {
            self.selection = sel;
            self.rev += 1;
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection?;
        let (a, b) = sel.ordered();
        let mut out = Vec::with_capacity((b - a + 1) as usize);
        for n in a..=b {
            out.push(self.source.line(n).unwrap_or(""));
        }
        Some(out.join("\n"))
    }

    pub(crate) fn press_line(&mut self, line: u32) {
        self.selection_at_press = self.selection;
        let sel = match self.selection {
            Some(s) if self.shift => FileLineSelection { start: s.start, end: line },
            _ => FileLineSelection { start: line, end: line },
        };
        self.unselect = !self.shift && self.selection == Some(sel);
        self.selecting = true;
        self.selection = Some(sel);
        self.rev += 1;
    }

    pub(crate) fn drag_to_offset(&mut self, start_line: u32, dy: f32) {
        if !self.selecting || self.source.is_empty() {
            return;
        }
        let delta = (dy / LINE_H).round() as i64;
        let target = (start_line as i64 + delta).clamp(1, self.source.len() as i64) as u32;
        if let Some(s) = self.selection
            && s.end != target
        {
            self.unselect = false;
            self.selection = Some(FileLineSelection { start: s.start, end: target });
            self.rev += 1;
        }
    }

    pub(crate) fn release_line(&mut self) {
        self.selection_at_press = None;
        if std::mem::take(&mut self.unselect) {
            self.selection = None;
            self.rev += 1;
        }
        self.selecting = false;
    }

    pub(crate) fn cancel_line(&mut self) {
        if self.selecting {
            self.selection = self.selection_at_press.take();
            self.rev += 1;
        }
        self.selecting = false;
        self.unselect = false;
    }

    pub fn scroll_to_line(&mut self, scroll_key: &str, line: u32, select: bool, tx: &mut Tx) {
        if self.source.is_empty() {
            return;
        }
        let clamped = line.clamp(1, self.source.len());
        if select {
            self.selection = Some(FileLineSelection { start: clamped, end: clamped });
            self.rev += 1;
        }
        let top_line = clamped.saturating_sub(3).max(1);
        tx.instant();
        tx.scroll_to_y(scroll_key.to_owned(), (top_line - 1) as f32 * LINE_H);
    }

    /// [`Self::scroll_to_line`] at `start`, selecting `start..=end`.
    pub fn scroll_to_lines(&mut self, scroll_key: &str, start: u32, end: u32, tx: &mut Tx) {
        self.scroll_to_line(scroll_key, start, true, tx);
        if let Some(sel) = self.selection.as_mut() {
            sel.end = end.clamp(sel.start, self.source.len().max(sel.start));
        }
    }

    pub fn sync_find_query(&mut self) -> bool {
        if self.find_input.text == self.find_query_applied {
            return false;
        }
        self.find_query_applied = self.find_input.text.clone();
        self.active_match = 0;
        self.recompute_matches();
        self.rev += 1;
        true
    }

    fn recompute_matches(&mut self) {
        self.matches.clear();
        self.by_line_matches.clear();
        let needle = self.find_query_applied.to_lowercase();
        if needle.is_empty() || self.source.is_empty() {
            return;
        }
        for n in 1..=self.source.len() {
            let Some(line) = self.source.line(n) else { continue };
            let lower = line.to_lowercase();
            if lower.len() != line.len() {
                // Safe ASCII/UTF-8 fallback when case-folding changes byte length.
                for (idx, _) in line.match_indices(&self.find_query_applied) {
                    self.matches.push((n, idx..idx + self.find_query_applied.len()));
                }
            } else {
                for (idx, _) in lower.match_indices(&needle) {
                    if line.is_char_boundary(idx) && line.is_char_boundary(idx + needle.len()) {
                        self.matches.push((n, idx..idx + needle.len()));
                    }
                }
            }
        }
        self.rebuild_line_match_index();
    }

    fn rebuild_line_match_index(&mut self) {
        self.by_line_matches.clear();
        for (i, (line, range)) in self.matches.iter().enumerate() {
            self.by_line_matches
                .entry(*line)
                .or_default()
                .push((range.clone(), i == self.active_match));
        }
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn active_match_index(&self) -> usize {
        self.active_match
    }

    pub fn step_match(&mut self, scroll_key: &str, forward: bool, tx: &mut Tx) {
        if self.matches.is_empty() {
            return;
        }
        let len = self.matches.len();
        self.active_match = if forward {
            (self.active_match + 1) % len
        } else {
            (self.active_match + len - 1) % len
        };
        self.rebuild_line_match_index();
        self.rev += 1;
        let line = self.matches[self.active_match].0;
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
            let built = HighlightChunk::from_lines(
                Some(lang),
                (first_n..=last_n).map(|m| self.source.line(m).unwrap_or("")),
            );
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
        if let Key::Modifier { modifier: Modifier::ShiftLeft | Modifier::ShiftRight, .. } = &ev.key
        {
            self.shift = ev.mods.shift;
            return false;
        }
        if crate::keymap::Bind::Find.matches_tx(ev, tx) {
            tx.instant();
            self.find_open = true;
            self.find_input.focused = true;
            return true;
        }
        if self.find_open && self.find_input.focused {
            match &ev.key {
                Key::Escape => {
                    tx.instant();
                    self.find_open = false;
                    self.find_input = TextEdit::new("");
                    self.sync_find_query();
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
            if self.find_open {
                tx.instant();
                self.find_open = false;
                self.find_input = TextEdit::new("");
                self.sync_find_query();
                return true;
            }
            if self.selection.is_some() {
                tx.instant();
                self.selection = None;
                self.rev += 1;
                return true;
            }
        }
        if ev.is_command('c')
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
