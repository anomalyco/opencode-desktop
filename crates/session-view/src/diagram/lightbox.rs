//! The open diagram over a scrim: drawn again for the window, not the column (so a wide
//! flowchart isn't turned on its side or shrunk as it was there), in a card like the reply's
//! whose padding and border stay their size while the drawing scales with `El::scale`. It
//! mounts over its card in the reply, the drawing fitted inside, then springs to the middle of
//! the window, sized to fit; closing springs it back there before it goes. Pinches, scrolls and
//! drags move it at once, on the fingers.

use rune::{Cx, Drag, El, Phase as Pointer, Spring, SpringPair, Transition, Tx, el};

use super::{Gesture, Open, Phase};
use crate::TimelineHost;
use crate::palette::Pal;

/// Points of the card's padding (`rune_mermaid`'s card) on each side of its drawing.
const CARD_PAD: f32 = 16.0;
/// The most a fitted drawing grows; past that only a pinch or ⌘+ makes it bigger.
const FIT_MAX: f32 = 2.0;
/// The least and most the drawing draws at, as a multiple of its full size.
const SCALE_MIN: f32 = 0.2;
const SCALE_MAX: f32 = 8.0;
/// Points of the drawing that stay in the window however far it's panned.
const KEEP: f32 = 64.0;

/// What the open diagram was last fitted to, in the zoomed shell's points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fit {
    /// The scale that fits the drawing's card in the window, at zoom 1.
    pub k: f32,
    pub win: [f32; 2],
    /// The drawing's full size.
    pub size: [f32; 2],
}

impl Fit {
    /// The drawing's scale and top-left at `zoom` times the fit, `pan` points from the middle.
    pub fn place(&self, zoom: f32, pan: [f32; 2]) -> (f32, [f32; 2]) {
        let k = self.k * zoom;
        let centred = |i: usize| (self.win[i] - k * self.size[i]) / 2.0;
        (k, [centred(0) + pan[0], centred(1) + pan[1]])
    }

    /// `pan` held so at least [`KEEP`] points of the drawing stay in the window.
    fn clamp(&self, zoom: f32, pan: [f32; 2]) -> [f32; 2] {
        let k = self.k * zoom;
        let axis = |i: usize| {
            let reach = ((self.win[i] + k * self.size[i]) / 2.0 - KEEP.min(k * self.size[i])).max(0.0);
            pan[i].clamp(-reach, reach)
        };
        [axis(0), axis(1)]
    }

    /// The card around the drawing at `zoom` and `pan`: its padding stays its size.
    pub fn card(&self, zoom: f32, pan: [f32; 2]) -> [f32; 4] {
        let (k, [x, y]) = self.place(zoom, pan);
        [x - CARD_PAD, y - CARD_PAD, k * self.size[0] + 2.0 * CARD_PAD, k * self.size[1] + 2.0 * CARD_PAD]
    }

    /// Whether `(x, y)` is on the card at `zoom` and `pan`.
    pub fn hits(&self, zoom: f32, pan: [f32; 2], x: f32, y: f32) -> bool {
        let [cx, cy, w, h] = self.card(zoom, pan);
        x >= cx && y >= cy && x <= cx + w && y <= cy + h
    }
}

/// The box a card fits in, in a `win` window: inside 56 points top and bottom and 3% of the
/// width (20 to 48 points) at the sides, as the image viewer fits a still.
fn room(win: [f32; 2]) -> [f32; 2] {
    let side = (win[0] * 0.03).clamp(8.0, 36.0) + 12.0;
    [(win[0] - 2.0 * side).max(1.0), (win[1] - 2.0 * 56.0).max(1.0)]
}

/// The scale that fits a drawing of `size` in a `win` window with its card's padding around it.
pub fn fit_scale(win: [f32; 2], size: [f32; 2]) -> f32 {
    let [bw, bh] = room(win);
    contain([bw, bh], size).min(FIT_MAX)
}

