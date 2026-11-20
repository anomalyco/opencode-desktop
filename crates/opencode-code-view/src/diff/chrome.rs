//! Diff row cells, fold separators, note rows, file headers, and `.patch` drag-out for [`DiffViewer`](super::DiffViewer).

use std::ops::Range;
use std::rc::Rc;

use rune::{Color, Cursor, Drag, El, Icon, Phase, Span, TextStyle, Tx, col, el, icon, rich, row, text};
use rune_kit::diff::{DiffMode, EXPAND_STEP, Expand, FileStatus, LineKind, LineRef, Side};
use opencode_ui::palette::Pal;
use opencode_ui::style::{INSTANT, UI};

use super::state::DiffDocState;
use super::view::{CellData, Inner, gutter_w, tints, view_w};
use crate::fold::{FoldEffect, Reveal};
use crate::notes::hand_off;
use crate::{BAR_W, CodeLook, GAP_H, HEADER_H, LINE_H, NOTE_H, expand_tabs};

// Pierre's separator glyphs (`diffs-icon-expand`, `diffs-icon-expand-all`). Its "up" button
// shows the chevron as drawn, pointing down into the lines it opens; its "down" button flips it
// (`[data-expand-down] [data-icon] { transform: scaleY(-1) }`).
static EXPAND_UP: Icon = Icon {
    name: "oc-diff-expand-up",
    view: 16.0,
    body: r#"<path fill="currentColor" d="M3.47 5.47a.75.75 0 0 1 1.06 0L8 8.94l3.47-3.47a.75.75 0 1 1 1.06 1.06l-4 4a.75.75 0 0 1-1.06 0l-4-4a.75.75 0 0 1 0-1.06"/>"#,
};
static EXPAND_DOWN: Icon = Icon {
    name: "oc-diff-expand-down",
    view: 16.0,
    body: r#"<g transform="matrix(1 0 0 -1 0 16)"><path fill="currentColor" d="M3.47 5.47a.75.75 0 0 1 1.06 0L8 8.94l3.47-3.47a.75.75 0 1 1 1.06 1.06l-4 4a.75.75 0 0 1-1.06 0l-4-4a.75.75 0 0 1 0-1.06"/></g>"#,
};
static EXPAND_ALL: Icon = Icon {
    name: "oc-diff-expand-all",
    view: 16.0,
    body: r#"<path fill="currentColor" d="M11.47 9.47a.75.75 0 1 1 1.06 1.06l-4 4a.75.75 0 0 1-1.06 0l-4-4a.75.75 0 1 1 1.06-1.06L8 12.94zM7.526 1.418a.75.75 0 0 1 1.004.052l4 4a.75.75 0 1 1-1.06 1.06L8 3.06 4.53 6.53a.75.75 0 1 1-1.06-1.06l4-4z"/>"#,
};

/// The separator's "N unmodified lines": the page's sans at Pierre's 13 px on the 24 px row.
const SEPARATOR_TEXT: TextStyle = UI.lh(GAP_H);
/// Pierre's 2 px of diff background right of the buttons, and between two stacked ones.
const SEPARATOR_GAP: f32 = 2.0;

const STATUS: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0).tracking(-0.04).features("'tnum' 1, 'lnum' 1");
const FILE_NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04).features("'tnum' 1, 'lnum' 1");
const FILE_PATH: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04).features("'tnum' 1, 'lnum' 1");
const COUNT: TextStyle = TextStyle::new(11.0, 440.0).lh(11.0).tracking(0.05);

