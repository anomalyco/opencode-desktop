//! Off-UI-thread preparation for [`CodeDocState::refresh`](super::CodeDocState::refresh): splits
//! lines, keeps unchanged prefix and suffix line identities across edits and insertions so
//! untouched and shifted rows keep their memos, pre-interns `"cb0"..` band keys, and colours the
//! visible chunks around the viewport when the edit touched them.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use super::file_state::Slab;
use super::{CHUNK_LINES, HighlightChunk, SourceLines, lang_for_path};

pub const BAND_LINES: u32 = 8;
const MAX_INTERNED_BANDS: usize = 65_536;

static NEXT_LINE_ID: AtomicU64 = AtomicU64::new(1);
static BAND_KEYS: OnceLock<RwLock<Arc<[Box<str>]>>> = OnceLock::new();

fn key_store() -> &'static RwLock<Arc<[Box<str>]>> {
    BAND_KEYS.get_or_init(|| RwLock::new(Arc::from([])))
}

/// Grows the shared `"cb0"..` band-key table up to `bands` (capped at [`MAX_INTERNED_BANDS`]), so
/// `virtual_rows` borrows band keys without formatting on the UI thread.
pub fn ensure_band_keys(bands: usize) {
    let want = bands.min(MAX_INTERNED_BANDS);
    if key_store().read().unwrap_or_else(|e| e.into_inner()).len() >= want {
        return;
    }
    let mut guard = key_store().write().unwrap_or_else(|e| e.into_inner());
    if guard.len() >= want {
        return;
    }
    let cap = want.next_power_of_two().min(MAX_INTERNED_BANDS);
    let mut vec: Vec<Box<str>> = Vec::with_capacity(cap);
    vec.extend(guard.iter().cloned());
    for b in guard.len()..cap {
        vec.push(format!("cb{b}").into_boxed_str());
    }
    *guard = vec.into();
}

/// Snapshot of the interned `"cb0"..` band-key table for `lines` total lines.
pub fn band_keys(lines: usize) -> Arc<[Box<str>]> {
    let bands = lines.div_ceil(BAND_LINES as usize);
    ensure_band_keys(bands);
    key_store().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Fresh monotonic identities for `count` logical lines.
pub fn fresh_line_ids(count: usize) -> Arc<[u64]> {
    if count == 0 {
        return Arc::from([]);
    }
    let base = NEXT_LINE_ID.fetch_add(count as u64, Ordering::Relaxed);
    (base..base + count as u64).collect::<Vec<_>>().into()
}

/// Snapshot of a [`CodeDocState`](super::CodeDocState) before a background read, so the worker
/// keeps line identities and highlights only what changed around the viewport.
pub struct Prior {
    pub source: SourceLines,
    pub trailing: bool,
    pub line_ids: Arc<[u64]>,
    pub lang: Option<&'static str>,
    pub mounted: (u32, u32),
}

impl Prior {
    pub fn initial(path: &str) -> Self {
        Self {
            source: SourceLines::default(),
            trailing: false,
            line_ids: Arc::from([]),
            lang: lang_for_path(path),
            mounted: (1, CHUNK_LINES * 2),
        }
    }
}

pub(super) struct PreparedFind {
    pub(super) query: String,
    pub(super) slab: opencode_search::SlabFind,
    pub(super) matches: Vec<(u32, Range<usize>)>,
    /// The matches by line, the first one lit (`CodeDocState::by_line_matches`).
    pub(super) by_line: HashMap<u32, Vec<(Range<usize>, bool)>>,
}

/// A file's text made ready on the worker that read it.
pub struct Prepared {
    pub(super) source: SourceLines,
    pub(super) trailing: bool,
    pub(super) line_ids: Arc<[u64]>,
    pub(super) row_keys: Arc<[Box<str>]>,
    pub(super) prefix_content: u32,
    pub(super) prefix_rows: u32,
    pub(super) same_content_len: bool,
    pub(super) chunks: Vec<(u32, Arc<HighlightChunk>)>,
    pub(super) find: Option<PreparedFind>,
}

impl Prepared {
    /// Prepares `text` against `prior`; `find` is Find's query while Find is open.
    pub fn new(text: String, find: Option<String>, prior: Prior) -> Self {
        let trailing = text.ends_with('\n');
        let source = SourceLines::from_string(text);
        let new_content = source.len();
        let old_content = prior.source.len();
        let same_content_len = old_content == new_content && old_content > 0;

        let min_content = old_content.min(new_content);
        let mut prefix_content = 0u32;
        while prefix_content < min_content
            && prior.source.line(prefix_content + 1) == source.line(prefix_content + 1)
        {
            prefix_content += 1;
        }

        let old_rows = old_content + u32::from(prior.trailing);
        let new_rows = new_content + u32::from(trailing);

        let (line_ids, prefix_rows, suffix_rows) =
            reuse_line_ids(&prior, &source, trailing, old_rows, new_rows);
        let row_keys = build_band_keys(
            new_rows as usize,
            old_rows as usize,
            prefix_rows as usize,
            suffix_rows as usize,
            &line_ids,
        );

        let mut chunks = Vec::new();
        if let Some(lang) = prior.lang
            && new_content > 0
        {
            let lo = prior.mounted.0.clamp(1, new_content);
            let hi = prior.mounted.1.max(lo).clamp(1, new_content);
            let c0 = (lo - 1) / CHUNK_LINES;
            let c1 = ((hi - 1) / CHUNK_LINES).min(c0 + 5);
            for c in c0..=c1 {
                if !chunk_kept(c, new_content, prefix_content, same_content_len) {
                    let first = c * CHUNK_LINES + 1;
                    let last = ((c + 1) * CHUNK_LINES).min(new_content);
                    let built = HighlightChunk::lines_worker(Some(lang), first, last, |m| {
                        source.line(m).unwrap_or("")
                    });
                    chunks.push((c, built));
                }
            }
        }

        let find = find.map(|query| {
            let slab = Slab::build(&source);
            let mut matches = Vec::new();
            if !query.is_empty() {
                slab.find(&query, |m| matches.push((m.row as u32 + 1, m.bytes)));
            }
            let mut by_line: HashMap<u32, Vec<(Range<usize>, bool)>> = HashMap::new();
            for (i, (line, range)) in matches.iter().enumerate() {
                by_line.entry(*line).or_default().push((range.clone(), i == 0));
            }
            PreparedFind { query, slab, matches, by_line }
        });

        Self {
            source,
            trailing,
            line_ids,
            row_keys,
            prefix_content,
            prefix_rows,
            same_content_len,
            chunks,
            find,
        }
    }

    pub(super) fn keeps_chunk(&self, c: u32) -> bool {
        chunk_kept(c, self.source.len(), self.prefix_content, self.same_content_len)
    }
}

fn chunk_kept(c: u32, new_content: u32, prefix_content: u32, same_content_len: bool) -> bool {
    if new_content == 0 {
        return false;
    }
    let end = (c + 1) * CHUNK_LINES;
    end <= prefix_content || (same_content_len && new_content <= prefix_content && c * CHUNK_LINES < new_content)
}

fn logical_line<'a>(source: &'a SourceLines, trailing: bool, n: u32) -> Option<&'a str> {
    source
        .line(n)
        .or_else(|| (trailing && n == source.len() + 1).then_some(""))
}

