//! Desktop's drop overlay (`composer/dropzone.tsx`) over the page while a file drag is over it:
//! the page frosts under a 1.5 pt blur that fades out toward the edges, then a wash, faint
//! diagonal stripes fading out from the centre, an upload badge, and the label for a model that
//! reads images and PDFs, as `attach_path` sends them. It fades in and out over Desktop's
//! 200 ms ease-out cubic, at once under Reduce Motion.

use rune::{Anim, Color, Curve, Easing, El, Spring, SpringPair, TextStyle, Transition, Tx, col, el, icon, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;

/// `backdrop-blur-[1.5px]`.
const BLUR: f32 = 1.5;
const LABEL: TextStyle = TextStyle::new(15.0, 530.0).lh(24.0);

/// `duration-200 ease-[cubic-bezier(0.215,0.61,0.355,1)]`.
pub fn fade() -> Curve {
    Curve::tween(0.2, Easing::cubic(0.215, 0.61, 0.355, 1.0))
}

/// A file drag is over the page, or has left it.
#[derive(Default)]
pub struct Dropzone {
    pub over: bool,
    /// Files just landed: the overlay fades out on the next frame, after the instant commit
    /// that attaches them.
    landed: bool,
}

pub fn hover(s: &mut App, over: bool, tx: &mut Tx) {
    tx.animated();
    s.attach.drop.over = over;
}

/// The drop's hover ends in the same commit, which lands the chips at once; the overlay keeps
/// showing through it and fades from `poll`.
pub fn drop(s: &mut App, paths: &[std::path::PathBuf], tx: &mut Tx) {
    tx.instant();
    s.attach.drop.over = true;
    s.attach.drop.landed = true;
    for p in paths {
        s.attach_path(p);
    }
}

pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if !std::mem::take(&mut s.attach.drop.landed) {
        return false;
    }
    tx.animated();
    s.attach.drop.over = false;
    true
}

pub fn overlay(s: &App, p: &Pal, size: (f32, f32)) -> El {
    layer(p, size, s.attach.drop.over, s.reduced_motion)
}

/// The overlay's layer over a `w`×`h` page: always there, inert, and empty while hidden; its
/// contents stay while it fades out.
pub fn layer(p: &Pal, (w, h): (f32, f32), shown: bool, reduced: bool) -> El {
    let layer = el()
        .key("drop-layer")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .inert()
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(if reduced { Anim::INSTANT } else { fade().into() });
    if !shown || w <= 0.0 || h <= 0.0 {
        return layer;
    }
    // Held until the layer has faded (its opacity does the fading).
    let hold = Transition::identity().springs(SpringPair::new(Spring::new(0.3, 0.0), Spring::new(0.3, 0.0)));
    let (wash, stripe, card) = if p.dark { (0.06, 0.14, p.layer1) } else { (0.04, 0.07, p.bg_base) };
    let badge = el()
        .size(40.0, 40.0)
        .radius(20.0)
        .fill(card)
        .shadow(4.0, 12.0, p.shadow)
        .center()
        .child(icon(&ic::ARROW_UP, 20.0).color(p.icon_muted));
    // Desktop's mask: 0→1 over the outer 25% across and 28% down, the two multiplied.
    let frost = el()
        .key("drop-frost")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .clip_fade(0.25 * w, 0.28 * h, 0.25 * w, 0.28 * h)
        .transition(hold)
        .child(el().w_full().h_full().backdrop_blur(BLUR));
    let content = el()
        .key("drop-overlay")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(p.text.alpha(wash))
        .transition(hold)
        .child(el().absolute(0.0, 0.0).w_full().h_full().fill(p.bg_base.alpha(0.25)).shape(stripes(w, h, p.text.alpha(stripe * 0.6))))
        .center()
        .child(col().gap(20.0).center().child(badge).child(text(opencode_i18n::t!("ui.promptInput.dropFiles.imagePdf")).font(LABEL).color(p.text)));
    layer.child(frost).child(content)
}

/// `repeating-linear-gradient(135deg, …12px, ink 12px 24px)` across the session column,
/// masked by Desktop's horizontal fade (12%–88%) and centred ellipse (59%×40%: full, 0.72 at
/// 58%, gone at the rim). Each stripe is cut into short quads that take the mask at their centre.
fn stripes(w: f32, h: f32, ink: Color) -> rune::Shape {
    const PERIOD: f32 = 24.0;
    const STEP: f32 = 12.0;
    let k = std::f32::consts::FRAC_1_SQRT_2;
    let cw = w.min(session_view::style::content_w() + 48.0);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let at = |t: f32, u: f32| [cx + (t + u) * k, cy + (t - u) * k];
    let mask = |[x, y]: [f32; 2]| {
        let fx = (x - (cx - cw / 2.0)) / cw;
        let side = (fx / 0.12).min((1.0 - fx) / 0.12).clamp(0.0, 1.0);
        let r = (((x - cx) / (0.59 * cw)).powi(2) + ((y - cy) / (0.4 * h)).powi(2)).sqrt();
        let radial = if r < 0.58 { 1.0 - 0.28 * r / 0.58 } else { (0.72 * (1.0 - r) / 0.42).max(0.0) };
        side * radial
    };
    let reach = (cw + h) * k / 2.0;
    let mut shape = rune::Shape::new();
    let mut t = -(reach / PERIOD).ceil() * PERIOD + PERIOD / 2.0;
    while t < reach {
        let mut u = -reach;
        while u < reach {
            let a = ink.a * mask(at(t + PERIOD / 4.0, u + STEP / 2.0));
            if a > 0.002 {
                let quad = vec![at(t, u), at(t + PERIOD / 2.0, u), at(t + PERIOD / 2.0, u + STEP), at(t, u + STEP)];
                shape = shape.fill(quad, ink.alpha(a));
            }
            u += STEP;
        }
        t += PERIOD;
    }
    shape
}
