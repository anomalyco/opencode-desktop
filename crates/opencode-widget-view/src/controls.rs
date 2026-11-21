//! Controls: button, checkbox, slider, select and input, each sending the contract's event.
//! A select draws inline (chips, or a list when its options carry descriptions) rather than as
//! a popup: no overlay to mount above a scrolling timeline, and a finger reaches every option.

use opencode_widget::{ButtonVariant, SelectOption, SelectValue, WidgetEvent};
use rune::{Drag, El, Phase, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{Button, Checkbox, Kit, RadioItem, TextArea, TextInput};
use serde_json::Value;

use crate::Cx;

/// A field's label over it.
fn label(k: &Kit, s: &str) -> El {
    text(s.to_owned()).key("label").font(k.ty.small_medium.lh(16.0)).color(k.c.text_base).wrap()
}

pub(crate) fn button<S: 'static>(cx: &Cx<S>, id: &str, label: &str, variant: ButtonVariant, disabled: bool) -> El {
    let v = match variant {
        ButtonVariant::Primary => rune_kit::ButtonVariant::Contrast,
        ButtonVariant::Danger => rune_kit::ButtonVariant::Danger,
        ButtonVariant::Secondary => rune_kit::ButtonVariant::Neutral,
    };
    let mut b = Button::new(label).variant(v).disabled(disabled || !cx.live);
    if let Some(hooks) = cx.act().filter(|_| !disabled) {
        let id = id.to_owned();
        b = b.on_press_tx(move |s: &mut S, tx: &mut Tx| hooks.send(s, WidgetEvent::press(id.clone()), tx));
    }
    b.build(cx.look.kit)
}

pub(crate) fn checkbox<S: 'static>(cx: &Cx<S>, id: &str, label: &str, checked: bool, disabled: bool) -> El {
    let mut c = Checkbox::new(checked).label(label).disabled(disabled || !cx.live);
    if let Some(hooks) = cx.act().filter(|_| !disabled) {
        let id = id.to_owned();
        c = c.on_toggle_tx(move |s: &mut S, tx: &mut Tx| hooks.send(s, WidgetEvent::change(id.clone(), !checked), tx));
    }
    c.build(cx.look.kit)
}

/// The step a slider moves by: the widget template's, else a hundredth of its range (whole numbers for a
/// range of whole numbers up to 100 wide).
fn slider_step(min: f64, max: f64, step: Option<f64>) -> f64 {
    match step.filter(|s| s.is_finite() && *s > 0.0) {
        Some(s) => s,
        None if min.fract() == 0.0 && max.fract() == 0.0 && max - min <= 100.0 => 1.0,
        None => ((max - min) / 100.0).max(f64::EPSILON),
    }
}

fn snap(v: f64, min: f64, max: f64, step: f64) -> f64 {
    let v = min + ((v - min) / step).round() * step;
    // Clear float noise (0.30000000000000004) to the step's decimals.
    let decimals = decimals(step);
    let scale = 10f64.powi(decimals as i32);
    ((v * scale).round() / scale).clamp(min.min(max), max.max(min))
}

fn decimals(step: f64) -> usize {
    let s = format!("{step}");
    s.split_once('.').map_or(0, |(_, d)| d.len().min(6))
}

fn number(v: f64, step: f64) -> String {
    format!("{v:.*}", decimals(step))
}

