//! Virtualized diff component [`DiffViewer`], shared by the Review panel and inline tool diffs.

use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use rune::{
    Anim, Color, Cursor, Drag, El, GrowthSpec, Icon, Phase, ScrollSpec, Span, Tx, col, el, icon,
    memo, rich, row, text,
};
use rune_kit::ScrollBar;
use rune_kit::diff::{
    DiffMode, EXPAND_STEP, Expand, FileStatus, LineKind, LineRef, Side,
};

use super::diff_state::{DiffDocState, DiffRow};
use super::fold::{FoldEffect, Reveal};
use super::notes::{LineNotes, hand_off};
use super::{
    BAR_W, CodeLook, EDGE_FADE, DiffTints, GAP_H, HEADER_H, LINE_H, NOTE_H, expand_tabs,
};
use crate::palette::Pal;

const INSTANT: Anim = Anim::INSTANT;

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
const SEPARATOR_TEXT: rune::TextStyle = crate::view::UI.lh(GAP_H);
/// Pierre's 2 px of diff background right of the buttons, and between two stacked ones.
const SEPARATOR_GAP: f32 = 2.0;

type Get<S> = Rc<dyn Fn(&S) -> &DiffDocState>;
type GetMut<S> = Rc<dyn Fn(&mut S) -> &mut DiffDocState>;

#[allow(clippy::type_complexity)]
pub struct DiffViewer<S> {
    key: String,
    get: Get<S>,
    get_mut: GetMut<S>,
    sticky_headers: bool,
    whole: bool,
    on_visible_file: Option<Rc<dyn Fn(&mut S, usize)>>,
    on_fetch_text: Option<Rc<dyn Fn(&mut S, usize)>>,
    header_actions: Option<Rc<dyn Fn(&S, usize) -> El>>,
    notes: Option<Rc<dyn LineNotes<S>>>,
}

impl<S: 'static> DiffViewer<S> {
    pub fn new(
        key: impl Into<String>,
        get: impl Fn(&S) -> &DiffDocState + 'static,
        get_mut: impl Fn(&mut S) -> &mut DiffDocState + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            get: Rc::new(get),
            get_mut: Rc::new(get_mut),
            sticky_headers: true,
            whole: false,
            on_visible_file: None,
            on_fetch_text: None,
            header_actions: None,
            notes: None,
        }
    }

    /// Line comments on the diff's files (`enableLineSelection`, `enableGutterUtility`,
    /// annotations).
    pub fn notes(mut self, notes: Rc<dyn LineNotes<S>>) -> Self {
        self.notes = Some(notes);
        self
    }

    pub fn sticky_headers(mut self, enabled: bool) -> Self {
        self.sticky_headers = enabled;
        self
    }

    /// Every row built at its own height, the diff as tall as they come and not scrolling
    /// itself: a wrapping diff shown whole inside another scroll.
    pub fn whole(mut self, whole: bool) -> Self {
        self.whole = whole;
        self
    }

    pub fn on_visible_file(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.on_visible_file = Some(Rc::new(f));
        self
    }

    /// Called with a file whose whole text a fold waits for (`FoldEffect::FetchText`); the app
    /// loads it into `DiffDocState::attach_file_text`, or `text_failed`.
    pub fn on_fetch_text(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.on_fetch_text = Some(Rc::new(f));
        self
    }

    pub fn header_actions(mut self, f: impl Fn(&S, usize) -> El + 'static) -> Self {
        self.header_actions = Some(Rc::new(f));
        self
    }

    pub fn build(self, s: &S, p: &Pal) -> El {
        let st = (self.get)(s);
        // Desktop draws a diff's rows on `background-stronger` (session-ui's `--diffs-bg`), the
        // panel's own background showing past the last of them.
        let look = CodeLook::new(p);
        let wrap = st.wraps();
        let root_key = self.key.clone();
        let scroll_path = format!("{root_key}/scroll-scroll-area/scroll");
        if !wrap && let Some(r) = rune::memo::layout(&scroll_path) {
            st.set_viewport(r);
        }
        let sticky_on = self.sticky_headers;
        let whole = self.whole && wrap;
        let inner = Rc::new(Inner { look, pal: *p, d: self });
        let count = st.rows.len();
        let list = col().key("rows").w_full().selection_group(true).role("list").label(opencode_i18n::t!("rune.diff.label"));
        if whole {
            let root = col().key(root_key).w_full().fill(p.bg_base).child(list.children((0..count).map(|i| inner.row(s, i))));
            return match inner.d.notes.clone() {
                Some(notes) => root.on_pointer_move_local(move |s: &mut S, _, tx: &mut Tx| notes.pointer_moved(s, tx)),
                None => root,
            };
        }
        let bar = super::scroll_bar(p);
        let mut list = list.growth(GrowthSpec::default()).pr(super::scroll_gutter(&bar, &scroll_path, &format!("{scroll_path}/rows")));
        if !wrap {
            list = list.min_w(inner.content_width(st).max(st.view_width()));
        }
        let build_inner = inner.clone();
        let vis_mut = inner.d.get_mut.clone();
        let on_file = inner.d.on_visible_file.clone();
        let list = list
            .virtual_rows(count, LINE_H, row_keys(inner.d.get.clone()), move |s: &S, i| {
                build_inner.row(s, i)
            })
            .virtual_rows_version(st.keys_rev)
            .on_visible_rows(move |s: &mut S, rows| {
                let (reported, sticky_changed) = vis_mut(s).set_visible_rows(rows);
                if let (Some(f), Some(cb)) = (reported, &on_file) {
                    cb(s, f);
                    return true;
                }
                sticky_changed
            });
        let scroll = col()
            .key("scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false });
        let scroll = if wrap { scroll } else { scroll.scroll_xy().scroll_edge_fade(EDGE_FADE) }.child(list).scroll_bar_styled(bar);

        let mut root = col()
            .key(root_key)
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(p.bg_base)
            .child(scroll);
        if let Some(notes) = inner.d.notes.clone() {
            root = root.on_pointer_move_local(move |s: &mut S, _, tx: &mut Tx| notes.pointer_moved(s, tx));
        }
        if sticky_on && let Some(f) = st.sticky() {
            root = root.child(
                inner
                    .file_header(s, st, f, true)
                    .key("sticky")
                    .pin(Some(0.0), Some(0.0), Some(0.0), None),
            );
        }
        root
    }
}