/// Default per-file header chrome (`session-review-v2-file-header`, 33 tall with its bottom rule):
/// its status letter, the file's coloured icon, its name and folder, and `+N -M` counts.
pub fn default_file_header(p: &Pal, path: &str, status: FileStatus, adds: usize, dels: usize) -> El {
    let (letter, color) = match status {
        FileStatus::Added => ("A", p.diff.add_text),
        FileStatus::Deleted => ("D", p.diff.del_text),
        _ => ("M", p.info),
    };
    let (dir, name) = opencode_kit::path::split(path);
    let title = row()
        .key("title")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(6.0)
        .center_y()
        .child(el().key("status").size(16.0, 16.0).center().child(text(letter).font(STATUS).color(color)))
        .child(icon(rune_theme_opencode::file_icons::file(path), 16.0).key("file-icon"))
        .child(text(name.to_owned()).key("name").font(FILE_NAME).color(p.text))
        .children((!dir.is_empty()).then(|| text(dir.to_owned()).key("dir").font(FILE_PATH).color(p.muted).shrink().min_w(0.0).truncate_fade()));
    // `DiffChanges` compact: both counts, 8 apart, in 11 px.
    let counts = row()
        .key("counts")
        .gap(8.0)
        .center_y()
        .children((adds + dels > 0).then(|| text(format!("+{adds}")).dir(rune::Direction::Ltr).key("adds").font(COUNT).color(p.success)))
        .children((adds + dels > 0).then(|| text(format!("-{dels}")).dir(rune::Direction::Ltr).key("dels").font(COUNT).color(p.danger)));
    row()
        .key("file-header")
        .w_full()
        .px(16.0)
        .py(8.0)
        .gap(8.0)
        .center_y()
        .fill(p.bg_base)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .border_layout()
        .child(title)
        .child(counts)
}

/// A changed line's bar over its number cell's left edge, the row's full height.
fn bar(look: &CodeLook, kind: LineKind) -> Option<El> {
    let t = tints(look, kind)?;
    Some(el().key("bar").absolute(0.0, 0.0).w(BAR_W).h_full().fill(t.bar))
}

#[allow(clippy::too_many_arguments)]
fn number_cell_base(
    l: &CodeLook,
    st: &DiffDocState,
    i: usize,
    n: Option<u32>,
    side: Side,
    kind: LineKind,
    selected: bool,
    digits: u32,
) -> (El, Option<LineRef>) {
    let unified = st.mode() == DiffMode::Unified;
    // Pierre's `[data-column-number]`: 2ch in, the number right-aligned, 1ch after it; a
    // wrapped line's number on its first row.
    let mut c = row()
        .w(l.number_w(digits))
        .min_h(LINE_H)
        .self_stretch()
        .pr(l.adv)
        .justify_end()
        .items_start()
        .children(bar(l, kind));
    if let Some(n) = n {
        let color = if selected {
            l.selected_number
        } else {
            tints(l, kind).map_or(l.faint, |t| t.number)
        };
        c = c.child(text(n.to_string()).font(l.mono).color(color));
    }
    let at = n.map(|num| LineRef::new(side, num)).or_else(|| {
        let other = if side == Side::Old { Side::New } else { Side::Old };
        unified
            .then(|| st.row_line(st.rows[i], other).map(|num| LineRef::new(other, num)))
            .flatten()
    });
    (c, at)
}

