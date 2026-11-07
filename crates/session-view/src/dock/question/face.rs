//! The question's face of the composer surface: header, answers and footer.

use rune::{Anim, Color, El, ScrollSpec, SpringPair, TextEdit, TextStyle, Transition, Tx, col, el, row, spacer, text, text_input};

use super::*;
use crate::dock::{Variant, button, face_transition};
use crate::odometer::odometer;
use crate::palette::Pal;
use crate::style::readable;

const TITLE: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
/// An answer's description: a little tighter than the question, so long answers stay compact.
const DESC: TextStyle = TextStyle::new(13.0, 440.0).lh(18.0);
const STRONG: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0);
const HINT: TextStyle = TextStyle::new(11.0, 440.0).lh(12.0);
const MARK: TextStyle = TextStyle::new(11.0, 530.0).lh(12.0).features("\"tnum\"");
const CUSTOM: &str = "Type your own answer…";

/// Whether the caret is on the typed answer's first line as it wraps at `width`: the text up to
/// the end of the caret's word fits on one line.
pub(super) fn on_first_line(t: &TextEdit, width: f32) -> bool {
    let caret = t.focus.min(t.text.len());
    let word_end = t.text[caret..].find(char::is_whitespace).map_or(t.text.len(), |i| caret + i);
    let head = &t.text[..word_end];
    if head.contains('\n') {
        return false;
    }
    width <= 0.0 || rune::text::measure(head, &[], TITLE, Some(width)).1 <= LINE * 1.5
}

/// A new question's text and answers slide in from the side they come from (`arrive`) over the
/// old ones fading out where they stood.
fn arrival(k: &rune_knobs::Knobs, step: i32) -> Transition {
    let slide = k.get(&knobs::ARRIVE_SLIDE) * step as f32;
    Transition::asymmetric(Transition::offset(slide, 0.0).combined(Transition::opacity()), Transition::opacity())
        .springs(SpringPair { enter: knobs::spring(k, &knobs::ARRIVE), exit: knobs::spring(k, &knobs::FADE_OUT) })
}

/// Question `tab`'s text and answers in their own viewport, so a new question starts at its top.
fn scroller(tab: usize, page: El) -> El {
    col().key(format!("q-scroll-{tab}")).w_full().shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(FADE).scroll_margin(FADE).child(page)
}

/// An answer's key and mark: round for one answer, square for many, filled once picked.
fn mark(p: &Pal, multi: bool, picked: bool, index: usize) -> El {
    let key = el().size(18.0, 18.0).center().radius(if multi { 4.0 } else { 9.0 });
    let key = match (picked, multi) {
        (true, _) => key.fill(p.bg_accent),
        (false, false) => key.fill(p.layer3).box_shadow(Pal::ring_in(p.border)),
        (false, true) => key.border(1.0, p.q.border_strong),
    };
    let ink = if picked { p.q.contrast_text } else { p.muted };
    let key = key.children((index < 9).then(|| text((index + 1).to_string()).font(MARK).color(ink)));
    el().key("mark").size(18.0, 20.0).center().child(key)
}