/// A slider: a rail filled up to its knob. Dragging moves it live and sends `change` once, on
/// release; arrow keys step it and send each step. A finger drags it from where it lands.
#[allow(clippy::too_many_arguments)]
pub(crate) fn slider<S: 'static>(cx: &Cx<S>, id: &str, title: Option<&str>, value: f64, min: f64, max: f64, step: Option<f64>) -> El {
    let k = cx.look.kit;
    let c = &k.c;
    let (lo, hi) = if max >= min { (min, max) } else { (max, min) };
    let step = slider_step(lo, hi, step);
    let shown = cx.local.dragging(id).unwrap_or(value).clamp(lo, hi);
    let frac = if hi > lo { ((shown - lo) / (hi - lo)) as f32 } else { 0.0 };
    let live = cx.live;
    let (fill, knob) = if live { (c.bg_accent, c.knob) } else { (c.border_strong, c.surface_disabled) };
    let rail = |key: &'static str, grow: f32, color| el().key(key).w(0.0).grow(grow).h(4.0).radius(2.0).fill(color);
    let mut handle = el().key("knob").size(16.0, 16.0).radius(8.0).fill(knob).border(0.5, c.border_base);
    if live {
        for sh in &k.elevation.knob {
            handle = handle.box_shadow(*sh);
        }
    }
    let mut track = row()
        .key("track")
        .w_full()
        .h(20.0)
        .items_center()
        .child(rail("fill", frac, fill))
        .child(handle)
        .child(rail("rest", 1.0 - frac, c.border_weak))
        .label(title.unwrap_or(id).to_owned())
        .range_value(shown, lo, hi)
        .range_step(step)
        .a11y_value(number(shown, step))
        .disabled(!live);
    if let Some(hooks) = cx.act() {
        let (steps, key) = (hooks.clone(), id.to_owned());
        track = track
            .cursor(rune::Cursor::Pointer)
            .touch_drags_at_once()
            .focus_ring(2.0, 2.0, c.border_focus)
            .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
                let at = if d.w > 0.0 { (d.x / d.w).clamp(0.0, 1.0) as f64 } else { 0.0 };
                let v = snap(lo + at * (hi - lo), lo, hi, step);
                match d.phase {
                    Phase::Down | Phase::Move => (hooks.local)(s).drag_to(&key, v),
                    Phase::Up => {
                        if let Some((id, v)) = (hooks.local)(s).drag_end(d.cancelled())
                            && v != value
                        {
                            hooks.send(s, WidgetEvent::change(id, v), tx);
                        }
                    }
                }
            });
        let key = id.to_owned();
        track = track.on_step(move |s: &mut S, delta: i32, tx: &mut Tx| {
            let v = snap(value + f64::from(delta) * step, lo, hi, step);
            if v != value {
                steps.send(s, WidgetEvent::change(key.clone(), v), tx);
            }
        });
    }
    let head = row()
        .key("head")
        .w_full()
        .justify_between()
        .gap(8.0)
        .children(title.map(|t| label(k, t).grow(1.0).min_w(0.0)))
        .child(text(number(shown, step)).key("value").font(k.ty.small.lh(16.0)).color(c.text_muted).a11y_hidden());
    col().w_full().gap(6.0).child(head).child(track)
}

fn select_event(id: &str, options: &[SelectOption], value: Option<&SelectValue>, multiple: bool, toggled: &str) -> WidgetEvent {
    if !multiple {
        return WidgetEvent::change(id, toggled);
    }
    let on = value.is_some_and(|v| v.has(toggled));
    let picked: Vec<Value> = options
        .iter()
        .filter(|o| if o.value == toggled { !on } else { value.is_some_and(|v| v.has(&o.value)) })
        .map(|o| Value::String(o.value.clone()))
        .collect();
    WidgetEvent::change(id, Value::Array(picked))
}