fn code_cell_base(
    l: &CodeLook,
    st: &DiffDocState,
    f: usize,
    c: &CellData<'_>,
    copy: bool,
    side: Side,
    at: Option<LineRef>,
) -> El {
    let (src_side, n) = c.source();
    let line_tints = tints(l, c.kind);
    let mut word_bgs: Vec<(Range<usize>, Color)> = match line_tints {
        Some(t) => c.changes.iter().cloned().map(|r| (r, t.word)).collect(),
        None => Vec::new(),
    };
    word_bgs.extend(
        st.find_line(f, src_side, n)
            .iter()
            .map(|(r, active)| (r.clone(), if *active { l.active_match_bg } else { l.match_bg })),
    );
    let spans = l.spans(c.text.len(), &st.highlights(f, src_side, n), &word_bgs, line_tints.map(|t| (t.word, t.word_text)));
    let (shown, spans) = expand_tabs(c.text, spans);
    let ink = l.plain;
    // Pierre's `[data-diff-span] { border-radius: 3px }` (`@pierre/diffs` `dist/style.js`).
    let mut t = rich(shown, spans).font(l.mono).color(ink).span_radius(3.0).key("text");
    // Pierre's `[data-overflow='wrap'] [data-line] { white-space: pre-wrap; word-break:
    // break-word }`: at spaces, inside a word only when it alone overflows.
    t = if st.wraps() { t.w_full().wrap_anywhere() } else { t.single_line() };
    if copy {
        t = t.selectable(true).selection_color(l.selection);
        if let Some(at) = at {
            t = t.key(crate::notes::text_key(f, Some(side), at.line));
        }
    }
    // `[data-line] { padding-inline: 1ch }`.
    let mut code = col().grow(1.0).min_w(0.0).px(l.adv).child(t);
    // Pierre's `[data-no-newline]` row under the line, in its colour: the code's face and
    // `--diffs-fg` at 60% (`[data-no-newline] span { opacity: .6 }`).
    if c.no_newline {
        code = code.child(text(opencode_i18n::t!("rune.diff.noNewline")).font(l.mono).color(l.plain).opacity(0.6).key("eof"));
    }
    code
}

fn gap_steps(hidden: u32, first: bool, last: bool) -> Vec<(Expand, &'static Icon, String)> {
    match (first, last) {
        (true, _) => vec![(Expand::Up, &EXPAND_DOWN, opencode_i18n::t!("rune.diff.showAbove", count = hidden.min(EXPAND_STEP)))],
        (_, true) => vec![(Expand::Down, &EXPAND_UP, opencode_i18n::t!("rune.diff.showBelow", count = hidden.min(EXPAND_STEP)))],
        _ if hidden <= EXPAND_STEP => vec![(Expand::All, &EXPAND_ALL, opencode_i18n::t!("rune.diff.showHidden", count = hidden))],
        _ => vec![
            (Expand::Down, &EXPAND_UP, opencode_i18n::t!("rune.diff.showBelow", count = EXPAND_STEP)),
            (Expand::Up, &EXPAND_DOWN, opencode_i18n::t!("rune.diff.showAbove", count = EXPAND_STEP)),
        ],
    }
}

#[allow(clippy::too_many_arguments)]
fn gap_step_button_base(p: &Pal, look: &CodeLook, how: Expand, glyph: &'static Icon, label: String, w: f32, h: f32, fill: Color) -> El {
    row()
        .key(format!("{how:?}"))
        .w(w)
        .h(h)
        .fill(fill)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .cursor(Cursor::Pointer)
        .role("button")
        .label(label)
        .child(icon(glyph, 16.0).color(p.faint).group_color(look.plain).absolute((w - 16.0) / 2.0, (h - 16.0) / 2.0))
}

fn gap_words_base(p: &Pal, count: &str) -> El {
    col()
        .key("words")
        .role("button")
        .label(count.to_owned())
        .cursor(Cursor::Pointer)
        .child(text(count.to_owned()).key("count").font(SEPARATOR_TEXT).color(p.faint).group_opacity(1.0, 0.0))
        .child(
            rich(count.to_owned(), vec![Span::new(0, count.len()).underline(p.faint)])
                .key("underlined")
                .font(SEPARATOR_TEXT)
                .color(p.faint)
                .group_opacity(0.0, 1.0)
                .absolute(0.0, 0.0),
        )
}

fn gap_row_expandable(look: &CodeLook, st: &DiffDocState, fill: Color, count: String, w: f32, buttons: Vec<El>, words: El) -> El {
    // The diff's background shows 2 px right of the buttons and between two of them.
    let inner = row()
        .key("inner")
        .h(GAP_H)
        .min_w(0.0)
        .child(col().key("expand").w(w + SEPARATOR_GAP).h(GAP_H).gap(SEPARATOR_GAP).fill(look.bg).children(buttons))
        // Hovering anywhere over the count underlines it; only the words open the range.
        .child(
            row()
                .key("content")
                .h(GAP_H)
                .px(look.adv)
                .center_y()
                .hover_group()
                .hover_motion(INSTANT, INSTANT)
                .cursor(Cursor::Pointer)
                .child(words),
        );
    gap_row_finish(st, fill, count, inner)
}

