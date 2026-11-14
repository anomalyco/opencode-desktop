//! Desktop's default question dock, Classic (`session-question-dock-classic.tsx`,
//! `question-prompt.css`): a raised body with the question and its answers as bordered cards,
//! each with its mark and its number key, over a footer tray with Dismiss, Back and Next or
//! Submit. The header's chevron folds the dock down to its question.
//!
//! Keys: 1–9 pick, ↑/↓ (←/→, Home/End) move between answers, Space or ↵ picks the one moved to,
//! ⌘↵ (Ctrl+↵) goes on or submits, ⌘[ (Alt+← off a Mac) goes back, Esc dismisses. In "Type your
//! own answer", keys type, ↵ and Esc leave the field.

use std::f32::consts::PI;

use rune::{Anim, Color, Curve, Easing, El, Key, KeyEvent, Shadow, Spring, TextEdit, TextStyle, Tx, col, el, icon, row, spacer, text};
use rune_theme_opencode::icons as ic;

use super::*;
use crate::dock::{BUTTON, Variant, button, face_transition};
use crate::palette::{Elevate as _, Pal};
use crate::style::readable;

/// `question-header-title`: 13 px on the compact line height.
const HEAD: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);
const QUESTION: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
const HINT: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const LABEL: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// `option-description`: 12 px on the tight line height.
const DESC: TextStyle = TextStyle::new(12.0, 440.0).lh(12.0).tracking(0.05);
const INPUT: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);
/// A number key's cap (`keybind-v2` at the shortcut's 10 px, regular).
const KEY: TextStyle = TextStyle::new(10.0, 400.0).lh(11.0).tracking(0.05).features("\"tnum\", \"lnum\"");
fn custom_label() -> &'static str {
    opencode_i18n::t!("ui.question.custom.answerPlaceholder")
}
/// The footer tray's height: 16 pt over and 8 pt under the 32 pt buttons, inside its borders.
const TRAY_H: f32 = 57.0;
/// How far the tray tucks up under the body (`margin-block-start: -9px`).
const TUCK: f32 = 9.0;
/// The gap the dock keeps from the top of the pane (`measure`'s `gap`), and its floor.
const PANE_GAP: f32 = 8.0;
const MIN_H: f32 = 240.0;
/// The fold's spring (`useSpring`, 0.3 s without bounce).
const FOLD: Spring = Spring::new(0.3, 0.0);
const MAC: bool = cfg!(target_os = "macos");