/// The scale that fits `size` inside a card `box_` points big, padding and all.
fn contain(box_: [f32; 2], size: [f32; 2]) -> f32 {
    let inner = |i: usize| (box_[i] - 2.0 * CARD_PAD).max(1.0) / size[i].max(1.0);
    inner(0).min(inner(1))
}

/// The flight out and back: the image viewer's spring (its knob).
pub(super) fn flight<S: TimelineHost>(s: &S) -> Spring {
    Spring::new(s.knobs().get(&crate::knobs::IMAGE_FLIGHT), 0.0)
}

/// Scales the open diagram by `by` about `(x, y)`, which stays under the pointer.
pub fn pinch<S: TimelineHost>(s: &mut S, by: f32, x: f32, y: f32) {
    zoom_about(s, by, x, y);
}

/// Moves the open diagram `(dx, dy)` points.
pub fn pan<S: TimelineHost>(s: &mut S, dx: f32, dy: f32) {
    pan_by(s, dx, dy);
}

fn zoom_about<S: TimelineHost>(s: &mut S, by: f32, x: f32, y: f32) {
    let fit = s.diagrams().fit.get();
    let Some(o) = s.diagrams_mut().open.as_mut().filter(|o| o.phase == Phase::Open && fit.k > 0.0) else { return };
    let (k, [cx, cy]) = fit.place(o.zoom, o.pan);
    let zoom = (o.zoom * by).clamp(SCALE_MIN / fit.k, (SCALE_MAX / fit.k).max(1.0));
    let k2 = fit.k * zoom;
    let at = [x - (x - cx) / k * k2, y - (y - cy) / k * k2];
    let (_, centred) = fit.place(zoom, [0.0, 0.0]);
    o.zoom = zoom;
    o.pan = fit.clamp(zoom, [at[0] - centred[0], at[1] - centred[1]]);
}

fn pan_by<S: TimelineHost>(s: &mut S, dx: f32, dy: f32) {
    let fit = s.diagrams().fit.get();
    if let Some(o) = s.diagrams_mut().open.as_mut().filter(|o| o.phase == Phase::Open) {
        o.pan = fit.clamp(o.zoom, [o.pan[0] + dx, o.pan[1] + dy]);
    }
}

/// ⌘+ and ⌘−: `by` times as big about the middle of the window, on the flight's spring.
pub(super) fn zoom_step<S: TimelineHost>(s: &mut S, by: f32, tx: &mut Tx) {
    let [w, h] = s.diagrams().fit.get().win;
    zoom_about(s, by, w / 2.0, h / 2.0);
    tx.animated();
}

/// ⌘0: fitted and centred again.
pub(super) fn fit<S: TimelineHost>(s: &mut S, tx: &mut Tx) {
    if let Some(o) = s.diagrams_mut().open.as_mut().filter(|o| o.phase == Phase::Open) {
        o.zoom = 1.0;
        o.pan = [0.0, 0.0];
    }
    tx.animated();
}

/// Smart magnify and a double-click on the card: fitted goes to twice as big about the
/// pointer, anything else back to fitted.
fn toggle<S: TimelineHost>(s: &mut S, x: f32, y: f32, tx: &mut Tx) {
    if s.diagrams().open.as_ref().is_some_and(|o| (o.zoom - 1.0).abs() < 0.01 && o.pan == [0.0, 0.0]) {
        zoom_about(s, 2.0, x, y);
    } else {
        fit(s, tx);
    }
    tx.animated();
}

fn drag<S: TimelineHost>(s: &mut S, d: &mut Drag, tx: &mut Tx) {
    let fit = s.diagrams().fit.get();
    let Some(o) = s.diagrams_mut().open.as_mut() else { return };
    match d.phase {
        Pointer::Down => o.gesture = Gesture::Panning { last: [d.x, d.y] },
        Pointer::Move => {
            if let Gesture::Panning { last: [gx, gy] } = std::mem::replace(&mut o.gesture, Gesture::Panning { last: [d.x, d.y] }) {
                pan_by(s, d.x - gx, d.y - gy);
            }
        }
        Pointer::Up => {
            let (zoom, pan) = (o.zoom, o.pan);
            o.gesture = Gesture::Idle;
            let still = d.dx.hypot(d.dy) < 4.0 && !d.cancelled();
            if still && !fit.hits(zoom, pan, d.x, d.y) {
                super::close(s, tx);
            } else if still && d.clicks == 2 {
                toggle(s, d.x, d.y, tx);
            }
        }
    }
}