/// A select: chips that wrap, or a list of radio rows (checkboxes when `multiple`) when its
/// options carry descriptions.
pub(crate) fn select<S: 'static>(cx: &Cx<S>, id: &str, title: Option<&str>, options: &[SelectOption], value: Option<&SelectValue>, multiple: bool) -> El {
    let k = cx.look.kit;
    let c = &k.c;
    let acts = cx.act();
    let live = acts.is_some();
    let listed = options.iter().any(|o| o.description.is_some());
    let press = |o: &SelectOption| {
        let (hooks, ev) = (acts.clone(), select_event(id, options, value, multiple, &o.value));
        move |s: &mut S, tx: &mut Tx| {
            if let Some(hooks) = &hooks {
                hooks.send(s, ev.clone(), tx);
            }
        }
    };
    let items: Vec<El> = options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let on = value.is_some_and(|v| v.has(&o.value));
            let item = if listed {
                if multiple {
                    let mut b = Checkbox::new(on).label(o.label.clone()).disabled(!live);
                    if let Some(d) = &o.description {
                        b = b.description(d.clone());
                    }
                    if live {
                        b = b.on_toggle_tx(press(o));
                    }
                    b.build(k)
                } else {
                    let mut b = RadioItem::new(o.label.clone(), on).disabled(!live);
                    if let Some(d) = &o.description {
                        b = b.description(d.clone());
                    }
                    if live {
                        b = b.on_select_tx(press(o));
                    }
                    b.build(k)
                }
            } else {
                let mut chip = row()
                    .gap(6.0)
                    .items_center()
                    .h(28.0)
                    .px(12.0)
                    .radius(14.0)
                    .role(if multiple { "checkbox" } else { "radio" })
                    .checked(on)
                    .label(o.label.clone())
                    .disabled(!live)
                    .border(if on { 1.5 } else { 1.0 }, if on { c.border_focus } else { c.border_weak })
                    .fill(if on { c.surface_weak } else { c.surface_raised_base });
                if on && multiple {
                    chip = chip.child(icon(k.glyph.check_small, 12.0).key("check").color(c.icon_base).a11y_hidden());
                }
                chip = chip.child(text(o.label.clone()).key("label").font(k.ty.ui_compact).color(if on { c.text_strong } else { c.text_base }).a11y_hidden());
                if live {
                    chip = k.interactive(chip.focus_ring(2.0, 2.0, c.border_focus)).on_press_tx(press(o));
                }
                chip
            };
            item.key(format!("o{i}"))
        })
        .collect();
    let group = if listed { col().gap(8.0) } else { row().gap(6.0).flex_wrap() };
    let group = group
        .key("options")
        .w_full()
        .role(if multiple { "group" } else { "radiogroup" })
        .label(title.unwrap_or(id).to_owned())
        .children(items);
    col().w_full().gap(6.0).children(title.map(|t| label(k, t))).child(group)
}

fn lens<S: 'static>(local: crate::Lens<S>, id: String) -> impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static {
    move |s: &mut S| local(s).edit_mut(&id)
}

/// A text input: its typing stays here until committed (Return on one line, a 300 ms rest, or
/// leaving the field), then goes as one `change`.
pub(crate) fn input<S: 'static>(cx: &Cx<S>, id: &str, title: Option<&str>, placeholder: Option<&str>, multiline: bool) -> El {
    let k = cx.look.kit;
    let empty = TextEdit::default();
    let edit = cx.local.draft(id).map_or(&empty, |d| &d.edit);
    let acts = cx.act();
    let live = acts.is_some();
    let wire = acts.map(|hooks| {
        let lens = lens(hooks.local.clone(), id.to_owned());
        let (local, key) = (hooks.local.clone(), id.to_owned());
        let changed = move |s: &mut S, tx: &mut Tx| {
            let due = (local)(s).edited(&key, tx.clock());
            tx.wake_at(due);
        };
        (hooks, lens, changed)
    });
    let field = if multiline {
        let mut f = TextArea::new(edit).auto_grow(2, 8).disabled(!live);
        if let Some(p) = placeholder {
            f = f.placeholder(p);
        }
        if let Some(t) = title {
            f = f.label(t);
        }
        if let Some((_, lens, changed)) = wire {
            f = f.on_edit(lens).on_change(changed);
        }
        f.build(k)
    } else {
        let mut f = TextInput::new(edit).disabled(!live);
        if let Some(p) = placeholder {
            f = f.placeholder(p);
        }
        if let Some(t) = title {
            f = f.label(t);
        }
        if let Some((hooks, lens, changed)) = wire {
            let key = id.to_owned();
            f = f.on_edit(lens).on_change(changed).on_submit(move |s: &mut S, tx: &mut Tx| {
                if let Some(ev) = (hooks.local)(s).commit(&key) {
                    hooks.send(s, ev, tx);
                }
            });
        }
        f.build(k)
    };
    col().w_full().gap(6.0).children(title.map(|t| label(k, t))).child(field.w_full())
}