pub fn face<S: DockHost>(s: &S, p: &Pal) -> Option<Vec<El>> {
    let f = asking(s)?;
    let (tab, focus) = place(s, &f);
    let field = &f.fields[tab];
    let multi = f.multi(tab);
    let k = f.key(tab);
    let editing = editing(s, &f, tab);
    let list = rows(field);
    let picks = s.dock().picks.get(&k).cloned().unwrap_or_default();
    let total = f.fields.len();
    let d = s.dock();
    let minimized = d.minimized;
    let ring = d.keyboard && !editing;
    let fade = || face_transition(s.knobs());
    let max = s.dock_pane_h().map(|h| (h - PANE_GAP).max(MIN_H));
    let live = !super::busy(s, &f);

    let title = if total == 1 { opencode_i18n::t!("session.question.title").to_owned() } else { opencode_i18n::t!("session.question.progress", current = tab + 1, total = total) };
    let top = row()
        .key("q-top")
        .w_full()
        .min_h(28.0)
        .gap(12.0)
        .center_y()
        .child(text(title).key("q-title").font(HEAD).color(p.muted).wrap_anywhere().shrink())
        .child(spacer())
        .child(row().key("q-header-actions").gap(8.0).center_y().children((total > 1).then(|| progress(s, p, &f, tab))).child(chevron::<S>(p, minimized)));
    let text_of = if field.description.is_empty() { &field.title } else { &field.description };
    let question = text(text_of.clone()).key("q-text").font(QUESTION).color(p.text).wrap_anywhere();
    let question = readable(p, if minimized { question.max_lines(3) } else { question });
    let hint = if multi { opencode_i18n::t!("ui.question.classic.multiHint") } else { opencode_i18n::t!("ui.question.singleHint") };
    // No bar, as Desktop hides it (`question-prompt.css:77-85`).
    let text_and_hint = col().w_full().gap(8.0).child(question).children((!minimized).then(|| text(hint).key("q-hint").font(HINT).color(p.faint).wrap()));
    let details = crate::scroller::scroller(format!("q-details-{tab}")).hidden(col().w_full().shrink().min_h(0.0), text_and_hint);
    let details = match max {
        Some(m) => details.max_h(((m - 120.0) * 0.45).max(48.0)),
        None => details,
    };
    let header = col().key("q-header").w_full().gap(4.0).child(top).child(details);

    // Folded, the answers close to nothing but keep their place, and the body's gap above them.
    let folded = minimized.then(|| el().key("q-folded").w_full().h(0.0));
    let options = (!minimized).then(|| {
        let mut answers = col().key(format!("q-options-{tab}")).w_full().gap(6.0).role(if multi { "group" } else { "radiogroup" });
        for (i, (value, label, desc)) in list.iter().enumerate() {
            let picked = picks.contains(value);
            let card = card(p, picked, ring && focus == i, d.keyboard)
                .key(format!("q-opt-{i}"))
                .role(if multi { "checkbox" } else { "radio" })
                .label(label.clone())
                .checked(picked)
                // The card draws its own keyboard ring (`ring`), which follows Rune's focus.
                .no_focus_ring()
                .items_start()
                .padding(10.0)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    let Some(f) = asking(s) else { return };
                    let (tab, _) = place(s, &f);
                    select(s, &f, tab, i);
                    let d = dock(s, &f);
                    (d.focus, d.keyboard) = (i, tx.focus_visible());
                })
                .child(mark(p, multi, picked, d.keyboard))
                .child(
                    col()
                        .key("main")
                        .grow(1.0)
                        .shrink()
                        .gap(6.0)
                        .child(text(label.clone()).key("label").font(LABEL).color(p.text).wrap_anywhere())
                        .children((!desc.is_empty()).then(|| text(desc.clone()).key("description").font(DESC).color(p.muted).wrap_anywhere())),
                )
                .children((i < 9).then(|| shortcut(p, i + 1, picked, d.keyboard)));
            answers = answers.child(card);
        }
        if has_custom(field) {
            answers = answers.child(custom(s, p, &f, tab, multi, editing, ring && focus >= list.len()));
        }
        // No bar, as Desktop hides it (`question-prompt.css:186-194`).
        // Room for the picked card's shadow, which the scroll box would clip (`margin: -4px; padding: 4px`).
        crate::scroller::scroller(format!("q-scroll-{tab}")).hidden(col().w_full().shrink().min_h(0.0).mx(-4.0).my(-4.0).transition(fade()), answers.padding(4.0))
    });

    let body = col()
        .key("question")
        .w_full()
        .pt(8.0)
        .px(12.0)
        .pb(12.0)
        .gap(16.0)
        .radius(10.0)
        .fill(p.bg_base)
        .transition(fade())
        .child(header)
        .children(options)
        .children(folded);
    let body = body.elevate(p.raised);
    let body = match max {
        Some(m) => body.max_h((m - (TRAY_H - TUCK)).max(0.0)),
        None => body,
    };

    let last = f.last(tab);
    // Both labels keep their room so the button doesn't change width between Next and Submit.
    let advance_w = [opencode_i18n::t!("ui.common.next"), opencode_i18n::t!("ui.common.submit")].map(|l| rune::text::measure(l, &[], BUTTON, None).0).into_iter().fold(0.0, f32::max) + 30.0;
    let (submit, back) = if MAC { ("⌘⏎".to_owned(), "⌘[".to_owned()) } else { (format!("{}+⏎", opencode_i18n::t!("common.key.ctrl")), format!("{}+←", opencode_i18n::t!("common.key.alt"))) };
    let footer = row()
        .key("question-footer")
        .w_full()
        .mt(-TUCK)
        .pt(16.0)
        .pb(8.0)
        .px(8.0)
        // The tray (`question-footer`): `layer-01` with `border: 0.5px`, which Chromium draws on
        // whole device pixels and lays out as such (`border_layout`: 1 pt each side at 1x, 0.667
        // at 1.5x). It sits under the body's bottom edge (`paint_order`), so the body's shadow
        // falls on it.
        .corners([0.0, 0.0, 10.0, 10.0])
        .fill(p.layer1)
        .border_sides([0.5, 0.5, 0.5, 0.5], p.border)
        .border_layout()
        .paint_order(-1)
        .center_y()
        .transition(fade())
        .child(button(p, "q-dismiss", opencode_i18n::t!("ui.common.dismiss"), Variant::GhostMuted, None, live).help(format!("{} Esc", opencode_i18n::t!("ui.common.dismiss"))).on_press_tx(|s: &mut S, tx: &mut Tx| {
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
                    button(p, "q-back", opencode_i18n::t!("ui.common.back"), Variant::Neutral, None, live).help(format!("{} {back}", opencode_i18n::t!("ui.common.back"))).on_press_tx(|s: &mut S, tx: &mut Tx| {
                        let Some(f) = asking(s) else { return };
                        let (tab, _) = place(s, &f);
                        jump(s, &f, tab.saturating_sub(1), tx);
                    })
                }))
                .child({
                    let label = if last { opencode_i18n::t!("ui.common.submit") } else { opencode_i18n::t!("ui.common.next") };
                    button(p, "q-next", label, if last { Variant::Contrast } else { Variant::Neutral }, None, live)
                        .w(advance_w)
                        .justify_center()
                        .help(format!("{label} {submit}"))
                        .on_press_tx(|s: &mut S, tx: &mut Tx| {
                            if let Some(f) = asking(s) {
                                next(s, &f, tx)
                            }
                        })
                }),
        );
    Some(vec![body, footer])
}