fn reuse_line_ids(
    prior: &Prior,
    source: &SourceLines,
    trailing: bool,
    old_rows: u32,
    new_rows: u32,
) -> (Arc<[u64]>, u32, u32) {
    if prior.line_ids.len() != old_rows as usize || old_rows == 0 || new_rows == 0 {
        return (fresh_line_ids(new_rows as usize), 0, 0);
    }
    let min_rows = old_rows.min(new_rows);
    let mut prefix = 0u32;
    while prefix < min_rows
        && logical_line(&prior.source, prior.trailing, prefix + 1)
            == logical_line(source, trailing, prefix + 1)
    {
        prefix += 1;
    }
    if prefix == old_rows && old_rows == new_rows {
        return (prior.line_ids.clone(), prefix, 0);
    }
    let max_suffix = min_rows - prefix;
    let mut suffix = 0u32;
    while suffix < max_suffix
        && logical_line(&prior.source, prior.trailing, old_rows - suffix)
            == logical_line(source, trailing, new_rows - suffix)
    {
        suffix += 1;
    }
    let mid = (new_rows - prefix - suffix) as usize;
    let fresh_base = if mid > 0 {
        NEXT_LINE_ID.fetch_add(mid as u64, Ordering::Relaxed)
    } else {
        0
    };
    let mut out = Vec::with_capacity(new_rows as usize);
    out.extend_from_slice(&prior.line_ids[..prefix as usize]);
    out.extend(fresh_base..fresh_base + mid as u64);
    out.extend_from_slice(&prior.line_ids[(old_rows - suffix) as usize..]);
    (out.into(), prefix, suffix)
}

/// Gives bands whose lines are 100% newly inserted a fresh `"cbn{lid}"` key so `VirtualList`'s
/// overscan budget mounts only the viewport plus half a viewport on the reload frame.
fn build_band_keys(
    new_rows: usize,
    old_rows: usize,
    prefix: usize,
    suffix: usize,
    line_ids: &[u64],
) -> Arc<[Box<str>]> {
    let shared = band_keys(new_rows);
    if old_rows == 0 || new_rows <= old_rows {
        return shared;
    }
    let ins_end = new_rows.saturating_sub(suffix);
    if ins_end.saturating_sub(prefix) < BAND_LINES as usize {
        return shared;
    }
    let bands = new_rows.div_ceil(BAND_LINES as usize);
    let first_new = prefix.div_ceil(BAND_LINES as usize);
    let end_new = ins_end / BAND_LINES as usize;
    if first_new >= end_new || bands > shared.len() {
        return shared;
    }
    let mut vec: Vec<Box<str>> = shared[..bands].to_vec();
    for b in first_new..end_new {
        let lid = line_ids.get(b * BAND_LINES as usize).copied().unwrap_or(0);
        vec[b] = format!("cbn{lid}").into_boxed_str();
    }
    vec.into()
}