fn gap_row_static(p: &Pal, st: &DiffDocState, fill: Color, count: String) -> El {
    // With no way to open the range, the count starts 1ch into the gutter.
    let ch = rune::text::measure("0", &[], SEPARATOR_TEXT, None).0;
    let inner = row()
        .key("inner")
        .h(GAP_H)
        .min_w(0.0)
        .child(row().key("content").h(GAP_H).px(ch).center_y().child(text(count.clone()).key("count").font(SEPARATOR_TEXT).color(p.faint)));
    gap_row_finish(st, fill, count, inner)
}

fn gap_row_finish(st: &DiffDocState, fill: Color, count: String, mut inner: El) -> El {
    if let Some(w) = view_w(st) {
        inner = inner.max_w(w).sticky_left(0.0);
    }
    row().w_full().h(GAP_H).fill(fill).role("group").label(count).child(inner)
}

pub(super) fn note_row(l: &CodeLook, st: &DiffDocState, f: usize) -> El {
    let file = &st.files()[f];
    let msg = if file.binary {
        opencode_i18n::t!("rune.diff.binary")
    } else if matches!(file.status, FileStatus::Renamed | FileStatus::Copied) {
        opencode_i18n::t!("rune.diff.moved")
    } else if file.old_mode != file.new_mode {
        opencode_i18n::t!("rune.diff.modeChanged")
    } else if matches!(file.status, FileStatus::Added | FileStatus::Deleted) {
        opencode_i18n::t!("rune.diff.empty")
    } else {
        opencode_i18n::t!("plugin.review.noChanges")
    };
    let mut inner = row()
        .key("inner")
        .h(NOTE_H)
        .center_y()
        .pl(gutter_w(l, st, f) + l.adv)
        .child(text(msg).font(l.mono_small).color(l.muted));
    if let Some(w) = view_w(st) {
        inner = inner.max_w(w).sticky_left(0.0);
    }
    row().w_full().h(NOTE_H).fill(l.bg).role("note").label(msg).child(inner)
}

#[allow(clippy::too_many_arguments)]
fn file_header_wrap(pal: &Pal, st: &DiffDocState, f: usize, sticky: bool, full_path: &str, adds: usize, dels: usize, inner: El) -> El {
    let mut h = col()
        .w_full()
        .h(HEADER_H)
        .role("heading")
        .label(format!("{full_path}, {adds} added, {dels} removed"));
    if let (false, Some(w)) = (sticky, view_w(st)) {
        h = h.w(w).sticky_left(0.0);
    }
    // Stacked under the file before it, a rule as its own bottom one parts them.
    if f > 0 && !sticky {
        h = h.child(el().key("top-rule").w_full().h(1.0).fill(pal.border));
    }
    h.child(inner)
}