/// How the cards' feedback eases (`question-prompt.css`: `--question-feedback-duration: 120ms`,
/// `ease`), and at once while keys drive the dock (`[data-keyboard="true"]` sets it to 0s).
fn feedback(keyboard: bool) -> Anim {
    if keyboard { Anim::INSTANT } else { Anim::new(Curve::tween(0.12, Easing::EASE)) }
}

/// An answer's card (`question-option`): `bg-base` under a hairline; picked, a raised card on a
/// `layer-01` wash (#62896); under the pointer, `layer-02`; moved to by key, the focus ring. The
/// hover's `background-color` and the wash's opacity (its `::before`) both ease on `feedback`;
/// the hairline goes at once (`box-shadow` isn't transitioned). The hover is the pointer's own
/// track, so moving over the answers commits nothing.
fn card(p: &Pal, picked: bool, ring: bool, keyboard: bool) -> El {
    let ease = feedback(keyboard);
    let wash = p.layer1;
    // Its hairline, elevation and ring change at once (`Spring::NONE`): only the hover and the wash ease.
    let c = row().w_full().gap(8.0).radius(6.0).fill(p.bg_base).motion(Spring::NONE).hover_motion(ease, ease);
    let c = if picked {
        // `--v2-elevation-raised`; CSS paints its first shadow on top, so the last goes first.
        let [far, near, edge_ring, edge] = p.raised;
        c.box_shadow(edge).box_shadow(edge_ring).box_shadow(near).box_shadow(far)
    } else {
        c.hover(p.layer2).box_shadow(Shadow { spread: 0.5, color: p.border, inset: true, ..Shadow::default() })
    };
    // Under a contrast theme every card has a solid edge, so the ring is three times as wide.
    let c = if ring { c.box_shadow(Shadow { spread: if p.forced { 3.0 } else { 1.0 }, color: p.q.focus, inset: true, ..Shadow::default() }) } else { c };
    c.child(el().key("wash").absolute(0.0, 0.0).w_full().h_full().radius(6.0).fill(wash).opacity(if picked { 1.0 } else { 0.0 }).opacity_motion(ease))
}

