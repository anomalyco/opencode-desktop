//! Virtualized single-file viewer [`CodeViewer`], sharing [`CodeLook`] and syntax highlighting with [`DiffViewer`](super::DiffViewer).

use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use rune::{
    Anim, Color, Cursor, Drag, El, GrowthSpec, Phase, ScrollSpec, Tx, col, memo, rich, row, text,
};

use super::file_state::CodeDocState;
use super::{CODE_PAD, CodeLook, EDGE_FADE, LINE_H, expand_tabs, layer_spans};
use crate::palette::Pal;

const INSTANT: Anim = Anim::INSTANT;

type Get<S> = Rc<dyn Fn(&S) -> &CodeDocState>;
type GetMut<S> = Rc<dyn Fn(&mut S) -> &mut CodeDocState>;

pub struct CodeViewer<S> {
    key: String,
    get: Get<S>,
    get_mut: GetMut<S>,
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
        }
    }

    pub fn build(self, s: &S, p: &Pal) -> El {
        let st = (self.get)(s);
        let look = CodeLook::new(p);
        let root_key = self.key.clone();
        let scroll_key = format!("{root_key}-scroll");

        if !st.ready {
            return col()
                .key(root_key)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .fill(look.bg)
                .center()
                .child(text("Loading file…").font(look.label).color(look.muted));
        }
        if st.missing {
            let name = st.path().rsplit('/').next().unwrap_or(st.path());
            return col()
                .key(root_key)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .fill(look.bg)
                .center()
                .child(text(format!("File not found: {name}")).font(look.label).color(look.muted));
        }
        if st.binary {
            return col()
                .key(root_key)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .fill(look.bg)
                .center()
                .child(
                    text(format!("Binary file · {} bytes", st.size_bytes))
                        .font(look.label)
                        .color(look.muted),
                );
        }

        let wrap = st.wrap();
        if !wrap && let Some(r) = rune::memo::layout(&scroll_key) {
            st.set_viewport(r);
        }

        let count = st.line_count();
        let inner = Rc::new(Inner { look, v: self });
        let mut list = col()
            .key("rows")
            .w_full()
            .py(4.0)
            .growth(GrowthSpec::default())
            .selection_group(true)
            .role("list")
            .label("File contents");
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
        let scroll = if wrap { scroll } else { scroll.scroll_xy().scroll_edge_fade(EDGE_FADE) }.child(list);

        col()
            .key(root_key)
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .fill(inner.look.bg)
            .child(scroll)
    }
}

struct Inner<S> {
    look: CodeLook,
    v: CodeViewer<S>,
}

impl<S: 'static> Inner<S> {
    fn content_width(&self, st: &CodeDocState) -> f32 {
        self.look.number_w(st.digits())
            + st.columns() as f32 * self.look.adv
            + CODE_PAD.0
            + CODE_PAD.1
    }

    fn row(&self, s: &S, i: usize) -> El {
        let st = (self.v.get)(s);
        let n = (i + 1) as u32;
        let selected = st.selection().is_some_and(|sel| sel.contains(n));
        let has_matches = !st.line_matches(n).is_empty();
        let key = format!("cl{n}");
        memo(
            key.as_str(),
            (self.look.version, st.rev, selected, has_matches),
            || self.line_row(st, n, selected),
        )
    }

    fn line_row(&self, st: &CodeDocState, n: u32, selected: bool) -> El {
        let l = &self.look;
        let fill = if selected { l.selected } else { l.bg };
        let gutter_fill = if selected { l.selected_gutter } else { l.bg };
        let num_color = if selected { l.text } else { l.faint };
        let get_mut = self.v.get_mut.clone();
        let mut gutter = row()
            .key("gutter")
            .w(l.number_w(st.digits()))
            .min_h(LINE_H)
            .self_stretch()
            .px(8.0)
            .justify_end()
            .items_start()
            .fill(gutter_fill)
            .cursor(Cursor::Pointer)
            .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
                tx.instant();
                let st = get_mut(s);
                match d.phase {
                    Phase::Down => st.press_line(n),
                    Phase::Move => st.drag_to_offset(n, d.y),
                    Phase::Up => {
                        if d.cancelled() {
                            st.cancel_line();
                        } else {
                            st.release_line();
                        }
                    }
                }
            })
            .child(text(n.to_string()).font(l.mono).color(num_color));
        if !st.wrap() {
            gutter = gutter.sticky_left(0.0);
        }

        let raw = st.line(n).unwrap_or("");
        let syntax: Vec<(Range<usize>, Color)> = st
            .highlights(n)
            .into_iter()
            .map(|(r, p)| (r, l.syntax.paint(p)))
            .collect();
        let match_bgs: Vec<(Range<usize>, Color)> = st
            .line_matches(n)
            .iter()
            .map(|(r, active)| {
                (r.clone(), if *active { l.active_match_bg } else { l.match_bg })
            })
            .collect();
        let spans = layer_spans(raw.len(), &syntax, &match_bgs);
        let (shown, spans) = expand_tabs(raw, spans);
        let mut code_txt = rich(shown, spans)
            .font(l.mono)
            .color(l.text)
            .selectable(true)
            .selection_color(l.selection)
            .key("text");
        code_txt = if st.wrap() { code_txt.w_full() } else { code_txt.single_line() };

        row()
            .w_full()
            .min_h(LINE_H)
            .items_start()
            .fill(fill)
            .hover(l.hover)
            .hover_motion(INSTANT, INSTANT)
            .role("listitem")
            .child(gutter)
            .child(col().grow(1.0).min_w(0.0).pl(CODE_PAD.0).pr(CODE_PAD.1).child(code_txt))
    }
}