/// `Send` representation of [`rune_kit::diff::FileDiff`] after [`DiffDocState`](super::DiffDocState)
/// has extracted `full_texts`, so a [`PreparedDiffDoc`] built on a worker crosses to the UI
/// thread without copying hunks.
pub(super) struct SentFileDiff {
    old_path: Option<String>,
    new_path: Option<String>,
    status: rune_kit::diff::FileStatus,
    similarity: Option<u8>,
    old_mode: Option<String>,
    new_mode: Option<String>,
    binary: bool,
    hunks: Vec<rune_kit::diff::Hunk>,
    sole: Option<rune_kit::diff::Side>,
}

impl SentFileDiff {
    fn from_diff(mut f: rune_kit::diff::FileDiff) -> Self {
        let sole = f.sole_side();
        Self {
            old_path: f.old_path.take(),
            new_path: f.new_path.take(),
            status: f.status,
            similarity: f.similarity,
            old_mode: f.old_mode.take(),
            new_mode: f.new_mode.take(),
            binary: f.binary,
            hunks: std::mem::take(&mut f.hunks),
            sole,
        }
    }

    fn into_diff(self) -> (rune_kit::diff::FileDiff, Option<rune_kit::diff::Side>) {
        let mut f = rune_kit::diff::FileDiff::between(self.old_path, self.new_path, "", "");
        f.status = self.status;
        f.similarity = self.similarity;
        f.old_mode = self.old_mode;
        f.new_mode = self.new_mode;
        f.binary = self.binary;
        f.hunks = self.hunks;
        (f, self.sole)
    }
}

/// A [`DiffDocState`](super::DiffDocState) built off the UI thread (`Send`), ready to install in
/// O(files).
pub struct PreparedDiffDoc {
    files: Vec<SentFileDiff>,
    full_texts: HashMap<String, SourceLines>,
    ui: Vec<super::diff_state::FileUi>,
    mode: rune_kit::diff::DiffMode,
    expand_mode: super::ExpandMode,
    wrap: bool,
    hide_file_headers: bool,
    only: Option<usize>,
    chunks: super::ChunkMap<(u32, rune_kit::diff::Side, u32)>,
    side_lines: Vec<Vec<[Vec<u32>; 2]>>,
    gaps: Vec<Vec<super::diff_state::GapSpan>>,
    digits: Vec<u32>,
    columns: u32,
    rows: Vec<super::diff_state::DiffRow>,
    keys: Vec<String>,
    row_of: HashMap<(u32, rune_kit::diff::Side, u32), u32>,
    file_rows: Vec<u32>,
    hunk_rows: Vec<u32>,
    rev: u64,
    keys_rev: u64,
    visible: usize,
    find: super::diff_find::DiffFind,
}

impl PreparedDiffDoc {
    pub(super) fn from_state(mut s: super::DiffDocState) -> Self {
        let files = std::mem::take(&mut s.files)
            .into_iter()
            .map(SentFileDiff::from_diff)
            .collect();
        Self {
            files,
            full_texts: s.full_texts,
            ui: s.ui,
            mode: s.mode,
            expand_mode: s.expand_mode,
            wrap: s.wrap,
            hide_file_headers: s.hide_file_headers,
            only: s.only,
            chunks: s.chunks.into_inner(),
            side_lines: s.side_lines,
            gaps: s.gaps,
            digits: s.digits,
            columns: s.columns,
            rows: s.rows,
            keys: s.keys,
            row_of: s.row_of,
            file_rows: s.file_rows,
            hunk_rows: s.hunk_rows,
            rev: s.rev,
            keys_rev: s.keys_rev,
            visible: s.visible,
            find: s.find,
        }
    }

    pub fn into_state(self) -> super::DiffDocState {
        let mut files = Vec::with_capacity(self.files.len());
        let mut sole_sides = Vec::with_capacity(self.files.len());
        for sf in self.files {
            let (f, sole) = sf.into_diff();
            files.push(f);
            sole_sides.push(sole);
        }
        super::DiffDocState {
            files,
            sole_sides,
            full_texts: self.full_texts,
            ui: self.ui,
            mode: self.mode,
            expand_mode: self.expand_mode,
            wrap: self.wrap,
            hide_file_headers: self.hide_file_headers,
            only: self.only,
            lines: rune_kit::line_selection::LineSelection::default(),
            copy_side: rune_kit::diff::Side::New,
            chunks: std::cell::RefCell::new(self.chunks),
            side_lines: self.side_lines,
            gaps: self.gaps,
            digits: self.digits,
            columns: self.columns,
            rows: self.rows,
            keys: self.keys,
            row_of: self.row_of,
            file_rows: self.file_rows,
            hunk_rows: self.hunk_rows,
            rev: self.rev,
            keys_rev: self.keys_rev,
            visible_rows: 0..0,
            view_w: std::cell::Cell::new(0.0),
            heights: super::RowHeights::default(),
            visible: self.visible,
            reported: None,
            nav: None,
            cursor: None,
            find: self.find,
        }
    }
}
