//! State and keyboard navigation for [`DiffViewer`](super::DiffViewer).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use rune_kit::diff::{
    DiffMode, EXPAND_STEP, Expand, FileDiff, Hunk, LineKind, LineRange, Side,
};
use rune_kit::syntax::Paint;

use super::{
    CHUNK_LINES, ChunkMap, GAP_H, HEADER_H, HighlightChunk, LINE_H, NOTE_H, SourceLines,
    lang_for_path, visual_columns,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExpandMode {
    #[default]
    Collapse,
    Expand,
}

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
    revealed: Vec<(u32, u32)>,
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
    pub(super) selection: Option<LineRange>,
    pub(super) selecting: bool,
    pub(super) selection_at_press: Option<LineRange>,
    pub(super) unselect: bool,
    pub(super) shift: bool,
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
    pub(super) visible: usize,
    reported: Option<usize>,
    pub(super) nav: Option<u32>,
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
            wrap: false,
            hide_file_headers: false,
            selection: None,
            selecting: false,
            selection_at_press: None,
            unselect: false,
            shift: false,
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
            visible: 0,
            reported: None,
            nav: None,
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

    pub fn set_hide_file_headers(&mut self, hide: bool) {
        if self.hide_file_headers != hide {
            self.hide_file_headers = hide;
            self.rebuild();
        }
    }

    pub fn set_files(&mut self, files: Vec<FileDiff>) {
        self.ui = files
            .iter()
            .map(|f| FileUi { collapsed: false, revealed: vec![(0, 0); f.hunks.len() + 1] })
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
        self.files = files;
        self.recompute_metrics();
        self.selection = None;
        self.chunks.borrow_mut().clear();
        self.visible = 0;
        self.reported = None;
        self.visible_rows = 0..0;
        if self.expand_mode == ExpandMode::Expand {
            self.expand_all_loaded_inner();
        }
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
            .enumerate()
            .map(|(i, f)| {
                let last_hunk = f
                    .hunks
                    .last()
                    .map_or(1, |h| hunk_end(h, Side::Old).max(hunk_end(h, Side::New)));
                let last_full = self.full_texts.get(f.path()).map_or(0, SourceLines::len);
                let _ = i;
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

    /// Attaches full file contents for `path` so unchanged gaps can be expanded on demand.
    pub fn attach_file_text(&mut self, path: &str, text: &str) -> bool {
        let src = SourceLines::new(text);
        self.full_texts.insert(path.to_owned(), src);
        self.recompute_metrics();
        self.chunks.borrow_mut().clear();
        if self.expand_mode == ExpandMode::Expand {
            self.expand_all_loaded_inner();
        }
        self.rebuild();
        true
    }

    pub fn has_full_text(&self, file: usize) -> bool {
        self.files
            .get(file)
            .is_some_and(|f| self.full_texts.contains_key(f.path()))
    }

    pub fn files(&self) -> &[FileDiff] {
        &self.files
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
            self.rebuild();
        }
    }

    pub fn expand_mode(&self) -> ExpandMode {
        self.expand_mode
    }

    pub fn set_expand_mode(&mut self, mode: ExpandMode) {
        if self.expand_mode == mode {
            return;
        }
        self.expand_mode = mode;
        match mode {
            ExpandMode::Collapse => {
                for u in &mut self.ui {
                    for r in &mut u.revealed {
                        *r = (0, 0);
                    }
                }
            }
            ExpandMode::Expand => self.expand_all_loaded_inner(),
        }
        self.rebuild();
    }

    fn expand_all_loaded_inner(&mut self) {
        for f in 0..self.files.len() {
            if !self.has_full_text(f) {
                continue;
            }
            for gap in 0..self.gaps[f].len() {
                if let Some(len) = self.gaps[f][gap].len {
                    self.ui[f].revealed[gap] = (len, 0);
                }
            }
        }
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.rev += 1;
        }
    }

    pub(crate) fn wraps(&self) -> bool {
        self.wrap || self.mode == DiffMode::Split
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
        let (top, bottom) = self.ui[file].revealed[gap];
        g.len.map(|n| n.saturating_sub(top + bottom))
    }

    pub fn expand(&mut self, file: usize, gap: usize, how: Expand) -> bool {
        if !self.has_full_text(file) {
            return false;
        }
        let Some(hidden) = self.hidden(file, gap) else { return false };
        if hidden == 0 {
            return true;
        }
        let last = gap == self.files[file].hunks.len();
        let r = &mut self.ui[file].revealed[gap];
        match how {
            Expand::All => r.0 += hidden,
            Expand::Up if gap > 0 || last => {
                if last {
                    r.0 += hidden.min(EXPAND_STEP);
                } else {
                    r.1 += hidden.min(EXPAND_STEP);
                }
            }
            Expand::Up => r.1 += hidden.min(EXPAND_STEP),
            Expand::Down => r.0 += hidden.min(EXPAND_STEP),
        }
        self.rebuild();
        true
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

    pub(super) fn estimate(&self, r: usize) -> f32 {
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
            let built = HighlightChunk::from_lines(
                Some(lang),
                (first_n..=last_n).map(|m| self.side_line(f, side, m).unwrap_or("")),
            );
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
                let (top, bottom) = self.ui[f].revealed[gap];
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
                let lines = &h.lines;
                let mut i = 0;
                while i < lines.len() {
                    if self.mode == DiffMode::Split && lines[i].kind == LineKind::Removed {
                        let del = i;
                        while i < lines.len() && lines[i].kind == LineKind::Removed {
                            i += 1;
                        }
                        let add = i;
                        while i < lines.len() && lines[i].kind == LineKind::Added {
                            i += 1;
                        }
                        let (nd, na) = (add - del, i - add);
                        for k in 0..nd.max(na) {
                            rows.push(match (k < nd, k < na) {
                                (true, true) => DiffRow::Pair {
                                    f: fu,
                                    h: hu,
                                    del: (del + k) as u32,
                                    add: (add + k) as u32,
                                },
                                (true, false) => DiffRow::Line { f: fu, h: hu, i: (del + k) as u32 },
                                _ => DiffRow::Line { f: fu, h: hu, i: (add + k) as u32 },
                            });
                        }
                        continue;
                    }
                    rows.push(DiffRow::Line { f: fu, h: hu, i: i as u32 });
                    i += 1;
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
