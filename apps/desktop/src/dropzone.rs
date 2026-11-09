//! Desktop's drop overlay (`composer/dropzone.tsx`) over the page while a file drag is over it:
//! the page frosts under a 1.5 pt blur that fades out toward the edges, then a wash, faint
//! diagonal stripes fading out from the centre, an upload badge, and a label for what the
//! selected model reads (images, PDFs, both or neither), as `attach_path` sends them. It fades in
//! and out over Desktop's 200 ms ease-out cubic, at once under Reduce Motion.

use rune::{Anim, Color, Curve, Easing, El, Spring, SpringPair, TextStyle, Transition, Tx, col, el, icon, text};
use rune_theme_opencode::icons as ic;
use session_view::pickers::{AttachmentInput, PickerHost as _};

use crate::App;
use crate::palette::Pal;

/// `backdrop-blur-[1.5px]`.
const BLUR: f32 = 1.5;
const LABEL: TextStyle = TextStyle::new(15.0, 530.0).lh(24.0);

/// `duration-200 ease-[cubic-bezier(0.215,0.61,0.355,1)]`.
pub fn fade() -> Curve {
    Curve::tween(0.2, Easing::cubic(0.215, 0.61, 0.355, 1.0))
}

/// The page's file drag, Desktop's `drag` half of the composer machine (`ComposerInteractionState`
/// `drag: "idle" | "active"`, `suggestions/machine.ts`) with the overlay's presence
/// (`createAnimatedPresence`, `runtime/presence-machine.ts`): files that just landed keep the
/// overlay up through the instant commit that attaches them, and once the drag has gone the
/// overlay stays for its 200 ms exit, its label as it was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dropzone {
    #[default]
    Idle,
    /// A file drag is over the page (`drag.enter`).
    Over,
    Landed,
    /// The drag left or the drop settled: the content slides 4 pt down as it fades, until `due`.
    Leaving { due: Due, input: AttachmentInput },
}

/// When the exit ends, in the window clock's milliseconds (so the state stays `Eq`).
pub type Due = u64;

impl Dropzone {
    /// The overlay shows (`data-visible`).
    pub fn shown(self) -> bool {
        matches!(self, Dropzone::Over | Dropzone::Landed)
    }

    /// The overlay is on the page, showing or leaving (`presence.present()`).
    pub fn present(self) -> bool {
        self != Dropzone::Idle
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// The drag came over the page (`drag.enter`).
    Enter,
    /// The drag left (`drag.leave`) with the model reading `input`, at `now`.
    Leave { now: f64, input: AttachmentInput },
    Dropped,
    /// The frame after a drop (`drag.leave`).
    Settled { now: f64, input: AttachmentInput },
    /// The window clock reached `now`.
    Tick { now: f64 },
}

/// What the overlay's change asks of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// The overlay fades in.
    Fade,
    /// The overlay fades out, its exit ending at the time given (`linger`).
    FadeUntil(f64),
    /// The change lands at once: the chips a drop attached under the overlay, or the exit's end.
    Instant,
}

/// `duration-200`.
const EXIT: f64 = 0.2;

pub fn step(state: &mut Dropzone, event: Event) -> Option<Effect> {
    let leave = |now: f64, input| {
        let due = now + EXIT;
        (Dropzone::Leaving { due: (due * 1000.0).ceil() as Due, input }, Effect::FadeUntil(due))
    };
    let (next, effect) = match (*state, event) {
        (Dropzone::Idle | Dropzone::Leaving { .. }, Event::Enter) => (Dropzone::Over, Effect::Fade),
        (Dropzone::Over, Event::Leave { now, input }) => leave(now, input),
        (_, Event::Dropped) => (Dropzone::Landed, Effect::Instant),
        (Dropzone::Landed, Event::Settled { now, input }) => leave(now, input),
        (Dropzone::Leaving { due, .. }, Event::Tick { now }) if now * 1000.0 >= due as f64 => (Dropzone::Idle, Effect::Instant),
        _ => return None,
    };
    *state = next;
    Some(effect)
}

fn apply(effect: Option<Effect>, tx: &mut Tx) -> bool {
    match effect {
        Some(Effect::Fade) => tx.animated(),
        Some(Effect::FadeUntil(due)) => {
            tx.animated();
            tx.wake_at(due);
        }
        Some(Effect::Instant) => tx.instant(),
        None => return false,
    }
    true
}

pub fn hover(s: &mut App, over: bool, tx: &mut Tx) {
    let event = if over { Event::Enter } else { Event::Leave { now: tx.clock(), input: s.attachment_input() } };
    apply(step(&mut s.attach.drop, event), tx);
}

/// The drop's hover ends in the same commit, which lands the chips at once; the overlay keeps
/// showing through it and fades from `poll`.
pub fn drop(s: &mut App, paths: &[std::path::PathBuf], tx: &mut Tx) {
    apply(step(&mut s.attach.drop, Event::Dropped), tx);
    for p in paths {
        s.attach_path(p);
    }
}

pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let now = tx.clock();
    let event = match s.attach.drop {
        Dropzone::Landed => Event::Settled { now, input: s.attachment_input() },
        _ => Event::Tick { now },
    };
    apply(step(&mut s.attach.drop, event), tx)
}

impl App {
    /// What the selected model reads natively besides text.
    pub fn attachment_input(&self) -> AttachmentInput {
        self.pickers.attachment_input(&self.selection().model)
    }
}

/// The overlay's words for what the model reads (`dropzone.tsx:15-23`).
pub fn label(input: AttachmentInput) -> &'static str {
    match (input.image, input.pdf) {
        (false, false) => opencode_i18n::t!("ui.promptInput.dropFiles"),
        (true, false) => opencode_i18n::t!("ui.promptInput.dropFiles.image"),
        (false, true) => opencode_i18n::t!("ui.promptInput.dropFiles.pdf"),
        (true, true) => opencode_i18n::t!("ui.promptInput.dropFiles.imagePdf"),
    }
}

pub fn overlay(s: &App, p: &Pal, size: (f32, f32)) -> El {
    let input = match s.attach.drop {
        Dropzone::Leaving { input, .. } => input,
        _ => s.attachment_input(),
    };
    layer(p, size, s.attach.drop, label(input), s.reduced_motion)
}

/// The overlay's layer over a `w`x`h` page: always there, inert, and empty while hidden; its
/// contents stay while it fades out, the badge and label sliding 4 pt down as they go
/// (`data-[visible=false]:translate-y-1 opacity-0`).
pub fn layer(p: &Pal, (w, h): (f32, f32), zone: Dropzone, label: &str, reduced: bool) -> El {
    let shown = zone.shown();
    let curve = || if reduced { Anim::INSTANT } else { fade().into() };
    let layer = el()
        .key("drop-layer")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .inert()
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(curve());
    if !zone.present() || w <= 0.0 || h <= 0.0 {
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
    let words = col()
        .key("drop-content")
        .gap(20.0)
        .center()
        .offset(0.0, if shown { 0.0 } else { 4.0 })
        .offset_motion(curve())
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(curve())
        .child(badge)
        .child(text(label.to_owned()).font(LABEL).color(p.text));
    let content = el()
        .key("drop-overlay")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(p.text.alpha(wash))
        .transition(hold)
        .child(el().absolute(0.0, 0.0).w_full().h_full().fill(p.bg_base.alpha(0.25)).shape(stripes(w, h, p.text.alpha(stripe * 0.6))))
        .center()
        .child(words);
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