/// The header's progress: a bar per question, accent once answered, muted on the current one.
fn progress<S: DockHost>(s: &S, p: &Pal, f: &Form, tab: usize) -> El {
    row().key("q-progress").gap(2.0).center_y().children((0..f.fields.len()).map(|i| {
        let color = if i == tab { p.icon_muted } else if answered(s, f, i) { p.bg_accent } else { p.q.layer4 };
        el().key(format!("q-seg-{i}"))
            .role("button")
            .label(opencode_i18n::t!("ui.tool.questions.numbered", number = i + 1))
            .size(20.0, 20.0)
            .radius(4.0)
            .center()
            .hover(p.hover)
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                if let Some(f) = asking(s) {
                    jump(s, &f, i, tx)
                }
            })
            .child(el().key("bar").size(14.0, 2.0).fill(color))
    }))
}

/// The header's chevron (`icon-button-v2`, ghost, large): folds the dock to its question and back,
/// turning over as it does.
fn chevron<S: DockHost>(p: &Pal, minimized: bool) -> El {
    el().key("q-minimize")
        .role("button")
        .label(if minimized { opencode_i18n::t!("session.question.restore") } else { opencode_i18n::t!("session.question.minimize") })
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut S, tx: &mut Tx| {
            let Some(f) = asking(s) else { return };
            tx.motion_curve(FOLD);
            let (tab, _) = place(s, &f);
            stop_typing(s, &f, tab);
            let focus = pick_focus(s, &f, tab);
            let d = dock(s, &f);
            d.minimized = !d.minimized;
            d.focus = focus;
            d.keyboard = false;
        })
        .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_muted).rotate(if minimized { PI } else { 0.0 }).rotate_motion(FOLD))
}

/// An answer's mark (`question-option-check`): a circle for one answer, a square for many,
/// accent with a dot or a tick once picked.
fn mark(p: &Pal, multi: bool, picked: bool, keyboard: bool) -> El {
    // The box's fill isn't transitioned: it turns accent at once (`Spring::NONE`).
    let b = el().key("box").size(16.0, 16.0).center().radius(if multi { 4.0 } else { 8.0 }).motion(Spring::NONE);
    let b = if picked {
        b.fill(p.bg_accent).gradient(Color::WHITE.alpha(0.2), Color::WHITE.alpha(0.0))
    } else {
        b.fill(p.bg_base).gradient(Color::WHITE.alpha(0.06), Color::WHITE.alpha(0.0))
    };
    // A picked circle drops its hairline; a picked square keeps it.
    let b = if picked && !multi { b } else { b.box_shadow(Shadow { spread: 0.5, color: p.q.border_strong, inset: true, ..Shadow::default() }) };
    // The dot and the tick are always there, shown once picked, as Desktop fades them (`opacity`
    // on `feedback`); the box's own fill changes at once.
    let inner = if multi {
        icon(&ic::CHECK, 16.0).key("check").color(p.icon_contrast)
    } else {
        el().key("dot").size(6.0, 6.0).radius(3.0).fill(p.q.dot).box_shadow(Shadow { y: 0.5, blur: 0.5, color: Color::BLACK.alpha(0.4), ..Shadow::default() })
    };
    el().key("mark").size(16.0, 20.0).center().child(b.child(inner.opacity(if picked { 1.0 } else { 0.0 }).opacity_motion(feedback(keyboard))))
}

