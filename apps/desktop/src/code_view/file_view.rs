//! Virtualized single-file viewer [`CodeViewer`], sharing [`CodeLook`] and syntax highlighting with [`DiffViewer`](super::DiffViewer).

use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use rune::{
    Anim, Color, Cursor, Drag, El, GrowthSpec, Phase, ScrollSpec, Tx, col, memo, rich, row, text,
};

use rune_kit::ScrollBar;
use rune_kit::diff::Side;

use super::file_state::{CodeDocState, FileLineSelection};
use super::notes::{Handoff, LineNotes};
use super::{CodeLook, EDGE_FADE, LINE_H, expand_tabs};
use crate::palette::Pal;

const INSTANT: Anim = Anim::INSTANT;

type Get<S> = Rc<dyn Fn(&S) -> &CodeDocState>;
type GetMut<S> = Rc<dyn Fn(&mut S) -> &mut CodeDocState>;

pub struct CodeViewer<S> {
    key: String,
    get: Get<S>,
    get_mut: GetMut<S>,
    notes: Option<Rc<dyn LineNotes<S>>>,
}

impl<S: 'static> CodeViewer<S> {
    pub fn new(
        key: impl Into<String>,
        get: impl Fn(&S) -> &CodeDocState + 'static,
        get_mut: impl Fn(&mut S) -> &mut CodeDocState + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            get: Rc::new(get),
            get_mut: Rc::new(get_mut),
            notes: None,
        }
    }

    /// Line comments on the file (`enableLineSelection`, `enableGutterUtility`, annotations).
    pub fn notes(mut self, notes: Rc<dyn LineNotes<S>>) -> Self {
        self.notes = Some(notes);
        self
    }

    pub fn build(self, s: &S, p: &Pal) -> El {
        let st = (self.get)(s);
        let look = CodeLook::new(p);

        let root_key = self.key.clone();
        let scroll_key = format!("{root_key}-scroll");

        if !st.ready() {
            return col()
                .key(root_key)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .fill(p.bg_base)
                .center()
                .child(text(opencode_i18n::t!("rune.file.loading")).font(look.label).color(look.muted));
        }
        // A read that failed says so, and why, rather than "not found" (`files::FilePayload::Failed`).
        if let Some((what, why)) = st.failure() {
            return col()
                .key(root_key)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .fill(p.bg_base)
                .center()
                .gap(4.0)
                .px(24.0)
                .child(text(what.clone()).key("file-read-failed").font(look.label).color(p.text))
                .child(text(why.clone()).font(look.label).color(look.muted).wrap().max_w(448.0).align(rune::Align::Center));
        }
        if st.missing() {
            let name = opencode_kit::path::basename(st.path());
            return col()
                .key(root_key)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .fill(p.bg_base)
                .center()
                .child(text(opencode_i18n::t!("plugin.file.tab.notFound", name = name)).font(look.label).color(look.muted));
        }
        if st.binary() {
            return col()
                .key(root_key)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .fill(p.bg_base)
                .center()
                .child(
                    text(opencode_i18n::t!("rune.file.binary", size = st.size_bytes()))
                        .font(look.label)
                        .color(look.muted),
                );
        }

        let wrap = st.wrap();
        if !wrap && let Some(r) = rune::memo::layout(&scroll_key) {
            st.set_viewport(r);
        }

        let count = st.line_count();
        let inner = Rc::new(Inner { look, pal: *p, v: self });
        // Pierre's file starts at its first line (`--diffs-gap-block: 0`, session-ui
        // `pierre/index.ts:329`), and Desktop's file tab leaves 160 px of the panel under the last
        // (`gui-plugins/src/file/view.tsx:480`, `pb-40`).
        let bar = super::scroll_bar(p);
        let mut list = col()
            .key("rows")
            .w_full()
            .pb(160.0)
            .pr(super::scroll_gutter(&bar, &scroll_key, &format!("{scroll_key}/rows")))
            .growth(GrowthSpec::default())
            .selection_group(true)
            .role("list")
            .label(opencode_i18n::t!("rune.file.contents"));
        if !wrap {
            list = list.min_w(inner.content_width(st).max(st.view_width()));
        }
        let build_inner = inner.clone();
        let list = list
            .virtual_rows(
                count,
                LINE_H,
                |_s: &S, i| Cow::Owned(format!("cl{}", i + 1)),
                move |s: &S, i| build_inner.row(s, i),
            )
            .virtual_rows_version(st.keys_rev);

        let scroll = col()
            .key(scroll_key)
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false });
        let scroll = if wrap { scroll } else { scroll.scroll_xy().scroll_edge_fade(EDGE_FADE) }.child(list).scroll_bar_styled(bar);

        // The rows on `background-stronger`, the panel's own background under them.
        let root = col().key(root_key).w_full().grow(1.0).min_h(0.0).fill(p.bg_base).child(scroll);
        match inner.v.notes.clone() {
            Some(notes) => root.on_pointer_move_local(move |s: &mut S, _, tx: &mut Tx| notes.pointer_moved(s, tx)),
            None => root,
        }
    }
}

