//! A titlebar item's pill (Desktop's `TitlebarItemButton`, `titlebar-items.tsx`,
//! `titlebar-items.css`): a round blue-tinted pill over `bg-deep` holding its icon, which grows on
//! hover or focus to show its label (11/16 at 530). In the titlebar it is 20 across with the
//! label before the icon; in the sidebar's footer (`vertical`) 28 across with the label after it.
//!
//! It springs open and shut (Desktop snaps), its label sliding out from beside the icon as the
//! pill widens and fading before the closing edge reaches it. Each hover drops a fresh arrow in
//! from above as the last one falls out the bottom. Reduce Motion lands all of it at once.

use opencode_ui::focus::FocusExt;
use rune::{Curve, Easing, El, MotionIntent, Spring, SpringPair, Transition, Tx, el, row, text};
use rune_knobs::{Group, Knob};

use crate::App;
use crate::palette::Pal;

/// Desktop's default titlebar item icon: a download arrow (`titlebar-items.tsx`).
pub static DOWNLOAD: rune::Icon = rune::Icon { name: "titlebar-download", view: 14.0, body: r#"<path d="M7 11V3M3.5 7.63128L7 11L10.5 7.63128" stroke="currentColor" fill="none"/>"# };

pub const OPEN: Knob = Knob::new("Pill open", 0.12, 0.6, 0.3).step(0.01).about("Spring seconds to grow out");
pub const BOUNCE: Knob = Knob::new("Pill bounce", 0.0, 0.4, 0.12).step(0.01).about("Overshoot as it grows out");
pub const CLOSE: Knob = Knob::new("Pill close", 0.1, 0.5, 0.22).step(0.01).about("Spring seconds to shrink back");
pub const DROP: Knob = Knob::choice("Arrow drop", &["Off", "On"], 1).about("Drop a fresh arrow in on hover")
    .hints(&["The arrow stays still", "The arrow falls out and a new one lands"]);
pub const LAND: Knob = Knob::new("Arrow land", 0.2, 0.8, 0.42).step(0.01).about("Spring seconds the new arrow lands in");
pub const LAND_BOUNCE: Knob = Knob::new("Arrow bounce", 0.0, 0.5, 0.32).step(0.01).about("How much the landing arrow bounces");

pub const GROUP: Group = Group::new("Update pill", "window", &[OPEN, BOUNCE, CLOSE, DROP, LAND, LAND_BOUNCE]);
rune_register::register!(crate::knobs::GROUP_LIST: Group = [GROUP]);

/// What a pill shows: its plugin's `show()` answer.
pub struct Pill {
    pub key: &'static str,
    pub label: String,
    pub title: Option<String>,
    pub icon: &'static rune::Icon,
    pub busy: bool,
}

/// `color-mix(in srgb, a n%, b)` for opaque `a` and `b`.
fn mix_srgb(a: rune::Color, b: rune::Color, n: f32) -> rune::Color {
    use rune::color::{srgb_decode, srgb_encode};
    let m = |x: f32, y: f32| srgb_decode(srgb_encode(x) * n + srgb_encode(y) * (1.0 - n));
    rune::Color { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: a.a * n + b.a * (1.0 - n) }
}

/// The pointer entered (`inside`) or left pill `key`; entering one that isn't `busy` drops a
/// fresh arrow in.
pub fn hover(s: &mut App, key: &'static str, inside: bool, busy: bool, tx: &mut Tx) {
    let now = inside.then_some(key);
    if s.plugins.titlebar_hover == now || !(inside || s.plugins.titlebar_hover == Some(key)) {
        return tx.live_only();
    }
    s.plugins.titlebar_hover = now;
    if inside && !busy && !*s.appearance_state.reduced_motion && s.diag.knobs.get(&DROP) > 0.5 {
        *s.plugins.titlebar_drops.entry(key).or_default() += 1;
    }
    tx.animated();
}

/// `pill` as a titlebar (`vertical` false) or sidebar footer item; a press runs `press`.
pub fn view(app: &App, p: &Pal, vertical: bool, pill: Pill, press: Option<impl Fn(&mut App, &mut Tx) + 'static>) -> El {
    let k = &app.diag.knobs;
    let light_blue = (!p.dark).then(|| session_view::palette::token(false, "blue-700").unwrap_or(p.accent));
    let icon_color = light_blue.unwrap_or(p.v2.icon_accent);
    let label_color = light_blue.unwrap_or_else(|| session_view::palette::token(true, "text-text-accent").unwrap_or(p.accent));
    let tint = if p.dark { p.v2.icon_accent } else { p.bg_accent };
    let rest_fill = mix_srgb(tint, p.bg_deep, if p.dark { 0.20 } else { 0.10 });
    let hover_fill = mix_srgb(tint, p.bg_deep, if p.dark { 0.25 } else { 0.14 });
    let rtl = opencode_i18n::direction() == opencode_i18n::Direction::Rtl;
    let closed_x = if vertical == rtl { 8.0 } else { -8.0 };
    let key = pill.key;
    let hovered = app.plugins.titlebar_hover == Some(key);
    let open = hovered || app.plugins.titlebar_focus.as_deref() == Some(key);
    let side = if vertical { 28.0 } else { 20.0 };
    let busy = pill.busy;
    // Opening overshoots a touch past the label's 16 pt of room; closing never dips under the
    // circle. The fill and label ride the same spring.
    let spring = if open { Spring::new(k.get(&OPEN), k.get(&BOUNCE)) } else { Spring::new(k.get(&CLOSE), 0.0) };
    let glyph = if busy {
        rune_kit::Loader::new().size(12.0).color(p.v2.icon_accent).build(crate::palette::kit(p))
    } else {
        let n = app.plugins.titlebar_drops.get(key).copied().unwrap_or(0);
        let fall = side * 0.5;
        let drop = Transition::asymmetric(
            Transition::offset(0.0, -fall).combined(Transition::opacity()),
            Transition::offset(0.0, fall).combined(Transition::opacity()),
        )
        .springs(SpringPair::new(Spring::new(k.get(&LAND), k.get(&LAND_BOUNCE)), Spring::new(0.16, 0.0)));
        rune::icon(pill.icon, 14.0)
            .color(icon_color)
            .key(format!("plugin-titlebar-arrow-{key}-{n}"))
            .transition(drop)
            .enter_delay(0.05)
            .motion_intent(MotionIntent::OVERSHOOT)
    };
    let glyph = row().size(side, side).center().child(glyph);
    let label = text(pill.label.clone())
        .font(rune::TextStyle::new(11.0, 530.0).lh(16.0))
        .color(label_color)
        .opacity(if open { 1.0 } else { 0.0 })
        .opacity_motion(if open { Curve::tween(0.2, Easing::cubic(0.0, 0.0, 0.58, 1.0)) } else { Curve::tween(0.08, Easing::cubic(0.42, 0.0, 1.0, 1.0)) })
        .offset(if open { 0.0 } else { closed_x }, 0.0)
        .offset_motion(Spring::new(k.get(if open { &OPEN } else { &CLOSE }), 0.0));
    let label = if vertical { label.ml(1.0).mr(16.0) } else { label.ml(8.0).mr(1.0) };
    let inner = if vertical { row().child(glyph).child(label) } else { row().justify_end().child(label).child(glyph) };
    let mut button = inner
        .key(format!("plugin-titlebar-{key}"))
        .role("button")
        .label(pill.title.unwrap_or(pill.label))
        .h(side)
        .min_w(side)
        .center_y()
        .radius(side / 2.0)
        .focus_inset(p)
        .fill(if hovered { hover_fill } else { rest_fill })
        .clip_rounded(side / 2.0)
        .no_window_drag();
    if !open {
        button = button.w(side);
    }
    if busy {
        button = button.layer_opacity(0.6).disabled(true);
    } else if let Some(press) = press {
        button = button.cursor(rune::Cursor::Pointer).on_press_tx(press);
    }
    let mut group = el()
        .key(format!("plugin-titlebar-wrap-{key}"))
        .h(side)
        .min_w(side)
        .radius(side / 2.0)
        .fill(p.bg_deep)
        .clip_rounded(side / 2.0)
        .no_window_drag()
        .motion(spring)
        .motion_intent(MotionIntent::OVERSHOOT)
        .on_hover_change(move |s: &mut App, inside: bool, tx: &mut Tx| hover(s, key, inside, busy, tx))
        .child(button);
    if !open {
        group = group.w(side);
    }
    group
}