/// An answer's number key (`keybind-v2`): filled, or outlined once picked. Its `background-color`
/// and `box-shadow` ease on `feedback` (`--keybind-transition-duration`, `keybind.css`): the fill
/// and the outline are two layers that cross-fade.
fn shortcut(p: &Pal, n: usize, picked: bool, keyboard: bool) -> El {
    let ease = feedback(keyboard);
    let layer = |key: &'static str, on: bool| el().key(key).absolute(0.0, 0.0).w_full().h_full().radius(2.0).opacity(if on { 1.0 } else { 0.0 }).opacity_motion(ease);
    let fill = layer("fill", !picked).fill(p.layer3);
    let ring = layer("ring", picked).box_shadow(Shadow { spread: 1.0, color: p.q.border_strong, inset: true, ..Shadow::default() });
    let cap = row().key("cap").h(14.0).min_w(14.0).px(4.0).center().radius(2.0).child(fill).child(ring);
    row().key("shortcut").h(20.0).self_center().center_y().child(cap.child(text(n.to_string()).font(KEY).color(p.muted)))
}

/// "Type your own answer" (`question-option[data-custom]`): its mark, the field in an answer's
/// place, and its number key.
fn custom<S: DockHost>(s: &S, p: &Pal, f: &Form, tab: usize, multi: bool, editing: bool, ring: bool) -> El {
    let field = &f.fields[tab];
    let k = f.key(tab);
    let i = rows(field).len();
    let on = s.dock().custom_on(&k);
    let edit = s.dock().typed.get(&k).cloned().unwrap_or_else(|| TextEdit::new(""));
    let (id, field_key) = (f.id.clone(), field.key.clone());
    let input = crate::style::field(&edit, INPUT)
        .key("q-input")
        .spellcheck(s.composer_spellcheck())
        .label(custom_label())
        .single_line()
        // `dir="auto"` (`session-question-dock-classic.tsx:673`).
        .dir(rune::Direction::Auto)
        .grow(1.0)
        .min_w(0.0)
        .color(p.text)
        .placeholder(custom_label(), if editing { p.faint } else { p.text })
        .caret(p.text, p.selection).selection_style(p.field_selection)
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
    // Open, the field gets a box of its own with the focus ring.
    let input = if editing {
        let lit = Color::WHITE.alpha(0.02);
        row()
            .key("q-field")
            .grow(1.0)
            .min_w(0.0)
            .h(28.0)
            .px(8.0)
            .center_y()
            .radius(6.0)
            .fill(p.bg_base)
            .gradient(lit, Color::WHITE.alpha(0.0))
            .box_shadow(Shadow { spread: 1.0, color: p.q.focus, inset: true, ..Shadow::default() })
            .child(input)
    } else {
        row().key("q-field").grow(1.0).min_w(0.0).h(28.0).center_y().child(input)
    };
    let keyboard = s.dock().keyboard;
    card(p, on, ring, keyboard)
        .key("q-custom")
        .role(if multi { "checkbox" } else { "radio" })
        .label(custom_label())
        .checked(on)
        .no_focus_ring()
        .h(40.0)
        .px(10.0)
        .center_y()
        .on_press_tx(|s: &mut S, _: &mut Tx| {
            let Some(f) = asking(s) else { return };
            let (tab, _) = place(s, &f);
            start_typing(s, &f, tab);
            dock(s, &f).keyboard = false;
        })
        .child(mark(p, multi, on, keyboard).role(if multi { "checkbox" } else { "radio" }).label(custom_label()).on_press_tx(|s: &mut S, _: &mut Tx| {
            let Some(f) = asking(s) else { return };
            let (tab, _) = place(s, &f);
            toggle_custom(s, &f, tab);
            dock(s, &f).keyboard = false;
        }))
        .child(input)
        .children((i < 9).then(|| shortcut(p, i + 1, on, keyboard)))
}

/// Back: ⌘[ on a Mac, Alt+← elsewhere.
fn back(ev: &KeyEvent) -> bool {
    let m = ev.mods;
    if MAC {
        return matches!(ev.key, Key::Command('[')) && !m.ctrl && !m.alt;
    }
    ev.key == Key::Left && m.alt && !m.ctrl && !m.cmd
}

fn advance(ev: &KeyEvent) -> bool {
    ev.key == Key::Enter && (ev.mods.cmd || ev.mods.ctrl) && !ev.mods.alt
}