struct Inner<S> {
    look: CodeLook,
    pal: Pal,
    v: CodeViewer<S>,
}

impl<S: 'static> Inner<S> {
    /// Line `n` as the keyboard and a screen reader meet it: the view's one Tab stop is the
    /// cursor's line, Up and Down move it (`CodeDocState::line_key`), each line says its number
    /// and its code, and Return or C opens a comment on it, as the `+` does under the pointer.
    fn line_stop(&self, st: &CodeDocState, n: u32, cursor: bool, el: El) -> El {
        let label = opencode_i18n::t!("rune.a11y.diff.line", line = n, code = st.line(n).unwrap_or("").trim());
        let get_mut = self.v.get_mut.clone();
        let notes = self.v.notes.clone();
        let path = st.path().to_owned();
        el.focusable(true)
            .tab_index(if cursor { 0 } else { -1 })
            .role("listitem")
            .label(label)
            .on_key(move |s: &mut S, ev: &rune::KeyEvent, tx: &mut Tx| {
                let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt);
                let comment = plain && (ev.key == rune::Key::Enter || matches!(&ev.key, rune::Key::Text(t) if t.eq_ignore_ascii_case("c")));
                if comment && let Some(notes) = &notes {
                    notes.plus(s, &path, None, n, tx);
                    return true;
                }
                let st = get_mut(s);
                st.cursor = n;
                st.line_key(ev, tx)
            })
    }

    fn content_width(&self, st: &CodeDocState) -> f32 {
        self.look.number_w(st.digits()) + (st.columns() + 2) as f32 * self.look.adv
    }

    fn row(&self, s: &S, i: usize) -> El {
        let st = (self.v.get)(s);
        let n = (i + 1) as u32;
        // A commented line shows as selected (`[data-comment-selected]`, session-ui
        // `pierre/index.ts:180-196`).
        // Rows build as they lay out, from the state then: the comments are read here.
        let notes_version = self.v.notes.as_ref().map_or(0, |notes| notes.version(s, st.path()));
        let commented = self.v.notes.as_ref().is_some_and(|notes| notes.commented(s, st.path(), Side::New, n));
        let selected = st.selection().is_some_and(|sel| sel.contains(n)) || commented;
        let has_matches = !st.line_matches(n).is_empty();
        let key = format!("cl{n}");
        let get_mut = self.v.get_mut.clone();
        let cursor = st.cursor == n;
        let row = memo(
            key.as_str(),
            (self.look.version, st.rev, selected, has_matches, notes_version, cursor),
            || {
                let quiet = self.v.notes.as_ref().is_some_and(|notes| notes.plus_quiet(s));
                let line = self.line_row(st, n, selected, quiet);
                let items = self.v.notes.as_ref().map_or_else(Vec::new, |notes| notes.annotations(s, &self.pal, st.path(), Side::New, n));
                let row = if items.is_empty() {
                    line
                } else {
                    col().w_full().child(line.key("line")).child(super::notes::annotation_row(&self.pal, self.look.number_w(st.digits()), items))
                };
                self.line_stop(st, n, cursor, row)
            },
        );
        super::report_height(row, move |s: &mut S, h| get_mut(s).heights.record(&key, h))
    }

    fn line_row(&self, st: &CodeDocState, n: u32, selected: bool, quiet: bool) -> El {
        let l = &self.look;
        let fill = if selected { l.selected } else { l.bg };
        let num_color = if selected { l.selected_number } else { l.faint };
        let get_mut = self.v.get_mut.clone();
        let notes = self.v.notes.clone();
        // Pierre's number cell, as a diff's: 2ch in, at least 3ch, 1ch after
        // (`--opencode-diffs-number-inset`, session-ui `pierre/index.ts:270-272`).
        let mut gutter = row()
            .key("gutter")
            .w(l.number_w(st.digits()))
            .min_h(LINE_H)
            .self_stretch()
            .pr(l.adv)
            .justify_end()
            .items_start()
            .cursor(Cursor::Pointer)
            .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
                tx.instant();
                let st = get_mut(s);
                let before = st.selection();
                let unselect = st.unselecting();
                let path = st.path().to_owned();
                let ended = match d.phase {
                    Phase::Down => {
                        st.press_line(n, tx.mods().shift);
                        false
                    }
                    Phase::Move => {
                        st.drag_to_offset(n, d.y);
                        false
                    }
                    Phase::Up if d.cancelled() => {
                        st.cancel_line();
                        false
                    }
                    Phase::Up => {
                        st.release_line();
                        true
                    }
                };
                let after = st.selection().map(file_range);
                let Some(notes) = &notes else { return };
                if let (true, Some(line)) = (ended, unselect) {
                    return notes.handoff(s, &path, Handoff::Unselected(file_range(line)), tx);
                }
                if before.map(file_range) != after {
                    notes.handoff(s, &path, Handoff::Selected(after), tx);
                }
                if ended {
                    notes.handoff(s, &path, Handoff::Ended(after), tx);
                }
            })
            .child(text(n.to_string()).font(l.mono).color(num_color));
        // Unwrapped, the numbers hold to the left edge over the code scrolling under them.
        if !st.wrap() {
            gutter = gutter.fill(fill).sticky_left(0.0);
        }

        let raw = st.line(n).unwrap_or("");

        let match_bgs: Vec<(Range<usize>, Color)> = st
            .line_matches(n)
            .iter()
            .map(|(r, active)| {
                (r.clone(), if *active { l.active_match_bg } else { l.match_bg })
            })
            .collect();
        let spans = l.spans(raw.len(), &st.highlights(n), &match_bgs, None);
        let (shown, spans) = expand_tabs(raw, spans);
        let mut code_txt = rich(shown, spans)
            .font(l.mono)
            .color(l.plain)
            .selectable(true)
            .selection_color(l.selection)
            .key("text");
        code_txt = if st.wrap() { code_txt.w_full().wrap_anywhere() } else { code_txt.single_line() };

        let mut line = row()
            .w_full()
            .min_h(LINE_H)
            .items_start()
            .fill(fill)
            .role("listitem")
            .child(gutter)
            // The pointer's click, not a control: the keyboard selects and comments by the line's row.
            .child(col().grow(1.0).min_w(0.0).px(l.adv).child(code_txt).on_background_press_tx({
                let get_mut = self.v.get_mut.clone();
                let notes = self.v.notes.clone();
                move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    let st = get_mut(s);
                    let before = st.selection().map(file_range);
                    st.click_code(n);
                    let after = st.selection().map(file_range);
                    let path = st.path().to_owned();
                    if let Some(notes) = &notes {
                        if before != after {
                            notes.handoff(s, &path, Handoff::Selected(after), tx);
                        }
                        notes.handoff(s, &path, Handoff::Ended(after), tx);
                    }
                }
            }));
        if let Some(notes) = self.v.notes.as_ref().filter(|_| !quiet) {
            let notes = notes.clone();
            let path = st.path().to_owned();
            line = line.hover_group().child(super::notes::plus_button(&self.pal, l.number_w(st.digits()), opencode_i18n::t!("ui.lineComment.submit"), move |s: &mut S, tx: &mut Tx| {
                notes.plus(s, &path, None, n, tx);
            }));
        }
        // A selected line keeps its fill under the pointer, as session-ui sets it outright; and
        // a line redrawn under a still pointer has lost its hover until it moves.
        if selected || quiet {
            return line;
        }
        line.hover(l.hover).hover_motion(INSTANT, INSTANT)
    }
}

/// A file view's selected lines as a comment's (a file's lines have no side).
fn file_range(sel: FileLineSelection) -> crate::line_comments::Range {
    crate::line_comments::Range { start: sel.start, end: sel.end, side: None, end_side: None }
}
