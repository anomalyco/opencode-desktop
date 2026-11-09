//! Desktop's controls a plugin asks the host to draw (`ElKind::Button`, `Loader`, `Radio`,
//! `Field`): the app's own rune-kit builds, so a plugin's button or field is the app's to the
//! pixel and links no UI code of its own.

use std::rc::Rc;

use opencode_plugin::abi::{BUTTON_CONTRAST, BUTTON_DANGER, BUTTON_GHOST, BUTTON_GHOST_MUTED, BUTTON_OUTLINE, ElKind, ElSpec, flags};
use rune::{Color, El, Shadow, TextStyle, Tx, col, el, row, text};
use rune_kit::{ButtonVariant, Loader};

use super::arena::Build;
use super::instance::{CallbackEnv, Press};
use crate::App;
use crate::palette::kit;

/// The control `spec` asks for; `None` when it isn't one of these kinds.
#[allow(deprecated)]
pub fn build(b: &Build, spec: &ElSpec, label: &str, help: &str) -> Option<El> {
    let k = kit(b.pal);
    let f = spec.flags;
    Some(match spec.kind {
        ElKind::Button => {
            let variant = match spec.value {
                v if v == BUTTON_CONTRAST => ButtonVariant::Contrast,
                v if v == BUTTON_GHOST => ButtonVariant::Ghost,
                v if v == BUTTON_GHOST_MUTED => ButtonVariant::GhostMuted,
                v if v == BUTTON_OUTLINE => ButtonVariant::Outline,
                v if v == BUTTON_DANGER => ButtonVariant::Danger,
                _ => ButtonVariant::Neutral,
            };
            let loading = f & flags::LOADING != 0;
            let small = f & flags::SMALL != 0;
            let size = if small { rune_kit::ButtonSize::Small } else { rune_kit::ButtonSize::Normal };
            let words = label.to_owned();
            let mut button = crate::widgets::button(label, if loading && !small { ButtonVariant::Loading } else { variant }).size(size).disabled(f & flags::DISABLED != 0);
            // SAFETY: the spec's strings are valid for the build call this runs in.
            let lead = unsafe { spec.icon.as_str() };
            if !lead.is_empty() {
                button = button.icon(super::icons::resolve(b.plugin, lead));
            }
            if loading && small {
                // `<Loader class="size-3.5" />` before the label, in the label's color.
                let spinner = Loader::new().size(14.0).build(k);
                button = button.label_with(move |style, ink| row().gap(4.0).center_y().child(spinner).child(text(words).font(style).color(ink)));
            } else if loading {
                let spinner = Loader::new().size(16.0).build(k);
                button = button.label_with(move |_, _| spinner);
            }
            button.build(k)
        }
        ElKind::Loader => Loader::new().size(if spec.font_size > 0.0 { spec.font_size } else { 16.0 }).build(k),
        ElKind::Radio => radio(k, label, help, spec.value > 0.5, f & flags::DISABLED != 0, (spec.text_color[3] > 0.0).then(|| color(spec.text_color))),
        ElKind::Field => field(b, spec, label),
        ElKind::Switch => {
            let switch = crate::settings::switch(b.pal, "switch", spec.value > 0.5);
            if f & flags::DISABLED != 0 { switch.disabled(true).layer_opacity(0.5) } else { switch }
        }
        _ => return None,
    })
}

/// The settings controls built around their children (`ElKind::SettingsRow`, `SettingsList`):
/// the app's own rows and card, so a plugin's settings page is the app's to the pixel.
pub fn compose(b: &Build, spec: &ElSpec) -> Option<super::arena::Compose> {
    // SAFETY: the spec's strings are valid for the build call this runs in.
    let s = |v: opencode_plugin::abi::AbiStr| unsafe { v.as_str() }.to_owned();
    let pal = *b.pal;
    let key = s(spec.key);
    match spec.kind {
        ElKind::SettingsRow => {
            let (title, description) = (s(spec.text), s(spec.help));
            let lit = crate::settings::search::reveal_of(b.app, &key);
            Some(Box::new(move |mut children: Vec<El>| {
                let control = if children.is_empty() { el() } else { children.remove(0) };
                let row = crate::settings::setting(&pal, &key, &title, &description, control);
                crate::settings::pages::revealed(&pal, lit, 20.0, row)
            }))
        }
        ElKind::SettingsList => Some(Box::new(move |rows: Vec<El>| crate::settings::list(&pal, &key, rows))),
        _ => None,
    }
}

