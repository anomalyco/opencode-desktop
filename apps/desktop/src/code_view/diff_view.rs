//! Virtualized diff component [`DiffViewer`], shared by the Review panel and inline tool diffs.

use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use rune::{
    Anim, Color, Cursor, Drag, El, GrowthSpec, Icon, Phase, ScrollSpec, Tx, col, el, icon, memo,
    rich, row, text,
};
use rune_kit::diff::{
    DiffMode, EXPAND_STEP, Expand, FileStatus, LineKind, LineRef, Side,
};

use super::diff_state::{DiffDocState, DiffRow};
use super::{
    BAR_W, CODE_PAD, CodeLook, EDGE_FADE, DiffTints, GAP_H, HEADER_H, LINE_H, NOTE_H, expand_tabs,
    layer_spans,
};
use crate::palette::Pal;

const INSTANT: Anim = Anim::INSTANT;

static EXPAND_UP: Icon = Icon {
    name: "oc-diff-expand-up",
    view: 16.0,
    body: r#"<path d="M4.5 9.5L8 6L11.5 9.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static EXPAND_DOWN: Icon = Icon {
    name: "oc-diff-expand-down",
    view: 16.0,
    body: r#"<path d="M4.5 6.5L8 10L11.5 6.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static EXPAND_ALL: Icon = Icon {
    name: "oc-diff-expand-all",
    view: 16.0,
    body: r#"<path d="M5 5.5L8 2.5L11 5.5M5 10.5L8 13.5L11 10.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

type Get<S> = Rc<dyn Fn(&S) -> &DiffDocState>;
type GetMut<S> = Rc<dyn Fn(&mut S) -> &mut DiffDocState>;

#[allow(clippy::type_complexity)]
pub struct DiffViewer<S> {
    key: String,
    get: Get<S>,
    get_mut: GetMut<S>,
    sticky_headers: bool,
    on_visible_file: Option<Rc<dyn Fn(&mut S, usize)>>,
    on_request_expand: Option<Rc<dyn Fn(&mut S, usize, usize, Expand)>>,
    header_actions: Option<Rc<dyn Fn(&S, usize) -> El>>,
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
            on_visible_file: None,
            on_request_expand: None,
            header_actions: None,
        }
    }

    pub fn sticky_headers(mut self, enabled: bool) -> Self {
        self.sticky_headers = enabled;
        self
    }

    pub fn on_visible_file(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.on_visible_file = Some(Rc::new(f));
        self
    }

    pub fn on_request_expand(
        mut self,
        f: impl Fn(&mut S, usize, usize, Expand) + 'static,
    ) -> Self {
        self.on_request_expand = Some(Rc::new(f));
        self
    }

    pub fn header_actions(mut self, f: impl Fn(&S, usize) -> El + 'static) -> Self {
        self.header_actions = Some(Rc::new(f));
        self
    }

    pub fn build(self, s: &S, p: &Pal) -> El {
        let st = (self.get)(s);
        let look = CodeLook::new(p);
        let wrap = st.wraps();
        let root_key = self.key.clone();
        if !wrap && let Some(r) = rune::memo::layout(&format!("{root_key}/scroll")) {
            st.set_viewport(r);
        }
        let sticky_on = self.sticky_headers;
        let inner = Rc::new(Inner { look, pal: *p, d: self });
        let count = st.rows.len();
        let mut list = col()
            .key("rows")
            .w_full()
            .growth(GrowthSpec::default())
            .selection_group(true)
            .role("list")
            .label(opencode_i18n::t!("rune.diff.label"));
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
        let scroll = if wrap { scroll } else { scroll.scroll_xy().scroll_edge_fade(EDGE_FADE) }.child(list);

        let mut root = col()
            .key(root_key)
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(inner.look.bg)
            .child(scroll);
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

    fn gutter_fill(&self, kind: LineKind, selected: bool) -> Color {
        match (selected, self.tints(kind)) {
            (true, _) => self.look.selected_gutter,
            (false, Some(t)) => t.gutter,
            (false, None) => self.look.bg,
        }
    }

    fn content_width(&self, st: &DiffDocState) -> f32 {
        let digits = (0..st.files().len()).map(|f| st.digits(f)).max().unwrap_or(3);
        BAR_W
            + 2.0 * self.look.number_w(digits)
            + st.columns() as f32 * self.look.adv
            + CODE_PAD.0
            + CODE_PAD.1
    }

    fn view_w(&self, st: &DiffDocState) -> Option<f32> {
        let w = st.view_width();
        (w > 0.0 && !st.wraps()).then_some(w)
    }

    fn gutter_w(&self, st: &DiffDocState, f: usize) -> f32 {
        let n = self.look.number_w(st.digits(f));
        BAR_W + if st.mode() == DiffMode::Unified { 2.0 * n } else { n }
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
                let version = (
                    self.look.version,
                    st.rev,
                    st.row_selected(i, None),
                    st.copy_side(),
                    st.selection().map(|sel| sel.start.side),
                );
                memo(key, version, || self.line_row(st, i))
            }
        }
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

    fn line_row(&self, st: &DiffDocState, i: usize) -> El {
        let r = st.rows[i];
        let f = r.file();
        let (a, b) = self.cells(st, r);
        let Some(a) = a else { return col() };
        match st.mode() {
            DiffMode::Unified => self.unified_line(st, i, f, a),
            DiffMode::Split => {
                let (old, new) = match (b, a.kind) {
                    (Some(b), _) => (Some(a), Some(b)),
                    (None, LineKind::Removed) => (Some(a), None),
                    (None, LineKind::Added) => (None, Some(a)),
                    (None, LineKind::Context) => (Some(a), Some(a)),
                };
                self.split_line(st, i, f, old, new)
            }
        }
    }

    fn unified_line(&self, st: &DiffDocState, i: usize, f: usize, c: CellData<'_>) -> El {
        let l = &self.look;
        let selected = st.row_selected(i, None);
        let fill = if selected {
            l.selected
        } else {
            self.tints(c.kind).map_or(l.bg, |t| t.line)
        };
        let digits = st.digits(f);
        let copy = match c.kind {
            LineKind::Context => true,
            LineKind::Added => st.copy_side() == Side::New,
            LineKind::Removed => st.copy_side() == Side::Old,
        };
        let hover_side = if c.kind == LineKind::Removed { Side::Old } else { Side::New };
        let mut gutter = row()
            .key("gutter")
            .self_stretch()
            .items_stretch()
            .fill(self.gutter_fill(c.kind, selected))
            .child(self.bar(c.kind))
            .child(self.number_cell(st, i, f, c.old, Side::Old, c.kind, selected, digits).key("no"))
            .child(self.number_cell(st, i, f, c.new, Side::New, c.kind, selected, digits).key("nn"));
        if !st.wraps() {
            gutter = gutter.sticky_left(0.0);
        }
        row()
            .w_full()
            .min_h(LINE_H)
            .items_start()
            .fill(fill)
            .hover(l.hover)
            .hover_motion(INSTANT, INSTANT)
            .role("listitem")
            .child(gutter)
            .child(self.code_cell(st, f, &c, copy, hover_side).key("code"))
    }

    fn split_line<'a>(
        &self,
        st: &'a DiffDocState,
        i: usize,
        f: usize,
        old: Option<CellData<'a>>,
        new: Option<CellData<'a>>,
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
            let Some(mut cell) = c else {
                return h.fill(l.empty_bg);
            };
            let kind = cell.kind;
            let selected = st.row_selected(i, Some(side));
            let fill = if selected {
                l.selected
            } else {
                self.tints(kind).map_or(l.bg, |t| t.line)
            };
            let n = if side == Side::Old { cell.old } else { cell.new };
            if kind == LineKind::Context {
                cell.text = st.side_line(f, side, n.unwrap_or(1)).unwrap_or(cell.text);
            }
            let copy = st.copy_side() == side;
            h = h
                .fill(fill)
                .hover(l.hover)
                .hover_motion(INSTANT, INSTANT)
                .role("listitem")
                .child(self.bar(kind))
                .child(self.number_cell(st, i, f, n, side, kind, selected, digits).key("n"))
                .child(self.code_cell(st, f, &cell, copy, side).key("code"));
            h
        };
        let line = row().w_full().min_h(LINE_H).items_stretch();
        // Desktop's review draws an added or deleted file's one side as a single column.
        match single {
            Some(Side::Old) => line.child(half(Side::Old, old)),
            Some(Side::New) => line.child(half(Side::New, new)),
            None => {
                let divider = col().key("divider").w(0.5).self_stretch().fill(self.look.border_muted);
                line.child(half(Side::Old, old)).child(divider).child(half(Side::New, new))
            }
        }
    }

    fn bar(&self, kind: LineKind) -> El {
        let b = col().key("bar").w(BAR_W).self_stretch();
        match self.tints(kind) {
            Some(t) => b.fill(t.bar),
            None => b,
        }
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
        let mut c = row()
            .w(l.number_w(digits))
            .min_h(LINE_H)
            .self_stretch()
            .px(6.0)
            .justify_end()
            .items_start();
        if !unified {
            c = c.fill(self.gutter_fill(kind, selected));
        }
        if let Some(n) = n {
            let color = if selected {
                l.text
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
        let row_at = i as u32;
        c.cursor(Cursor::Pointer).on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            let st = get_mut(s);
            match d.phase {
                Phase::Down => st.press_line(f, at),
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
                }
                Phase::Up => {
                    if d.cancelled() {
                        st.cancel_line();
                    } else {
                        let _ = st.release_line();
                    }
                }
            }
        })
    }

    fn code_cell(
        &self,
        st: &DiffDocState,
        f: usize,
        c: &CellData<'_>,
        copy: bool,
        side: Side,
    ) -> El {
        let l = &self.look;
        let (src_side, n) = c.source();
        let syntax: Vec<(Range<usize>, Color)> = st
            .highlights(f, src_side, n)
            .into_iter()
            .map(|(r, p)| (r, l.syntax.paint(p)))
            .collect();
        let word_bgs: Vec<(Range<usize>, Color)> = match self.tints(c.kind) {
            Some(t) => c.changes.iter().cloned().map(|r| (r, t.word)).collect(),
            None => Vec::new(),
        };
        let spans = layer_spans(c.text.len(), &syntax, &word_bgs);
        let (shown, spans) = expand_tabs(c.text, spans);
        let mut t = rich(shown, spans).font(l.mono).color(l.text).key("text");
        t = if st.wraps() { t.w_full() } else { t.single_line() };
        if copy {
            t = t.selectable(true).selection_color(l.selection);
        }
        let get_mut = self.d.get_mut.clone();
        let mut code = col()
            .grow(1.0)
            .min_w(0.0)
            .pl(CODE_PAD.0)
            .pr(CODE_PAD.1)
            .child(t)
            .on_hover_change(move |s: &mut S, inside, tx| {
                let st = get_mut(s);
                if inside && st.copy_side() != side {
                    st.set_copy_side(side);
                } else {
                    tx.live_only();
                }
            });
        if c.no_newline {
            code = code.child(
                text(opencode_i18n::t!("rune.diff.noNewline"))
                    .font(l.mono_small)
                    .color(l.muted)
                    .opacity(0.6)
                    .key("eof")
                    .pb(2.0),
            );
        }
        code
    }

    fn gap_row(&self, st: &DiffDocState, f: usize, gap: usize) -> El {
        let l = &self.look;
        let file = &st.files()[f];
        let hidden = st.hidden(f, gap).unwrap_or(0);
        let last = gap == file.hunks.len();
        let can_expand = st.has_full_text(f) || self.d.on_request_expand.is_some();
        let mut buttons = Vec::new();
        if can_expand && hidden > 0 {
            let mut add = |how: Expand, glyph: &'static Icon, label: String| {
                let get_mut = self.d.get_mut.clone();
                let req = self.d.on_request_expand.clone();
                buttons.push(
                    row()
                        .key(format!("{how:?}"))
                        .size(22.0, 22.0)
                        .center()
                        .radius(5.0)
                        .hover(l.hover)
                        .hover_motion(INSTANT, INSTANT)
                        .cursor(Cursor::Pointer)
                        .role("button")
                        .label(label)
                        .child(icon(glyph, 14.0).color(l.muted))
                        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                            tx.instant();
                            if !get_mut(s).expand(f, gap, how)
                                && let Some(cb) = &req
                            {
                                cb(s, f, gap, how);
                            }
                        }),
                );
            };
            let step = hidden.min(EXPAND_STEP);
            if hidden <= EXPAND_STEP {
                add(Expand::All, &EXPAND_ALL, opencode_i18n::t!("rune.diff.showHidden", count = hidden));
            } else {
                if gap > 0 {
                    add(Expand::Down, &EXPAND_DOWN, opencode_i18n::t!("rune.diff.showBelow", count = step));
                }
                if !last {
                    add(Expand::Up, &EXPAND_UP, opencode_i18n::t!("rune.diff.showAbove", count = step));
                }
            }
        }
        let btn_box = row()
            .key("expand")
            .w(self.gutter_w(st, f))
            .h(GAP_H)
            .center()
            .gap(2.0)
            .children(buttons);
        let mut inner = row()
            .key("inner")
            .h(GAP_H)
            .min_w(0.0)
            .center_y()
            .gap(10.0)
            .pr(12.0)
            .child(btn_box);
        if let Some(w) = self.view_w(st) {
            inner = inner.max_w(w).sticky_left(0.0);
        }
        if hidden > 0 {
            // Pierre's separator text, which Desktop's review shows untranslated.
            let count_lbl = if hidden == 1 {
                "1 unmodified line".to_owned()
            } else {
                format!("{hidden} unmodified lines")
            };
            let count_el = text(count_lbl).font(l.mono_small).color(l.muted).key("count");
            let count_wrap = if can_expand && hidden > EXPAND_STEP {
                let get_mut = self.d.get_mut.clone();
                let req = self.d.on_request_expand.clone();
                row()
                    .key("all")
                    .px(6.0)
                    .h(22.0)
                    .center_y()
                    .radius(5.0)
                    .hover(l.hover)
                    .hover_motion(INSTANT, INSTANT)
                    .cursor(Cursor::Pointer)
                    .role("button")
                    .label(opencode_i18n::t!("rune.diff.showAll", count = hidden))
                    .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                        tx.instant();
                        if !get_mut(s).expand(f, gap, Expand::All)
                            && let Some(cb) = &req
                        {
                            cb(s, f, gap, Expand::All);
                        }
                    })
                    .child(count_el)
            } else {
                row().key("all").px(6.0).center_y().child(count_el)
            };
            inner = inner.child(count_wrap);
        }
        row().w_full().h(GAP_H).center_y().fill(l.gap_bg).role("group").child(inner)
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
            .pl(self.gutter_w(st, f) + CODE_PAD.0)
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