impl<S: 'static> Inner<S> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn number_cell(
        &self,
        st: &DiffDocState,
        i: usize,
        f: usize,
        n: Option<u32>,
        side: Side,
        kind: LineKind,
        selected: bool,
        digits: u32,
    ) -> El {
        let (c, at) = number_cell_base(&self.look, st, i, n, side, kind, selected, digits);
        let Some(at) = at else { return c };
        let unified = st.mode() == DiffMode::Unified;
        let get_mut = self.d.get_mut.clone();
        let notes = self.d.notes.clone();
        // Capture the row's stable key, not its index `i`: expanding or collapsing a fold above
        // shifts `st.rows` while the row's memo reuses this handler (`rune::memo` contract).
        let row_key = st.keys[i].clone();
        c.cursor(Cursor::Pointer).on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            let st = get_mut(s);
            let before = st.selection();
            let unselect = st.unselecting();
            let ended = match d.phase {
                Phase::Down => {
                    st.press_line(f, at, tx.mods().shift);
                    false
                }
                Phase::Move => {
                    let Some(row_at) = st.keys.iter().position(|k| k == &row_key).and_then(|r| u32::try_from(r).ok()) else {
                        return;
                    };
                    let Some(r_idx) = st.row_from(row_at, d.y) else { return };
                    let r = st.rows[r_idx as usize];
                    if r.file() != f {
                        return;
                    }
                    let to = st.row_line(r, side).map(|num| LineRef::new(side, num)).or_else(|| {
                        let other = if side == Side::Old { Side::New } else { Side::Old };
                        unified.then(|| st.row_line(r, other).map(|num| LineRef::new(other, num))).flatten()
                    });
                    if let Some(to) = to {
                        st.drag_to(f, to);
                    }
                    false
                }
                Phase::Up if d.cancelled() => {
                    st.cancel_line();
                    false
                }
                Phase::Up => {
                    let _ = st.release_line();
                    true
                }
            };
            let after = st.selection();
            let path = st.files()[f].path().to_owned();
            if let Some(notes) = &notes {
                hand_off(notes.as_ref(), s, &path, before, after, ended.then_some(unselect), tx);
            }
        })
    }

    /// The gutter's `+` for line `n` on `side` of file `f`, on a row (or half) under the pointer.
    pub(super) fn plus(&self, st: &DiffDocState, f: usize, n: Option<u32>, side: Side, quiet: bool, row: El) -> El {
        let (Some(notes), Some(n), false) = (self.d.notes.clone(), n, quiet) else { return row };
        let path = st.files()[f].path().to_owned();
        let cell_w = gutter_w(&self.look, st, f);
        row.hover_group().child(crate::notes::plus_button(
            &self.pal,
            cell_w,
            opencode_i18n::t!("ui.lineComment.submit"),
            move |s: &mut S, tx: &mut Tx| {
                notes.plus(s, &path, Some(side), n, tx);
            },
        ))
    }

    pub(super) fn code_cell(
        &self,
        st: &DiffDocState,
        f: usize,
        c: &CellData<'_>,
        copy: bool,
        side: Side,
        at: Option<LineRef>,
    ) -> El {
        let get_mut = self.d.get_mut.clone();
        let mut code = code_cell_base(&self.look, st, f, c, copy, side, at).on_hover_change(move |s: &mut S, inside, tx| {
            let st = get_mut(s);
            if inside && st.copy_side() != side {
                st.set_copy_side(side);
            } else {
                tx.live_only();
            }
        });
        if let (Some(notes), Some(at)) = (self.d.notes.clone(), at) {
            let get_mut = self.d.get_mut.clone();
            code = code.on_background_press_tx(move |s: &mut S, tx| {
                if !tx.touch_tap() {
                    return;
                }
                let st = get_mut(s);
                let before = st.selection();
                st.click_code(f, at);
                let after = st.selection();
                let path = st.files()[f].path().to_owned();
                hand_off(notes.as_ref(), s, &path, before, after, Some(None), tx);
            });
        }
        code
    }

    /// The unchanged lines between two hunks, folded, as Desktop's review draws them (Pierre's
    /// `line-info-basic` separator on session-ui's 24 px row): buttons across the gutter, then
    /// "N unmodified lines" in the page's sans where the code starts. A range of more than
    /// twenty lines between hunks gets two half-height buttons, each opening twenty more lines at
    /// its end; a shorter one gets one button that opens it all; the first and last ranges get
    /// one button that opens toward their hunk. The count opens twenty lines at each end, and
    /// Shift on a button opens the whole range.
    pub(super) fn gap_row(&self, st: &DiffDocState, f: usize, gap: usize) -> El {
        let p = &self.pal;
        let hidden = st.hidden(f, gap).unwrap_or(0);
        let (first, last) = (gap == 0, gap == st.files()[f].hunks.len());
        let can_expand = hidden > 0 && (st.has_full_text(f) || self.d.on_fetch_text.is_some());
        let fill = p.diff.separator;
        // Pierre's text, which Desktop's review shows untranslated.
        let count = if hidden == 1 { "1 unmodified line".to_owned() } else { format!("{hidden} unmodified lines") };
        if !can_expand {
            return gap_row_static(p, st, fill, count);
        }
        let open = {
            let (get_mut, fetch) = (self.d.get_mut.clone(), self.d.on_fetch_text.clone());
            Rc::new(move |s: &mut S, how: Reveal| {
                if get_mut(s).reveal(f, gap, how) == Some(FoldEffect::FetchText)
                    && let Some(cb) = &fetch
                {
                    cb(s, f);
                }
            })
        };
        let steps = gap_steps(hidden, first, last);
        let w = gutter_w(&self.look, st, f) - SEPARATOR_GAP;
        let h = if steps.len() == 2 { (GAP_H - SEPARATOR_GAP) / 2.0 } else { GAP_H };
        let buttons = steps
            .into_iter()
            .map(|(how, glyph, label)| {
                let open = open.clone();
                gap_step_button_base(p, &self.look, how, glyph, label, w, h, fill).on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    open(s, Reveal::Step(if tx.mods().shift { Expand::All } else { how }));
                })
            })
            .collect();
        let words = gap_words_base(p, &count).on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            open(s, Reveal::Around);
        });
        gap_row_expandable(&self.look, st, fill, count, w, buttons, words)
    }

    /// A file's header (custom via [`DiffViewer::file_header`] or [`default_file_header`]);
    /// pressed, it folds the file.
    pub(super) fn file_header(&self, s: &S, st: &DiffDocState, f: usize, sticky: bool) -> El {
        let file = &st.files()[f];
        let collapsed = st.collapsed(f);
        let get_mut = self.d.get_mut.clone();
        let full_path = file.path();
        let (adds, dels) = (file.additions(), file.deletions());
        let base = match &self.d.file_header {
            Some(build) => build(&self.pal, full_path, file.status, adds, dels),
            None => default_file_header(&self.pal, full_path, file.status, adds, dels),
        };
        let mut inner = base
            .key("inner")
            .role("button")
            .label(if collapsed { opencode_i18n::t!("rune.diff.expandFile") } else { opencode_i18n::t!("rune.diff.collapseFile") })
            .expanded(!collapsed)
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                tx.instant();
                let st = get_mut(s);
                let next = !st.collapsed(f);
                st.set_collapsed(f, next);
                if sticky {
                    st.scroll_to_file(f, tx);
                }
            });
        if let Some(actions) = &self.d.header_actions {
            inner = inner.child(actions(s, f).key("actions"));
        }
        // The header drags the file's changes out as a `.patch` (a chat, an issue, `git apply`).
        let get = self.d.get.clone();
        inner = inner.drag_out(move |s: &S| get(s).files().get(f).and_then(patch_drag));
        file_header_wrap(&self.pal, st, f, sticky, full_path, adds, dels, inner)
    }
}

/// A drag of `file`'s changes: a `<name>.patch` written to the app's drag folder, and the patch
/// as text for a target that takes words.
#[cfg(not(target_arch = "wasm32"))]
fn patch_drag(file: &rune_kit::diff::FileDiff) -> Option<rune::DragItem> {
    let patch = file.patch();
    let name = std::path::Path::new(file.path()).file_name()?.to_string_lossy().into_owned();
    let dir = std::env::temp_dir().join("opencode-drag");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{name}.patch"));
    std::fs::write(&path, patch.as_bytes()).ok()?;
    Some(rune::DragItem::new().file(path).text(patch))
}

#[cfg(target_arch = "wasm32")]
fn patch_drag(file: &rune_kit::diff::FileDiff) -> Option<rune::DragItem> {
    Some(rune::DragItem::new().text(file.patch()))
}