/// The question's face of the composer surface: header, answers and footer.
pub fn face<S: DockHost>(s: &S, p: &Pal) -> Option<Vec<El>> {
    let f = asking(s)?;
    let (tab, focus) = place(s, &f);
    let field = &f.fields[tab];
    let multi = f.multi(tab);
    let k = f.key(tab);
    let editing = editing(s, &f, tab);
    let list = rows(field);
    let focus = if editing { list.len() } else { focus };
    let picks = s.dock().picks.get(&k).cloned().unwrap_or_default();
    let total = f.fields.len();
    let fade = || face_transition(s.knobs());

    let title = if total == 1 { "Question".to_owned() } else { format!("{} of {total} questions", tab + 1) };
    let progress = (total > 1).then(|| {
        row().key("q-progress").gap(2.0).children((0..total).map(|i| {
            let color = if i == tab { p.icon_muted } else if answered(s, &f, i) { p.bg_accent } else { p.q.layer4 };
            el().key(format!("q-seg-{i}"))
                .role("button")
                .label(format!("Question {}", i + 1))
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .hover(p.hover)
                .hover_motion(Anim::INSTANT, Anim::INSTANT)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| if let Some(f) = asking(s) { jump(s, &f, i, tx) })
                .child(el().key("bar").size(14.0, 2.0).fill(color))
        }))
    });
    let text_of = if field.description.is_empty() { &field.title } else { &field.description };
    let header = col()
        .key("q-header")
        .w_full()
        .px(10.0)
        .child(row().key("q-top").w_full().min_h(28.0).gap(12.0).center_y().child(odometer(format!("q-title-{}", f.id), title, (tab + 1) as f64, TITLE, p.muted)).child(spacer()).children(progress));
    let details = col()
        .key(format!("q-details-{tab}"))
        .w_full()
        .px(10.0)
        .gap(4.0)
        .child(readable(p, text(text_of.clone()).key("q-text").font(STRONG).color(p.text).wrap()))
        .children(multi.then(|| text("Pick any").key("q-hint").font(HINT).color(p.faint)));

    let mut answers = col().key(format!("q-options-{tab}")).w_full().gap(2.0).role(if multi { "group" } else { "radiogroup" });
    for (i, (value, label, desc)) in list.iter().enumerate() {
        let picked = picks.contains(value);
        answers = answers.child(
            row()
                .key(format!("q-opt-{i}"))
                .role(if multi { "checkbox" } else { "radio" })
                .label(label.clone())
                .w_full()
                .padding(10.0)
                .gap(8.0)
                .items_start()
                .radius(6.0)
                .fill(if i == focus { p.q.selected } else { Color::TRANSPARENT })
                .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| if inside { hover_answer(s, i, tx) })
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    let Some(f) = asking(s) else { return };
                    tx.instant();
                    let (tab, _) = place(s, &f);
                    select(s, &f, tab, i);
                    dock(s, &f).focus = i;
                })
                .child(mark(p, multi, picked, i))
                .child(
                    col()
                        .key("main")
                        .grow(1.0)
                        .gap(2.0)
                        .child(text(label.clone()).key("label").font(STRONG).color(p.text).wrap())
                        .children((!desc.is_empty()).then(|| text(desc.clone()).key("description").font(DESC).color(p.muted).wrap())),
                ),
        );
    }
    if has_custom(field) {
        let i = list.len();
        let on = s.dock().custom_on(&k);
        let edit = s.dock().typed.get(&k).cloned().unwrap_or_else(|| TextEdit::new(""));
        let (id, field_key) = (f.id.clone(), field.key.clone());
        // The typed answer is written where an answer's title is, in the same row: no box of its
        // own, wrapping inside the row as it grows.
        let input = text_input(&edit, if editing { TITLE } else { STRONG })
            .key("q-input")
            .label(CUSTOM)
            .grow(1.0)
            .min_w(0.0)
            .color(p.text)
            .placeholder(CUSTOM, if editing { p.faint } else { p.text })
            .caret(p.text, p.selection)
            .on_layout(|s: &mut S, r| {
                let d = s.dock_mut();
                if (d.input_w - r[2]).abs() > 0.5 {
                    d.input_w = r[2];
                }
            })
            .on_edit(move |s: &mut S| s.dock_mut().typed_mut(&id, &field_key))
            .on_input_change(|s: &mut S, _| {
                let Some(f) = asking(s) else { return };
                let (tab, _) = place(s, &f);
                let k = f.key(tab);
                let d = s.dock_mut();
                d.custom.insert(k.clone());
                if !f.multi(tab) {
                    d.picks.remove(&k);
                }
            })
            .on_input_key(|s: &mut S, ev, tx| typed_key(s, ev, tx));
        answers = answers.child(
            row()
                .key("q-custom")
                .w_full()
                .padding(10.0)
                .gap(8.0)
                .items_start()
                .radius(6.0)
                .fill(if i == focus { p.q.selected } else { Color::TRANSPARENT })
                .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| if inside { hover_answer(s, i, tx) })
                .on_press_tx(|s: &mut S, tx: &mut Tx| {
                    let Some(f) = asking(s) else { return };
                    tx.instant();
                    let (tab, _) = place(s, &f);
                    start_typing(s, &f, tab);
                })
                .child(mark(p, multi, on, i).role(if multi { "checkbox" } else { "radio" }).label(CUSTOM).on_press_tx(|s: &mut S, tx: &mut Tx| {
                    let Some(f) = asking(s) else { return };
                    tx.instant();
                    let (tab, _) = place(s, &f);
                    toggle_custom(s, &f, tab);
                }))
                .child(input),
        );
    }

    // The question and its answers scroll together under the header, sliding in as one.
    let page = scroller(tab, col().key("q-page").w_full().gap(16.0).child(details).child(answers)).transition(arrival(s.knobs(), s.dock().step));
    let body = col().key("question").w_full().padding(8.0).gap(4.0).transition(fade()).child(header).child(page);
    let body = match body_max(s) {
        Some(h) => body.max_h(h),
        None => body,
    };

    let last = f.last(tab);
    let footer = row()
        .key("question-footer")
        .w_full()
        .px(12.0)
        .pt(7.0)
        .pb(8.0)
        .center_y()
        .transition(fade())
        .child(button(p, "q-dismiss", "Dismiss", Variant::GhostMuted, None, true).on_press_tx(|s: &mut S, tx: &mut Tx| {
            if let Some(f) = asking(s) {
                dismiss(s, &f, tx)
            }
        }))
        .child(spacer())
        .child(
            row()
                .key("q-actions")
                .gap(8.0)
                .children((tab > 0).then(|| {
                    button(p, "q-back", "Back", Variant::Neutral, None, true).on_press_tx(|s: &mut S, tx: &mut Tx| {
                        let Some(f) = asking(s) else { return };
                        let (tab, _) = place(s, &f);
                        jump(s, &f, tab.saturating_sub(1), tx);
                    })
                }))
                .child({
                    let ready = answered(s, &f, tab);
                    let b = button(p, "q-next", if last { "Submit" } else { "Next" }, if last { Variant::Submit } else { Variant::Neutral }, Some(&crate::keymap::Bind::QuestionAdvance.label()), ready);
                    if ready { b.on_press_tx(|s: &mut S, tx: &mut Tx| if let Some(f) = asking(s) { next(s, &f, tx) }) } else { b }
                }),
        );
    Some(vec![body, footer])
}