/// The open diagram over the whole window: the scrim, and the card over it.
pub fn view<S: TimelineHost>(s: &S, p: &Pal, cx: &Cx<S>) -> Vec<El> {
    let Some(o) = &s.diagrams().open else { return Vec::new() };
    let shell = cx.layout("zoomed").or_else(|| cx.layout("window")).unwrap_or([0.0, 0.0, 1280.0, 820.0]);
    let win = [shell[2], shell[3]];
    let Some(drawing) = rune_kit::mermaid::drawing(&o.seen.source, &o.seen.style, room(win)[0]) else { return Vec::new() };
    let fit = Fit { k: fit_scale(win, drawing.size), win, size: drawing.size };
    s.diagrams().fit.set(fit);
    let (card, k) = match o.phase {
        Phase::Open => (fit.card(o.zoom, o.pan), fit.place(o.zoom, o.pan).0),
        // Over its card in the reply, the drawing fitted inside it.
        Phase::Opening | Phase::Closing { .. } => {
            let [x, y, w, h] = o.seen.rect;
            ([x - shell[0], y - shell[1], w, h], contain([w, h], drawing.size))
        }
    };
    let inset = |i: usize| (card[2 + i] - k * drawing.size[i]) / 2.0;
    let spring = flight(s);
    let body = el()
        .key("diagram-lightbox")
        .role("dialog")
        .label(opencode_i18n::t!("rune.diagram.title"))
        .absolute(card[0], card[1])
        .size(card[2], card[3])
        .radius(6.0)
        .fill(drawing.surface)
        .border(0.5, drawing.surface_border)
        .clip_rounded(6.0)
        .motion(spring)
        .transition(Transition::none())
        .on_layout_tx(|s: &mut S, _, tx| match &mut s.diagrams_mut().open {
            Some(Open { phase, .. }) if *phase == Phase::Opening => {
                *phase = Phase::Open;
                tx.animated();
            }
            _ => tx.live_only(),
        })
        .child(
            el().key("diagram-lightbox-drawing")
                .absolute(inset(0), inset(1))
                .size(drawing.size[0], drawing.size[1])
                .origin(0.0, 0.0)
                .scale(k)
                .scale_motion(spring)
                .child(drawing.el),
        );
    let fade = SpringPair::new(Spring::new(0.15, 0.0), Spring::new(0.12, 0.0));
    let open = !matches!(o.phase, Phase::Closing { .. });
    let mut stage = el()
        .key("diagram-stage")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .transition(Transition::none())
        .modal(false, |s: &mut S| {
            super::close_later(s);
        });
    if open {
        stage = stage
            .on_magnify(|s: &mut S, e, tx| {
                zoom_about(s, 1.0 + e.delta, e.x, e.y);
                tx.instant();
                true
            })
            .on_smart_magnify(|s: &mut S, x, y, tx| {
                toggle(s, x, y, tx);
                true
            })
            .on_wheel(|s: &mut S, e, tx| {
                if e.mods.cmd {
                    zoom_about(s, (-e.dy * 0.01).exp(), e.x, e.y);
                } else {
                    // Deltas come in window points; the shell is laid out at 1/zoom.
                    let z = s.zoom().max(0.01);
                    pan_by(s, e.dx / z, e.dy / z);
                }
                tx.instant();
                true
            })
            .on_drag(drag::<S>)
            .child(el().key("diagram-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::opacity().springs(fade)));
    }
    vec![stage.child(body)]
}
