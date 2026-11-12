//! State and keyboard navigation for [`DiffViewer`](super::DiffViewer).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use rune_kit::diff::{DiffLine, DiffMode, Expand, FileDiff, Hunk, LineKind, Side, SplitRow};
use rune_kit::line_selection::LineSelection;
use rune_kit::syntax::Paint;

use super::fold::{ExpandMode, Fold, FoldEffect, FoldEvent, GapFacts, Reveal};
use super::{
    CHUNK_LINES, ChunkMap, GAP_H, HEADER_H, HighlightChunk, LINE_H, NOTE_H, SourceLines,
    lang_for_path, visual_columns,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffRow {
    File(u32),
    Note(u32),
    Gap { f: u32, gap: u32 },
    Ctx { f: u32, old: u32, new: u32 },
    Line { f: u32, h: u32, i: u32 },
    Pair { f: u32, h: u32, del: u32, add: u32 },
}

impl DiffRow {
    pub(crate) fn file(self) -> usize {
        match self {
            Self::File(f)
            | Self::Note(f)
            | Self::Gap { f, .. }
            | Self::Ctx { f, .. }
            | Self::Line { f, .. }
            | Self::Pair { f, .. } => f as usize,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GapSpan {
    pub(crate) old: u32,
    pub(crate) new: u32,
    pub(crate) len: Option<u32>,
}

#[derive(Clone, Debug, Default)]
struct FileUi {
    collapsed: bool,
    /// Each unchanged range's fold, by range (before each hunk, then after the last).
    folds: Vec<Fold>,
}

fn hunk_first(h: &Hunk, side: Side) -> u32 {
    let (start, count) = match side {
        Side::Old => (h.old_start, h.old_count),
        Side::New => (h.new_start, h.new_count),
    };
    if count == 0 { start.saturating_add(1) } else { start.max(1) }
}

fn hunk_end(h: &Hunk, side: Side) -> u32 {
    hunk_first(h, side).saturating_add(if side == Side::Old { h.old_count } else { h.new_count })
}

fn compute_gaps(file: &FileDiff, full: Option<&SourceLines>) -> Vec<GapSpan> {
    let mut out = Vec::with_capacity(file.hunks.len() + 1);
    let mut old_cur = 1u32;
    let mut new_cur = 1u32;
    for h in &file.hunks {
        let old_next = hunk_first(h, Side::Old);
        let new_next = hunk_first(h, Side::New);
        let len = new_next.saturating_sub(new_cur).max(old_next.saturating_sub(old_cur));
        out.push(GapSpan { old: old_cur, new: new_cur, len: Some(len) });
        old_cur = hunk_end(h, Side::Old);
        new_cur = hunk_end(h, Side::New);
    }
    let tail_len = full.map(|src| src.len().saturating_add(1).saturating_sub(new_cur));
    out.push(GapSpan { old: old_cur, new: new_cur, len: tail_len });
    out
}

/// Reusable diff state for both the Review panel and inline tool-step diffs.
pub struct DiffDocState {
    pub(super) files: Vec<FileDiff>,
    full_texts: HashMap<String, SourceLines>,
    ui: Vec<FileUi>,
    mode: DiffMode,
    expand_mode: ExpandMode,
    wrap: bool,
    pub(super) hide_file_headers: bool,
    /// Draws only this file's rows: Review's single-file preview.
    only: Option<usize>,
    /// The selected lines and the gesture on the line numbers making them (rune-kit's shared machine).
    pub(super) lines: LineSelection<DiffLine>,
    pub(super) copy_side: Side,
    chunks: RefCell<ChunkMap<(u32, Side, u32)>>,
    side_lines: Vec<Vec<[Vec<u32>; 2]>>,
    pub(super) gaps: Vec<Vec<GapSpan>>,
    digits: Vec<u32>,
    columns: u32,
    pub(crate) rows: Vec<DiffRow>,
    pub(crate) keys: Vec<String>,
    pub(super) row_of: HashMap<(u32, Side, u32), u32>,
    pub(super) file_rows: Vec<u32>,
    pub(super) hunk_rows: Vec<u32>,
    pub(crate) rev: u64,
    pub(crate) keys_rev: u64,
    pub(super) visible_rows: Range<usize>,
    view_w: Cell<f32>,
    /// The rows' laid-out heights by key: a drag down the numbers walks them.
    pub(crate) heights: super::RowHeights,
    pub(super) visible: usize,
    reported: Option<usize>,
    pub(super) nav: Option<u32>,
    /// The line row the keyboard's line cursor is on: the diff's one Tab stop, which Up and Down
    /// move (a roving tab index). None: the first line row.
    pub(super) cursor: Option<u32>,
}

impl Default for DiffDocState {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl std::fmt::Debug for DiffDocState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiffDocState")
            .field("files", &self.files.len())
            .field("rows", &self.rows.len())
            .field("mode", &self.mode)
            .finish()
    }
}

impl DiffDocState {
    pub fn new(files: Vec<FileDiff>) -> Self {
        let mut s = Self {
            files: Vec::new(),
            full_texts: HashMap::new(),
            ui: Vec::new(),
            mode: DiffMode::Unified,
            expand_mode: ExpandMode::Collapse,
            // Desktop's review wraps long lines (`overflow={props.overflow ?? "wrap"}`,
            // session-ui `session-review.tsx:691`); only the narrow review lets them scroll.
            wrap: true,
            hide_file_headers: false,
            only: None,
            lines: LineSelection::default(),
            copy_side: Side::New,
            chunks: RefCell::new(HashMap::new()),
            side_lines: Vec::new(),
            gaps: Vec::new(),
            digits: Vec::new(),
            columns: 0,
            rows: Vec::new(),
            keys: Vec::new(),
            row_of: HashMap::new(),
            file_rows: Vec::new(),
            hunk_rows: Vec::new(),
            rev: 0,
            keys_rev: 0,
            visible_rows: 0..0,
            view_w: Cell::new(0.0),
            heights: super::RowHeights::default(),
            visible: 0,
            reported: None,
            nav: None,
            cursor: None,
        };
        s.set_files(files);
        s
    }

    /// Inline single-file diff (e.g. for an `edit` tool step in the session timeline) without file header rows.
    pub fn inline(file: FileDiff) -> Self {
        let mut s = Self::new(vec![file]);
        s.hide_file_headers = true;
        s.rebuild();
        s
    }

    /// Draws only file `file`'s rows (`None`: every file's, one under another).
    pub fn show_only(&mut self, file: Option<usize>) {
        let file = file.filter(|&f| f < self.files.len());
        if self.only != file {
            self.only = file;
            self.set_selection(None);
            self.visible = file.unwrap_or(0);
            self.reported = None;
            self.visible_rows = 0..0;
            self.rebuild();
        }
    }

    /// The file whose rows alone show, if one.
    pub fn only(&self) -> Option<usize> {
        self.only
    }

    pub fn set_hide_file_headers(&mut self, hide: bool) {
        if self.hide_file_headers != hide {
            self.hide_file_headers = hide;
            self.rebuild();
        }
    }

    pub fn set_files(&mut self, files: Vec<FileDiff>) {
        self.ui = files
            .iter()
            .map(|f| FileUi { collapsed: false, folds: vec![Fold::Folded { top: 0, bottom: 0 }; f.hunks.len() + 1] })
            .collect();
        self.side_lines = files
            .iter()
            .map(|f| {
                f.hunks
                    .iter()
                    .map(|h| {
                        let on = |side: Side| {
                            (0..h.lines.len() as u32)
                                .filter(|&i| h.lines[i as usize].number(side).is_some())
                                .collect()
                        };
                        [on(Side::Old), on(Side::New)]
                    })
                    .collect()
            })
            .collect();
        // A folded full-context diff carries its whole new text, so its unchanged ranges expand
        // without a fetch, and a stale text from an earlier load gives way to it.
        for f in &files {
            if let Some(text) = f.text(Side::New) {
                self.full_texts.insert(f.path().to_owned(), SourceLines::new(text));
            }
        }
        self.files = files;
        self.only = None;
        self.recompute_metrics();
        for f in 0..self.files.len() {
            for gap in 0..self.gaps[f].len() {
                self.ui[f].folds[gap] = Fold::initial(self.gap_facts(f, gap));
            }
        }
        if let (_, Some(event)) = ExpandMode::Collapse.step(self.expand_mode) {
            self.step_file_folds(None, event);
        }
        self.set_selection(None);
        self.chunks.borrow_mut().clear();
        self.heights.clear();
        self.visible = 0;
        self.reported = None;
        self.visible_rows = 0..0;
        self.rebuild();
    }

    fn recompute_metrics(&mut self) {
        self.gaps = self
            .files
            .iter()
            .map(|f| compute_gaps(f, self.full_texts.get(f.path())))
            .collect();
        self.digits = self
            .files
            .iter()
            .map(|f| {
                let last_hunk = f
                    .hunks
                    .last()
                    .map_or(1, |h| hunk_end(h, Side::Old).max(hunk_end(h, Side::New)));
                let last_full = self.full_texts.get(f.path()).map_or(0, SourceLines::len);
                (last_hunk.max(last_full).max(1).ilog10() + 1).max(3)
            })
            .collect();
        let hunk_cols = self
            .files
            .iter()
            .flat_map(|f| &f.hunks)
            .flat_map(|h| &h.lines)
            .map(|l| visual_columns(&l.text))
            .max()
            .unwrap_or(0);
        let full_cols = self.full_texts.values().map(SourceLines::columns).max().unwrap_or(0);
        self.columns = hunk_cols.max(full_cols);
    }

    /// What range `gap` of file `f`'s transitions read.
    fn gap_facts(&self, f: usize, gap: usize) -> GapFacts {
        GapFacts {
            len: self.gaps[f][gap].len,
            has_text: self.has_full_text(f),
            first: gap == 0,
            last: gap + 1 == self.gaps[f].len(),
        }
    }

    /// Steps every range of file `only` (every file's when `None`) with `event`.
    fn step_file_folds(&mut self, only: Option<usize>, event: FoldEvent) {
        for f in (0..self.files.len()).filter(|&f| only.is_none_or(|o| o == f)) {
            for gap in 0..self.gaps[f].len() {
                let facts = self.gap_facts(f, gap);
                // Only a range's own reveal asks for text; these events never do.
                self.ui[f].folds[gap] = self.ui[f].folds[gap].step(event, facts).0;
            }
        }
    }

    /// Attaches full file contents for `path`: every range of that file waiting on it opens as
    /// it was asked to, and under "Show all lines" the rest open too.
    pub fn attach_file_text(&mut self, path: &str, text: &str) -> bool {
        let src = SourceLines::new(text);
        self.full_texts.insert(path.to_owned(), src);
        self.recompute_metrics();
        self.chunks.borrow_mut().clear();
        let of_path: Vec<usize> = (0..self.files.len()).filter(|&f| self.files[f].path() == path).collect();
        for f in of_path {
            self.step_file_folds(Some(f), FoldEvent::TextArrived);
            if self.expand_mode == ExpandMode::Expand {
                self.step_file_folds(Some(f), FoldEvent::Unfold);
            }
        }
        self.rebuild();
        true
    }

    /// The text of `path` can't be had: ranges waiting on it fold again.
    pub fn text_failed(&mut self, path: &str) {
        let of_path: Vec<usize> = (0..self.files.len()).filter(|&f| self.files[f].path() == path).collect();
        for f in of_path {
            self.step_file_folds(Some(f), FoldEvent::TextFailed);
        }
        self.rebuild();
    }

    pub fn has_full_text(&self, file: usize) -> bool {
        self.files
            .get(file)
            .is_some_and(|f| self.full_texts.contains_key(f.path()))
    }

    pub fn files(&self) -> &[FileDiff] {
        &self.files
    }

    /// The rows' height one under another, unwrapped: a diff shown whole inside another scroll.
    /// A row laid out with comments under it counts as tall as it came out.
    pub fn rows_height(&self) -> f32 {
        (0..self.rows.len()).map(|r| self.estimate(r)).sum()
    }

    pub fn file_index(&self, path: &str) -> Option<usize> {
        self.files
            .iter()
            .position(|f| f.path() == path || f.old_path.as_deref() == Some(path))
    }

    pub fn mode(&self) -> DiffMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: DiffMode) {
        if self.mode != mode {
            self.mode = mode;
            self.heights.clear();
            self.rebuild();
        }
    }

    pub fn expand_mode(&self) -> ExpandMode {
        self.expand_mode
    }

    pub fn set_expand_mode(&mut self, mode: ExpandMode) {
        let (next, event) = self.expand_mode.step(mode);
        self.expand_mode = next;
        if let Some(event) = event {
            self.step_file_folds(None, event);
            self.rebuild();
        }
    }

    pub(crate) fn wrap(&self) -> bool {
        self.wrap
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.heights.clear();
            self.rev += 1;
        }
    }

    pub(crate) fn wraps(&self) -> bool {
        self.wrap || self.mode == DiffMode::Split
    }

    /// In split mode, the one side `file` shows across the whole width, as Desktop's review
    /// draws an added or deleted file: one column, no empty half.
    pub(crate) fn single_column(&self, file: usize) -> Option<Side> {
        if self.mode != DiffMode::Split {
            return None;
        }
        self.files.get(file)?.sole_side()
    }

    pub fn collapsed(&self, file: usize) -> bool {
        self.ui.get(file).is_some_and(|u| u.collapsed)
    }

    pub fn set_collapsed(&mut self, file: usize, collapsed: bool) {
        if let Some(u) = self.ui.get_mut(file).filter(|u| u.collapsed != collapsed) {
            u.collapsed = collapsed;
            self.rebuild();
        }
    }

    pub fn hidden(&self, file: usize, gap: usize) -> Option<u32> {
        let g = self.gaps.get(file)?.get(gap)?;
        self.ui[file].folds[gap].hidden(g.len)
    }

    /// Range `gap` of `file`'s fold.
    pub fn fold(&self, file: usize, gap: usize) -> Option<Fold> {
        self.ui.get(file)?.folds.get(gap).copied()
    }

    /// Opens range `gap` of `file` as a separator does. Asks the app to load the file's text
    /// (`FetchText`) when it has to wait for it; the range shows as expanding until
    /// [`Self::attach_file_text`] or [`Self::text_failed`].
    pub fn reveal(&mut self, file: usize, gap: usize, how: Reveal) -> Option<FoldEffect> {
        let fold = self.fold(file, gap)?;
        let (next, effect) = fold.step(FoldEvent::Reveal(how), self.gap_facts(file, gap));
        if next != fold {
            self.ui[file].folds[gap] = next;
            self.rebuild();
        }
        effect
    }

    /// Opens `gap` of `file` by `how`, when the file's text is loaded.
    pub fn expand(&mut self, file: usize, gap: usize, how: Expand) -> bool {
        self.has_full_text(file) && self.reveal(file, gap, Reveal::Step(how)).is_none()
    }

    pub fn digits(&self, file: usize) -> u32 {
        self.digits.get(file).copied().unwrap_or(3)
    }

    pub fn columns(&self) -> u32 {
        self.columns
    }

    pub(crate) fn set_viewport(&self, r: [f32; 4]) {
        self.view_w.set(r[2]);
    }

    pub(crate) fn view_width(&self) -> f32 {
        self.view_w.get()
    }

    pub(crate) fn set_visible_rows(&mut self, rows: Range<usize>) -> (Option<usize>, bool) {
        let before_sticky = self.sticky();
        self.visible_rows = rows;
        if let Some(n) = self.nav
            && self.visible_rows.contains(&(n as usize))
        {
            self.visible = self.rows[n as usize].file();
        } else {
            self.nav = None;
            if let Some(&r) = self.rows.get(self.visible_rows.start) {
                self.visible = r.file();
            }
        }
        let reported = (!self.files.is_empty() && self.reported != Some(self.visible)).then(|| {
            self.reported = Some(self.visible);
            self.visible
        });
        (reported, self.sticky() != before_sticky)
    }

    pub(crate) fn row_from(&self, from: u32, mut y: f32) -> Option<u32> {
        let mut r = usize::try_from(from).ok()?.min(self.rows.len().checked_sub(1)?);
        while y < 0.0 && r > 0 {
            r -= 1;
            y += self.estimate(r);
        }
        while r + 1 < self.rows.len() {
            let h = self.estimate(r);
            if y < h {
                break;
            }
            y -= h;
            r += 1;
        }
        Some(r as u32)
    }

    pub(crate) fn sticky(&self) -> Option<usize> {
        if self.hide_file_headers || self.files.is_empty() || self.visible_rows.is_empty() {
            return None;
        }
        let f = self.visible;
        let header = *self.file_rows.get(f)? as usize;
        (self.visible_rows.start > header).then_some(f)
    }

    /// Row `r`'s height: as laid out, else its kind's (a wrapped line, or one with comments
    /// under it, is taller than a line).
    pub(super) fn estimate(&self, r: usize) -> f32 {
        if let Some(h) = self.keys.get(r).and_then(|k| self.heights.get(k)) {
            return h;
        }
        match self.rows[r] {
            DiffRow::File(_) => HEADER_H,
            DiffRow::Note(_) => NOTE_H,
            DiffRow::Gap { .. } => GAP_H,
            _ => LINE_H,
        }
    }

    pub(crate) fn side_line(&self, f: usize, side: Side, n: u32) -> Option<&str> {
        let file = &self.files[f];
        let hunks = &file.hunks;
        let h = hunks.partition_point(|hk| hunk_end(hk, side) <= n);
        if let Some(hunk) = hunks.get(h).filter(|hk| hunk_first(hk, side) <= n) {
            let i = self.side_lines[f][h][side as usize].get((n - hunk_first(hk_ref(hunk), side)) as usize)?;
            return Some(&hunk.lines[*i as usize].text);
        }
        let g = *self.gaps.get(f)?.get(h)?;
        let off = n.checked_sub(if side == Side::Old { g.old } else { g.new })?;
        let full = self.full_texts.get(file.path())?;
        full.line(g.new.saturating_add(off))
    }

    pub(crate) fn highlights(&self, f: usize, side: Side, n: u32) -> Vec<(Range<usize>, Paint)> {
        let Some(lang) = lang_for_path(self.files[f].path()) else {
            return Vec::new();
        };
        let c = n.saturating_sub(1) / CHUNK_LINES;
        let key = (f as u32, side, c);
        let cached = self.chunks.borrow().get(&key).cloned();
        let chunk = cached.unwrap_or_else(|| {
            let first_n = c * CHUNK_LINES + 1;
            let last_n = (c + 1) * CHUNK_LINES;
            let built = HighlightChunk::lines(Some(lang), first_n, last_n, |m| self.side_line(f, side, m).unwrap_or(""));
            let mut cache = self.chunks.borrow_mut();
            if cache.len() > 512 {
                cache.clear();
            }
            cache.insert(key, built.clone());
            built
        });
        chunk.line_highlights((n.saturating_sub(1) % CHUNK_LINES) as usize)
    }

    pub(crate) fn row_line(&self, r: DiffRow, side: Side) -> Option<u32> {
        match r {
            DiffRow::Ctx { old, new, .. } => Some(if side == Side::Old { old } else { new }),
            DiffRow::Line { f, h, i } => {
                self.files[f as usize].hunks[h as usize].lines[i as usize].number(side)
            }
            DiffRow::Pair { f, h, del, add } => {
                let i = if side == Side::Old { del } else { add };
                self.files[f as usize].hunks[h as usize].lines[i as usize].number(side)
            }
            _ => None,
        }
    }

    fn rebuild(&mut self) {
        self.rev += 1;
        self.nav = None;
        let mut rows = Vec::new();
        self.file_rows.clear();
        self.hunk_rows.clear();
        for (f, file) in self.files.iter().enumerate() {
            let fu = f as u32;
            self.file_rows.push(rows.len() as u32);
            if self.only.is_some_and(|o| o != f) {
                continue;
            }
            if !self.hide_file_headers {
                rows.push(DiffRow::File(fu));
                if self.ui[f].collapsed {
                    continue;
                }
            }
            if file.hunks.is_empty() {
                rows.push(DiffRow::Note(fu));
                continue;
            }
            for gap in 0..=file.hunks.len() {
                let g = self.gaps[f][gap];
                let (top, bottom) = self.ui[f].folds[gap].shown(g.len);
                let ctx = |k: u32| DiffRow::Ctx { f: fu, old: g.old + k, new: g.new + k };
                rows.extend((0..top).map(ctx));
                let hidden = g.len.map(|n| n.saturating_sub(top + bottom));
                let first_row = rows.len() as u32;
                if hidden.is_some_and(|n| n > 0) {
                    rows.push(DiffRow::Gap { f: fu, gap: gap as u32 });
                }
                if let Some(n) = g.len {
                    rows.extend((n - bottom.min(n)..n).map(ctx));
                }
                let Some(h) = file.hunks.get(gap) else { break };
                self.hunk_rows.push(first_row);
                let hu = gap as u32;
                // Split rows pair lines as Pierre does, by similarity in an uneven run
                // (`realignChangeContent.js`), from rune-kit's `Line::pair`.
                match self.mode {
                    DiffMode::Split => rows.extend(h.split_rows().map(|r| match r {
                        SplitRow::Pair { del, add } => DiffRow::Pair { f: fu, h: hu, del: del as u32, add: add as u32 },
                        SplitRow::Line(i) => DiffRow::Line { f: fu, h: hu, i: i as u32 },
                    })),
                    DiffMode::Unified => rows.extend((0..h.lines.len() as u32).map(|i| DiffRow::Line { f: fu, h: hu, i })),
                }
            }
        }
        self.row_of.clear();
        let keys: Vec<String> = rows.iter().map(|&r| self.key_of(r)).collect();
        self.keys = keys;
        static KEYS: AtomicU64 = AtomicU64::new(1);
        self.keys_rev = KEYS.fetch_add(1, Ordering::Relaxed);
        for (i, &r) in rows.iter().enumerate() {
            for side in [Side::Old, Side::New] {
                if let Some(n) = self.row_line(r, side) {
                    self.row_of.insert((r.file() as u32, side, n), i as u32);
                }
            }
        }
        self.rows = rows;
    }

    fn key_of(&self, r: DiffRow) -> String {
        match r {
            DiffRow::File(f) => format!("df{f}"),
            DiffRow::Note(f) => format!("df{f}.note"),
            DiffRow::Gap { f, gap } => format!("df{f}.g{gap}"),
            DiffRow::Ctx { f, old, .. } => format!("df{f}.c{old}"),
            DiffRow::Line { f, h, i } => {
                let l = &self.files[f as usize].hunks[h as usize].lines[i as usize];
                match l.kind {
                    LineKind::Context => format!("df{f}.c{}", l.old.unwrap_or(0)),
                    LineKind::Removed => format!("df{f}.o{}", l.old.unwrap_or(0)),
                    LineKind::Added => format!("df{f}.n{}", l.new.unwrap_or(0)),
                }
            }
            DiffRow::Pair { f, h, del, .. } => format!(
                "df{f}.o{}",
                self.files[f as usize].hunks[h as usize].lines[del as usize]
                    .old
                    .unwrap_or(0)
            ),
        }
    }
}

fn hk_ref(h: &Hunk) -> &Hunk {
    h
}