fn row_keys<S: 'static>(get: Get<S>) -> impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static {
    move |s, i| Cow::Borrowed(get(s).keys[i].as_str())
}

#[derive(Clone, Copy)]
struct CellData<'a> {
    kind: LineKind,
    old: Option<u32>,
    new: Option<u32>,
    text: &'a str,
    changes: &'a [Range<usize>],
    no_newline: bool,
}

impl CellData<'_> {
    fn source(&self) -> (Side, u32) {
        match (self.kind, self.old, self.new) {
            (LineKind::Removed, Some(n), _) => (Side::Old, n),
            (_, _, Some(n)) => (Side::New, n),
            (_, Some(n), None) => (Side::Old, n),
            _ => (Side::New, 1),
        }
    }
}

struct Inner<S> {
    look: CodeLook,
    pal: Pal,
    d: DiffViewer<S>,
}

impl<S: 'static> Inner<S> {
    fn tints(&self, kind: LineKind) -> Option<&DiffTints> {
        match kind {
            LineKind::Added => Some(&self.look.add),
            LineKind::Removed => Some(&self.look.del),
            LineKind::Context => None,
        }
    }

    /// A row's fill, its number cell's too: Pierre draws the cell in the line's colour.
    fn fill(&self, kind: LineKind, selected: bool) -> Color {
        match (selected, self.tints(kind)) {
            (true, _) => self.look.selected,
            (false, Some(t)) => t.line,
            (false, None) => self.look.bg,
        }
    }

    /// Only an unchanged, unselected row lights under the pointer: Pierre mixes a changed one's
    /// fill toward itself, and session-ui sets a selected one's outright (`index.ts:210-216`).
    fn hover(&self, row: El, kind: LineKind, selected: bool, quiet: bool) -> El {
        if selected || quiet || kind != LineKind::Context {
            return row;
        }
        row.hover(self.look.hover).hover_motion(INSTANT, INSTANT)
    }

    /// One unwrapped line's width: its number cell, then its code 1ch in from each side.
    fn content_width(&self, st: &DiffDocState) -> f32 {
        let digits = (0..st.files().len()).map(|f| st.digits(f)).max().unwrap_or(3);
        self.look.number_w(digits) + (st.columns() + 2) as f32 * self.look.adv
    }

    fn view_w(&self, st: &DiffDocState) -> Option<f32> {
        let w = st.view_width();
        (w > 0.0 && !st.wraps()).then_some(w)
    }

    /// Pierre's gutter: one number column, in unified mode as in each half of a split row.
    fn gutter_w(&self, st: &DiffDocState, f: usize) -> f32 {
        self.look.number_w(st.digits(f))
    }

    fn row(&self, s: &S, i: usize) -> El {
        let st = (self.d.get)(s);
        let r = st.rows[i];
        let key = st.keys[i].as_str();
        match r {
            DiffRow::File(f) => self.file_header(s, st, f as usize, false),
            DiffRow::Note(f) => memo(
                key,
                (self.look.version, st.rev, st.view_width().to_bits()),
                || self.note_row(st, f as usize),
            ),
            DiffRow::Gap { f, gap } => memo(
                key,
                (self.look.version, st.rev, st.view_width().to_bits()),
                || self.gap_row(st, f as usize, gap as usize),
            ),
            _ => {
                let notes = self.d.notes.as_ref().map(|n| {
                    let path = st.files()[r.file()].path();
                    let lines = [Side::Old, Side::New].map(|side| st.row_line(r, side).map(|n| (side, n)));
                    let commented = lines.map(|l| l.is_some_and(|(side, line)| n.commented(s, path, side, line)));
                    (n.version(s, path), commented)
                });
                let version = (
                    self.look.version,
                    st.rev,
                    st.row_selected(i),
                    st.copy_side(),
                    st.selection().map(|sel| sel.start.side),
                    notes,
                );
                let commented = notes.map_or([false; 2], |(_, c)| c);
                let quiet = self.d.notes.as_ref().is_some_and(|n| n.plus_quiet(s));
                let get_mut = self.d.get_mut.clone();
                let cursor = st.cursor_row() == Some(i);
                let row = memo(key, (version, cursor), || {
                    let line = self.line_row(st, i, commented, quiet);
                    self.line_stop(st, i, cursor, self.with_annotations(s, st, i, line))
                });
                let key = key.to_owned();
                super::report_height(row, move |s: &mut S, h| get_mut(s).heights.record(&key, h))
            }
        }
    }

    /// Line row `i` as the keyboard and a screen reader meet it: one Tab stop for the whole diff
    /// (the cursor's line; the rest are reached by Up and Down, `DiffDocState::line_key`), named
    /// by what it is and says (added, removed, or a line of context, its number and its code), and
    /// Return or C opens a comment on it, as the `+` beside it does under the pointer.
    fn line_stop(&self, st: &DiffDocState, i: usize, cursor: bool, el: El) -> El {
        let r = st.rows[i];
        let f = r.file();
        let (a, b) = self.cells(st, r);
        let said = |c: &CellData<'_>| {
            let code = c.text.trim();
            match (c.kind, c.old, c.new) {
                (LineKind::Added, _, Some(n)) => opencode_i18n::t!("rune.a11y.diff.added", line = n, code = code),
                (LineKind::Removed, Some(n), _) => opencode_i18n::t!("rune.a11y.diff.removed", line = n, code = code),
                (_, _, n) => opencode_i18n::t!("rune.a11y.diff.line", line = n.or(c.old).unwrap_or(0), code = code),
            }
        };
        let label = [a.as_ref().map(said), b.as_ref().map(said)].into_iter().flatten().collect::<Vec<_>>().join("; ");
        // The line a comment goes on: the new side's, else the old one's.
        let at = b.or(a).and_then(|c| if c.kind == LineKind::Removed { c.old.map(|n| (Side::Old, n)) } else { c.new.map(|n| (Side::New, n)) });
        let get_mut = self.d.get_mut.clone();
        let notes = self.d.notes.clone();
        let path = st.files()[f].path().to_owned();
        el.focusable(true)
            .tab_index(if cursor { 0 } else { -1 })
            .role("listitem")
            .label(label)
            .on_key(move |s: &mut S, ev: &rune::KeyEvent, tx: &mut Tx| {
                let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt);
                let comment = plain && (ev.key == rune::Key::Enter || matches!(&ev.key, rune::Key::Text(t) if t.eq_ignore_ascii_case("c")));
                if comment && let (Some(notes), Some((side, n))) = (&notes, at) {
                    notes.plus(s, &path, Some(side), n, tx);
                    return true;
                }
                get_mut(s).line_key(ev, tx)
            })
    }

    /// Row `i` with the annotations under its lines: one row across, or in a split diff a half
    /// under each side, the side without any empty (Pierre's annotation row and its buffer).
    fn with_annotations(&self, s: &S, st: &DiffDocState, i: usize, line: El) -> El {
        let Some(notes) = &self.d.notes else { return line };
        let r = st.rows[i];
        let f = r.file();
        let path = st.files()[f].path();
        let items = |side: Side| st.row_line(r, side).map_or_else(Vec::new, |n| notes.annotations(s, &self.pal, path, side, n));
        let (old, new) = (items(Side::Old), items(Side::New));
        if old.is_empty() && new.is_empty() {
            return line;
        }
        let gutter = self.gutter_w(st, f);
        let annotation = match (st.mode(), st.single_column(f)) {
            (DiffMode::Split, None) => {
                let half = |k: &str, items: Vec<El>| row().key(k.to_owned()).w_frac(0.5).min_w(0.0).items_stretch().child(el().key("buffer").w(gutter)).child(super::notes::annotation_slots(items));
                row().key("annotation").w_full().items_stretch().fill(super::notes::comment_bg(&self.pal)).child(half("old", old)).child(half("new", new))
            }
            _ => super::notes::annotation_row(&self.pal, gutter, old.into_iter().chain(new).collect()),
        };
        col().w_full().child(line.key("line")).child(annotation)
    }

    fn cells<'a>(
        &self,
        st: &'a DiffDocState,
        r: DiffRow,
    ) -> (Option<CellData<'a>>, Option<CellData<'a>>) {
        let of = |f: u32, h: u32, i: u32| {
            let l = &st.files()[f as usize].hunks[h as usize].lines[i as usize];
            CellData {
                kind: l.kind,
                old: l.old,
                new: l.new,
                text: &l.text,
                changes: &l.changes,
                no_newline: l.no_newline,
            }
        };
        match r {
            DiffRow::Ctx { f, old, new } => {
                let txt = st
                    .side_line(f as usize, Side::New, new)
                    .or_else(|| st.side_line(f as usize, Side::Old, old))
                    .unwrap_or("");
                let c = CellData {
                    kind: LineKind::Context,
                    old: Some(old),
                    new: Some(new),
                    text: txt,
                    changes: &[],
                    no_newline: false,
                };
                (Some(c), None)
            }
            DiffRow::Line { f, h, i } => (Some(of(f, h, i)), None),
            DiffRow::Pair { f, h, del, add } => (Some(of(f, h, del)), Some(of(f, h, add))),
            _ => (None, None),
        }
    }

    fn line_row(&self, st: &DiffDocState, i: usize, commented: [bool; 2], quiet: bool) -> El {
        let r = st.rows[i];
        let f = r.file();
        let (a, b) = self.cells(st, r);
        let Some(a) = a else { return col() };
        // A commented line shows as selected, both halves of its row in a split diff
        // (markCommentedDiffLines, session-ui pierre/commented-lines.ts).
        let selected = st.row_selected(i) || commented.contains(&true);
        match st.mode() {
            DiffMode::Unified => self.unified_line(st, i, f, a, selected, quiet),
            DiffMode::Split => {
                let (old, new) = match (b, a.kind) {
                    (Some(b), _) => (Some(a), Some(b)),
                    (None, LineKind::Removed) => (Some(a), None),
                    (None, LineKind::Added) => (None, Some(a)),
                    (None, LineKind::Context) => (Some(a), Some(a)),
                };
                self.split_line(st, i, f, old, new, selected, quiet)
            }
        }
    }

    /// A unified row: Pierre's one number column, a removed line's old number and any other's
    /// new one, then the code.
    fn unified_line(&self, st: &DiffDocState, i: usize, f: usize, c: CellData<'_>, selected: bool, quiet: bool) -> El {
        let digits = st.digits(f);
        let copy = match c.kind {
            LineKind::Context => true,
            LineKind::Added => st.copy_side() == Side::New,
            LineKind::Removed => st.copy_side() == Side::Old,
        };
        let (side, n) = if c.kind == LineKind::Removed { (Side::Old, c.old) } else { (Side::New, c.new) };
        let mut gutter = row()
            .key("gutter")
            .self_stretch()
            .items_stretch()
            .child(self.number_cell(st, i, f, n, side, c.kind, selected, digits).key("nn"));
        // Unwrapped, the numbers hold to the left edge over the code scrolling under them.
        if !st.wraps() {
            gutter = gutter.fill(self.fill(c.kind, selected)).sticky_left(0.0);
        }
        let line = row()
            .w_full()
            .min_h(LINE_H)
            .items_start()
            .fill(self.fill(c.kind, selected))
            .role("listitem")
            .child(gutter)
            .child(self.code_cell(st, f, &c, copy, side, n.map(|n| LineRef::new(side, n))).key("code"));
        let line = self.plus(st, f, n, side, quiet, line);
        self.hover(line, c.kind, selected, quiet)
    }

    fn split_line<'a>(
        &self,
        st: &'a DiffDocState,
        i: usize,
        f: usize,
        old: Option<CellData<'a>>,
        new: Option<CellData<'a>>,
        selected: bool,
        quiet: bool,
    ) -> El {
        let digits = st.digits(f);
        let single = st.single_column(f);
        let half = |side: Side, c: Option<CellData<'a>>| -> El {
            let l = &self.look;
            let k = if side == Side::Old { "old" } else { "new" };
            let mut h = row()
                .key(k)
                .w_frac(if single.is_some() { 1.0 } else { 0.5 })
                .min_w(0.0)
                .min_h(LINE_H)
                .items_start()
                .self_stretch();
            // Session-ui's `--diffs-bg-context-gutter` where a side has no line
            // (`[data-content-buffer]`, `index.ts:175-178`).
            let Some(mut cell) = c else {
                return h.fill(l.empty_bg);
            };
            let kind = cell.kind;
            // Pierre selects rows: both sides of a split row light (`[data-selected-line]`).
            let n = if side == Side::Old { cell.old } else { cell.new };
            if kind == LineKind::Context {
                cell.text = st.side_line(f, side, n.unwrap_or(1)).unwrap_or(cell.text);
            }
            let copy = st.copy_side() == side;
            h = h
                .fill(self.fill(kind, selected))
                .child(self.number_cell(st, i, f, n, side, kind, selected, digits).key("n"))
                .child(self.code_cell(st, f, &cell, copy, side, n.map(|n| LineRef::new(side, n))).key("code"));
            let h = self.plus(st, f, n, side, quiet, h);
            self.hover(h, kind, selected, quiet)
        };
        // Two halves side by side, nothing between: session-ui takes away the 1 px of diff
        // background Pierre puts between them (`index.ts:167-173`). Desktop's review draws an
        // added or deleted file's one side as a single column.
        let line = row().w_full().min_h(LINE_H).items_stretch();
        match single {
            Some(Side::Old) => line.child(half(Side::Old, old)),
            Some(Side::New) => line.child(half(Side::New, new)),
            None => line.child(half(Side::Old, old)).child(half(Side::New, new)),
        }
    }

    /// A changed line's bar over its number cell's left edge, the row's full height.
    fn bar(&self, kind: LineKind) -> Option<El> {
        let t = self.tints(kind)?;
        Some(el().key("bar").absolute(0.0, 0.0).w(BAR_W).h_full().fill(t.bar))
    }

    #[allow(clippy::too_many_arguments)]
    fn number_cell(
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
        let l = &self.look;
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
            .children(self.bar(kind));
        if let Some(n) = n {
            let color = if selected {
                l.selected_number
            } else {
                self.tints(kind).map_or(l.faint, |t| t.number)
            };
            c = c.child(text(n.to_string()).font(l.mono).color(color));
        }
        let at = n.map(|num| LineRef::new(side, num)).or_else(|| {
            let other = if side == Side::Old { Side::New } else { Side::Old };
            unified
                .then(|| st.row_line(st.rows[i], other).map(|num| LineRef::new(other, num)))
                .flatten()
        });
        let Some(at) = at else { return c };
        let get_mut = self.d.get_mut.clone();
        let notes = self.d.notes.clone();
        let row_at = i as u32;
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
    fn plus(&self, st: &DiffDocState, f: usize, n: Option<u32>, side: Side, quiet: bool, row: El) -> El {
        let (Some(notes), Some(n), false) = (self.d.notes.clone(), n, quiet) else { return row };
        let path = st.files()[f].path().to_owned();
        let cell_w = self.gutter_w(st, f);
        row.hover_group().child(super::notes::plus_button(&self.pal, cell_w, opencode_i18n::t!("ui.lineComment.submit"), move |s: &mut S, tx: &mut Tx| {
            notes.plus(s, &path, Some(side), n, tx);
        }))
    }

    fn code_cell(
        &self,
        st: &DiffDocState,
        f: usize,
        c: &CellData<'_>,
        copy: bool,
        side: Side,
        at: Option<LineRef>,
    ) -> El {
        let l = &self.look;
        let (src_side, n) = c.source();
        let tints = self.tints(c.kind);
        let word_bgs: Vec<(Range<usize>, Color)> = match tints {
            Some(t) => c.changes.iter().cloned().map(|r| (r, t.word)).collect(),
            None => Vec::new(),
        };
        let spans = l.spans(c.text.len(), &st.highlights(f, src_side, n), &word_bgs, tints.map(|t| (t.word, t.word_text)));
        let (shown, spans) = expand_tabs(c.text, spans);
        let ink = l.plain;
        // Pierre's `[data-diff-span] { border-radius: 3px }` (`@pierre/diffs` `dist/style.js`).
        let mut t = rich(shown, spans).font(l.mono).color(ink).span_radius(3.0).key("text");
        // Pierre's `[data-overflow='wrap'] [data-line] { white-space: pre-wrap; word-break:
        // break-word }`: at spaces, inside a word only when it alone overflows.
        t = if st.wraps() { t.w_full().wrap_anywhere() } else { t.single_line() };
        if copy {
            t = t.selectable(true).selection_color(l.selection);
        }
        let get_mut = self.d.get_mut.clone();
        // `[data-line] { padding-inline: 1ch }`.
        let mut code = col()
            .grow(1.0)
            .min_w(0.0)
            .px(l.adv)
            .child(t)
            .on_hover_change(move |s: &mut S, inside, tx| {
                let st = get_mut(s);
                if inside && st.copy_side() != side {
                    st.set_copy_side(side);
                } else {
                    tx.live_only();
                }
            });
        // Pierre's `[data-no-newline]` row under the line, in its colour: the code's face and
        // `--diffs-fg` at 60% (`[data-no-newline] span { opacity: .6 }`).
        if c.no_newline {
            code = code.child(text(opencode_i18n::t!("rune.diff.noNewline")).font(l.mono).color(l.plain).opacity(0.6).key("eof"));
        }
        // With line comments, a click on a line's code selects it, or clears it when it was the
        // selection (`selection-machine.ts`, the `text` column).
        if let (Some(notes), Some(at)) = (self.d.notes.clone(), at) {
            let get_mut = self.d.get_mut.clone();
            // The pointer's click, not a control: the keyboard selects and comments by the line's row.
            code = code.on_background_press_tx(move |s: &mut S, tx: &mut Tx| {
                tx.instant();
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
    fn gap_row(&self, st: &DiffDocState, f: usize, gap: usize) -> El {
        let p = &self.pal;
        let hidden = st.hidden(f, gap).unwrap_or(0);
        let (first, last) = (gap == 0, gap == st.files()[f].hunks.len());
        let can_expand = hidden > 0 && (st.has_full_text(f) || self.d.on_fetch_text.is_some());
        let fill = p.diff.separator;
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
        // Pierre's text, which Desktop's review shows untranslated.
        let count = if hidden == 1 { "1 unmodified line".to_owned() } else { format!("{hidden} unmodified lines") };
        let mut inner = row().key("inner").h(GAP_H).min_w(0.0);
        if can_expand {
            let steps: Vec<(Expand, &'static Icon, String)> = match (first, last) {
                (true, _) => vec![(Expand::Up, &EXPAND_DOWN, opencode_i18n::t!("rune.diff.showAbove", count = hidden.min(EXPAND_STEP)))],
                (_, true) => vec![(Expand::Down, &EXPAND_UP, opencode_i18n::t!("rune.diff.showBelow", count = hidden.min(EXPAND_STEP)))],
                _ if hidden <= EXPAND_STEP => vec![(Expand::All, &EXPAND_ALL, opencode_i18n::t!("rune.diff.showHidden", count = hidden))],
                _ => vec![
                    (Expand::Down, &EXPAND_UP, opencode_i18n::t!("rune.diff.showBelow", count = EXPAND_STEP)),
                    (Expand::Up, &EXPAND_DOWN, opencode_i18n::t!("rune.diff.showAbove", count = EXPAND_STEP)),
                ],
            };
            let w = self.gutter_w(st, f) - SEPARATOR_GAP;
            let h = if steps.len() == 2 { (GAP_H - SEPARATOR_GAP) / 2.0 } else { GAP_H };
            let buttons = steps.into_iter().map(|(how, glyph, label)| {
                let open = open.clone();
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
                    .child(icon(glyph, 16.0).color(p.faint).group_color(self.look.plain).absolute((w - 16.0) / 2.0, (h - 16.0) / 2.0))
                    .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                        tx.instant();
                        open(s, Reveal::Step(if tx.mods().shift { Expand::All } else { how }));
                    })
            });
            // The diff's background shows 2 px right of the buttons and between two of them.
            inner = inner.child(col().key("expand").w(w + SEPARATOR_GAP).h(GAP_H).gap(SEPARATOR_GAP).fill(self.look.bg).children(buttons));
            // Hovering anywhere over the count underlines it; only the words open the range.
            let words = col()
                .key("words")
                .role("button")
                .label(count.clone())
                .cursor(Cursor::Pointer)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    open(s, Reveal::Around);
                })
                .child(text(count.clone()).key("count").font(SEPARATOR_TEXT).color(p.faint).group_opacity(1.0, 0.0))
                .child(
                    rich(count.clone(), vec![Span::new(0, count.len()).underline(p.faint)])
                        .key("underlined")
                        .font(SEPARATOR_TEXT)
                        .color(p.faint)
                        .group_opacity(0.0, 1.0)
                        .absolute(0.0, 0.0),
                );
            inner = inner.child(
                row()
                    .key("content")
                    .h(GAP_H)
                    .px(self.look.adv)
                    .center_y()
                    .hover_group()
                    .hover_motion(INSTANT, INSTANT)
                    .cursor(Cursor::Pointer)
                    .child(words),
            );
        } else {
            // With no way to open the range, the count starts 1ch into the gutter.
            let ch = rune::text::measure("0", &[], SEPARATOR_TEXT, None).0;
            inner = inner.child(row().key("content").h(GAP_H).px(ch).center_y().child(text(count.clone()).key("count").font(SEPARATOR_TEXT).color(p.faint)));
        }
        if let Some(w) = self.view_w(st) {
            inner = inner.max_w(w).sticky_left(0.0);
        }
        row().w_full().h(GAP_H).fill(fill).role("group").label(count).child(inner)
    }

    fn note_row(&self, st: &DiffDocState, f: usize) -> El {
        let l = &self.look;
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
            .pl(self.gutter_w(st, f) + l.adv)
            .child(text(msg).font(l.mono_small).color(l.muted));
        if let Some(w) = self.view_w(st) {
            inner = inner.max_w(w).sticky_left(0.0);
        }
        row().w_full().h(NOTE_H).fill(l.bg).role("note").label(msg).child(inner)
    }

    /// A file's header, as Review's (`review::chrome::file_header`); pressed, it folds the file.
    fn file_header(&self, s: &S, st: &DiffDocState, f: usize, sticky: bool) -> El {
        let file = &st.files()[f];
        let collapsed = st.collapsed(f);
        let get_mut = self.d.get_mut.clone();
        let full_path = file.path();
        let (adds, dels) = (file.additions(), file.deletions());
        let mut inner = crate::review::chrome::file_header(&self.pal, full_path, file.status, adds as usize, dels as usize)
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
        let mut h = col()
            .w_full()
            .h(HEADER_H)
            .role("heading")
            .label(format!("{full_path}, {adds} added, {dels} removed"));
        if let (false, Some(w)) = (sticky, self.view_w(st)) {
            h = h.w(w).sticky_left(0.0);
        }
        // Stacked under the file before it, a rule as its own bottom one parts them.
        if f > 0 && !sticky {
            h = h.child(el().key("top-rule").w_full().h(1.0).fill(self.pal.border));
        }
        h.child(inner)
    }
}

/// A drag of `file`'s changes: a `<name>.patch` written to the app's drag folder, and the patch
/// as text for a target that takes words.
fn patch_drag(file: &rune_kit::diff::FileDiff) -> Option<rune::DragItem> {
    let patch = file.patch();
    let name = std::path::Path::new(file.path()).file_name()?.to_string_lossy().into_owned();
    let dir = std::env::temp_dir().join("opencode-drag");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{name}.patch"));
    std::fs::write(&path, patch.as_bytes()).ok()?;
    Some(rune::DragItem::new().file(path).text(patch))
}
