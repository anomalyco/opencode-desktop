//! Desktop's radio item (`radio-v2`), drawn by the Luau `ui.radio` node with the app's own
//! rune-kit colours.

use rune::{Color, El, Shadow, TextStyle, col, el, row, text};

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
/// Desktop's `RadioItem`: its label's line box is `label_lh` tall (20, or 16 where a list row's
/// label sets its own), the description 6 under it.
pub(crate) fn radio(k: &rune_kit::Kit, label: &str, description: &str, checked: bool, disabled: bool, tone: Option<Color>, label_lh: f32) -> El {
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
    // The 16-point text sits centred in its line box.
    let lead = ((label_lh - 16.0) / 2.0).max(0.0);
    let mut words = col().key("text").gap(6.0 + lead).grow(1.0).shrink().min_w(0.0).child(text(label.to_owned()).key("label").font(TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04)).color(ink).mt(lead));
    if !description.is_empty() {
        let ink = if disabled { c.text_faint } else { tone.unwrap_or(c.text_muted) };
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