/// A key while the Classic dock shows (its `nav`); false when it isn't the dock's.
pub fn key<S: DockHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(f) = asking(s) else { return false };
    let (tab, focus) = place(s, &f);
    let field = &f.fields[tab];
    let n = rows(field).len();
    let count = count(field);
    let minimized = s.dock().minimized;
    dock(s, &f).keyboard = true;
    // A reply on its way: Esc, Back and the advance key do nothing, and the rest aren't the dock's.
    if super::busy(s, &f) {
        return ev.key == Key::Escape || back(ev) || advance(ev);
    }
    if ev.key == Key::Escape {
        dismiss(s, &f, tx);
        return true;
    }
    if back(ev) {
        if tab > 0 {
            jump(s, &f, tab - 1, tx);
        }
        return true;
    }
    if advance(ev) {
        next(s, &f, tx);
        return true;
    }
    let m = ev.mods;
    if editing(s, &f, tab) || m.alt || m.ctrl || m.cmd || matches!(ev.key, Key::Command(_)) {
        return false;
    }
    let digit = match &ev.key {
        Key::Text(t) if t.chars().count() == 1 => t.chars().next().and_then(|c| c.to_digit(10)).filter(|d| *d >= 1).map(|d| d as usize - 1),
        _ => None,
    };
    if let Some(i) = digit {
        if minimized || i >= count {
            return false;
        }
        // A pick's mark fades in, and opening the typed answer's field moves its words in by the
        // field's padding, on the springs rather than at once.
        if i == n {
            toggle_custom(s, &f, tab);
        } else {
            select(s, &f, tab, i);
            dock(s, &f).focus = i;
        }
        reveal(s, tx);
        return true;
    }
    // The rest move and pick among the answers, which a folded dock hides.
    if minimized {
        return false;
    }
    let rtl = opencode_i18n::direction() == opencode_i18n::Direction::Rtl;
    let to = match &ev.key {
        Key::Left if rtl => Some((focus + 1).min(count - 1)),
        Key::Right if rtl => Some(focus.saturating_sub(1)),
        Key::Down | Key::Right => Some((focus + 1).min(count - 1)),
        Key::Up | Key::Left => Some(focus.saturating_sub(1)),
        Key::Home => Some(0),
        Key::End => Some(count - 1),
        _ => None,
    };
    if let Some(to) = to {
        tx.instant();
        dock(s, &f).focus = to;
        reveal(s, tx);
        return true;
    }
    if matches!(&ev.key, Key::Text(t) if t == " ") || (ev.key == Key::Enter && !m.shift) {
        if focus >= n { toggle_custom(s, &f, tab) } else { select(s, &f, tab, focus) }
        return true;
    }
    false
}

/// Keys in the open "Type your own answer": ↵ and Esc leave the field for its card, ⌘↵ and Back
/// as anywhere; everything else types.
fn typed_key<S: DockHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(f) = asking(s) else { return false };
    let (tab, _) = place(s, &f);
    let n = rows(&f.fields[tab]).len();
    let m = ev.mods;
    if ev.key == Key::Escape || (ev.key == Key::Enter && !m.cmd && !m.ctrl && !m.alt) {
        stop_typing(s, &f, tab);
        let d = dock(s, &f);
        (d.focus, d.keyboard) = (n, true);
        return true;
    }
    if advance(ev) || back(ev) {
        stop_typing(s, &f, tab);
        return key(s, ev, tx);
    }
    s.dock_shortcut(ev, tx)
}

/// Brings the highlighted answer into sight in the answers' scroller.
fn reveal<S: DockHost>(s: &S, tx: &mut Tx) {
    let Some(f) = asking(s) else { return };
    let (tab, focus) = place(s, &f);
    let field = &f.fields[tab];
    let key = if focus >= rows(field).len() && has_custom(field) { "q-custom".to_owned() } else { format!("q-opt-{focus}") };
    // Keyboard focus goes with the dock's own, so a reader says the answer moved to.
    tx.focus(key.clone(), true);
    tx.scroll_into_view_in(format!("q-scroll-{tab}"), key);
}
