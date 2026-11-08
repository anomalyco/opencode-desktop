//! The open diagram: the same card the reply draws, laid out at the column's width and drawn
//! larger with `El::scale`, over a scrim. It mounts exactly over its card in the reply, then
//! springs to the middle of the window, sized to fit; closing springs it back there before it
//! goes. Pinches, scrolls and drags move it at once, on the fingers.

use rune::{Cx, Drag, El, Phase as Pointer, Spring, SpringPair, Transition, Tx, el};

use super::{Open, Phase};
use crate::TimelineHost;
use crate::palette::Pal;

/// Points of the card's side padding (`rune_mermaid`'s card) on each side of its drawing.
const CARD_PAD: f32 = 16.0;
/// The most a fitted card grows; past that only a pinch or ⌘+ makes it bigger.
const FIT_MAX: f32 = 2.0;
/// The least and most the card draws at, as a multiple of its laid-out size.
const SCALE_MIN: f32 = 0.2;
const SCALE_MAX: f32 = 8.0;
/// Points of the card that stay in the window however far it's panned.
const KEEP: f32 = 64.0;

/// What the open diagram was last fitted to, in the zoomed shell's points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fit {
    /// The scale that fits the card in the window, at zoom 1.
    pub k: f32,
    pub win: [f32; 2],
    /// The card's laid-out size.
    pub size: [f32; 2],
}

impl Fit {
    /// The card's scale and top-left at `zoom` times the fit, `pan` points from the middle.
    pub fn place(&self, zoom: f32, pan: [f32; 2]) -> (f32, [f32; 2]) {
        let k = self.k * zoom;
        let centred = |i: usize| (self.win[i] - k * self.size[i]) / 2.0;
        (k, [centred(0) + pan[0], centred(1) + pan[1]])
    }

    /// `pan` held so at least [`KEEP`] points of the card stay in the window.
    fn clamp(&self, zoom: f32, pan: [f32; 2]) -> [f32; 2] {
        let k = self.k * zoom;
        let axis = |i: usize| {
            let reach = ((self.win[i] + k * self.size[i]) / 2.0 - KEEP.min(k * self.size[i])).max(0.0);
            pan[i].clamp(-reach, reach)
        };
        [axis(0), axis(1)]
    }

    /// Whether `(x, y)` is on the card at `zoom` and `pan`.
    pub fn hits(&self, zoom: f32, pan: [f32; 2], x: f32, y: f32) -> bool {
        let (k, [cx, cy]) = self.place(zoom, pan);
        x >= cx && y >= cy && x <= cx + k * self.size[0] && y <= cy + k * self.size[1]
    }
}

/// The scale that fits a card of `size` in a `win` window: inside 56 points top and bottom and
/// 3% of the width (20 to 48 points) at the sides, as the image viewer fits a still.
pub fn fit_scale(win: [f32; 2], size: [f32; 2]) -> f32 {
    let side = (win[0] * 0.03).clamp(8.0, 36.0) + 12.0;
    let (bw, bh) = ((win[0] - 2.0 * side).max(1.0), (win[1] - 2.0 * 56.0).max(1.0));
    (bw / size[0].max(1.0)).min(bh / size[1].max(1.0)).min(FIT_MAX)
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
        Pointer::Down => o.grab = Some([d.x, d.y]),
        Pointer::Move => {
            if let Some([gx, gy]) = o.grab.replace([d.x, d.y]) {
                pan_by(s, d.x - gx, d.y - gy);
            }
        }
        Pointer::Up => {
            let (zoom, pan) = (o.zoom, o.pan);
            o.grab = None;
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
    let Some(card) = rune_kit::mermaid::render(&o.seen.source, &o.seen.style) else { return Vec::new() };
    let shell = cx.layout("zoomed").or_else(|| cx.layout("window")).unwrap_or([0.0, 0.0, 1280.0, 820.0]);
    let [x0, y0, w0, h0] = o.seen.rect;
    let from = [x0 - shell[0], y0 - shell[1]];
    // The card hugs its drawing once it's out of the column.
    let drawing = cx.layout_size("diagram-lightbox/diagram");
    let height = cx.layout_size("diagram-lightbox").map_or(h0, |[_, h]| h);
    let hug = drawing.map_or(w0, |[w, _]| (w + 2.0 * CARD_PAD + 1.0).min(w0));
    let fit = Fit { k: 0.0, win: [shell[2], shell[3]], size: [hug, height] };
    let fit = Fit { k: fit_scale(fit.win, fit.size), ..fit };
    s.diagrams().fit.set(fit);
    let (width, k, [x, y]) = match o.phase {
        Phase::Open => {
            let (k, at) = fit.place(o.zoom, o.pan);
            (hug, k, at)
        }
        Phase::Opening | Phase::Closing { .. } => (w0, 1.0, from),
    };
    let spring = flight(s);
    let body = el()
        .key("diagram-lightbox")
        .role("dialog")
        .label(opencode_i18n::t!("rune.diagram.title"))
        .absolute(x, y)
        .w(width)
        .origin(0.0, 0.0)
        .scale(k)
        .motion(spring)
        .scale_motion(spring)
        .transition(Transition::none())
        .on_layout_tx(|s: &mut S, _, tx| match &mut s.diagrams_mut().open {
            Some(Open { phase, .. }) if *phase == Phase::Opening => {
                *phase = Phase::Open;
                tx.animated();
            }
            _ => tx.live_only(),
        })
        .child(card);
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