fn color(c: [f32; 4]) -> Color {
    Color { r: c[0], g: c[1], b: c[2], a: c[3] }
}

/// `top` over `bottom`, as CSS layers paint.
fn over(top: Color, bottom: Color) -> Color {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let mix = |t: f32, b: f32| (t * top.a + b * bottom.a * (1.0 - top.a)) / a;
    Color { r: mix(top.r, bottom.r), g: mix(top.g, bottom.g), b: mix(top.b, bottom.b), a }
}

/// A `radio-v2` item (`radio.css`): the 16 pt control in its 16 × 20 stack, the 13/440 label on
/// a 20 line and the 12/440 description under it, 8 apart; the description in `tone` when given.
pub(crate) fn radio(k: &rune_kit::Kit, label: &str, description: &str, checked: bool, disabled: bool, tone: Option<Color>) -> El {
    let c = &k.c;
    let (base, sheen) = if checked { (c.bg_accent, k.light_on(0.2, c.bg_accent)) } else { (c.bg_base, k.light(0.06)) };
    let mut control = el()
        .key("control")
        .size(16.0, 16.0)
        .absolute(0.0, 2.0)
        .radius(8.0)
        .fill(base)
        .gradient(sheen, k.light(0.0))
        .box_shadow(Shadow { spread: 0.5, color: c.border_strong, inset: true, ..Shadow::default() });
    if checked {
        control = control.child(
            el().key("dot")
                .size(6.0, 6.0)
                .absolute(5.0, 5.0)
                .radius(3.0)
                .fill(c.knob_on)
                .box_shadow(Shadow { spread: 0.5, color: k.on(c.depth_top, c.knob_on), inset: true, ..Shadow::default() })
                .box_shadow(Shadow { y: 0.5, blur: 0.5, color: k.dark_a(0.4), ..Shadow::default() }),
        );
        if !disabled {
            control = control.hover(over(k.on(c.contrast_hover, c.bg_accent), c.bg_accent));
        }
    } else if !disabled {
        control = control.hover(over(c.hover, c.bg_base));
    }
    if disabled {
        control = control.layer_opacity(0.5);
    }
    let ink = if disabled { c.text_muted } else { c.text_base };
    let mut words = col().key("text").gap(8.0).grow(1.0).shrink().min_w(0.0).child(text(label.to_owned()).key("label").font(TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04)).color(ink).mt(2.0));
    if !description.is_empty() {
        let ink = if disabled { c.text_muted } else { tone.unwrap_or(c.text_muted) };
        words = words.child(text(description.to_owned()).key("description").font(TextStyle::new(12.0, 440.0).lh(12.0).tracking(0.05).features("\"tnum\"")).color(ink));
    }
    row()
        .role("radio")
        .label(label.to_owned())
        .gap(8.0)
        .items_start()
        .focus_selected(checked)
        .disabled(disabled)
        .child(el().key("stack").size(16.0, 20.0).child(control).a11y_hidden())
        .child(words.a11y_hidden())
}

/// Desktop's `TextInput` over the plugin's editor; Enter runs its press.
fn field(b: &Build, spec: &ElSpec, placeholder: &str) -> El {
    let f = spec.flags;
    let Some(edit) = b.editors.get(spec.editor) else { return el() };
    let editor = spec.editor;
    let mut input = rune_kit::TextInput::new(edit).placeholder(placeholder.to_owned()).invalid(f & flags::INVALID != 0).disabled(f & flags::DISABLED != 0).on_edit(move |app: &mut App| app.plugins.editors.get_mut(editor));
    if f & flags::LARGE != 0 {
        input = input.large();
    }
    if f & flags::SECURE != 0 {
        input = input.secure();
    }
    if !spec.width.is_nan() && spec.width > 0.0 {
        input = input.width(Some(spec.width));
    }
    // The field owns its press (`arena::handlers` leaves it): Enter runs it, a click doesn't.
    if spec.press.func.is_some() {
        let press = Rc::new(Press { slot: b.slot, generation: b.generation, env: CallbackEnv::new(spec.press, b.lib.clone()) });
        input = input.on_submit(move |app: &mut App, tx: &mut Tx| super::run::press(app, tx, &press));
    }
    input.build(kit(b.pal))
}
