//! Desktop's narrow-window drawer (`mobile-drawer.tsx`, `mobile-drawer.css`): a sheet from the
//! window's bottom over a scrim, 16 pt top corners on the deep background with the overlay
//! elevation, a 28 pt handle row with its 32 × 4 pill, 12 pt sides and foot. It slides up over
//! 240 ms and back down over 200 on `cubic-bezier(0.2, 0.8, 0.2, 1)`; the scrim fades in over
//! 240 ms ease-out and out over 200 ease-in. The host says whether it is `leaving` (sliding out
//! by `travel`, its height at the last layout) and what a press on the scrim does.

use rune::{Curve, Easing, El, Spring, SpringPair, Transition, Tx, col, el};

use crate::palette::{Elevate as _, Pal};

/// The slide in (`transform 240ms`).
pub const SLIDE_IN: f32 = 0.24;
/// The slide out (`[data-closing] { transition-duration: 200ms }`).
pub const SLIDE_OUT: f32 = 0.2;

const SLIDE: Easing = Easing::cubic(0.2, 0.8, 0.2, 1.0);

/// The scrim and the sheet keyed `key` holding `content`, in a window `win_h` tall.
pub fn sheet<S: 'static>(p: &Pal, key: &str, leaving: bool, travel: f32, win_h: f32, content: El, dismiss: fn(&mut S, &mut Tx)) -> Vec<El> {
    let fade = if leaving { Curve::tween(SLIDE_OUT, Easing::cubic(0.42, 0.0, 1.0, 1.0)) } else { Curve::tween(SLIDE_IN, Easing::cubic(0.0, 0.0, 0.58, 1.0)) };
    let mut scrim = el()
        .key(format!("{key}-scrim"))
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(p.scrim)
        .opacity(if leaving { 0.0 } else { 1.0 })
        .opacity_motion(fade)
        .transition(Transition::asymmetric(Transition::opacity().springs(SpringPair::new(Spring::new(SLIDE_IN, 0.0), Spring::NONE)), Transition::none()))
        .on_press_tx(move |s: &mut S, tx: &mut Tx| dismiss(s, tx));
    let mut panel = col()
        .key(key.to_owned())
        .role("dialog")
        .pin(Some(0.0), None, Some(0.0), Some(0.0))
        .max_h((win_h * 0.75).min(win_h - 16.0))
        .px(12.0)
        .pb(12.0)
        .corners([16.0, 16.0, 0.0, 0.0])
        .fill(p.bg_deep)
        .elevate(p.overlay)
        .no_window_drag()
        .offset(0.0, if leaving { travel } else { 0.0 })
        .offset_motion(Curve::tween(if leaving { SLIDE_OUT } else { SLIDE_IN }, SLIDE))
        .transition(Transition::asymmetric(
            Transition::move_edge(rune::Edge::Bottom).springs(SpringPair::new(Spring::new(SLIDE_IN, 0.0), Spring::NONE)),
            Transition::none(),
        ))
        // Presses inside stay inside.
        .on_press(|_: &mut S| {})
        .child(el().key("handle").w_full().h(28.0).min_h(28.0).center().child(el().size(32.0, 4.0).radius(2.0).fill(p.q.border_strong)))
        .child(content);
    if leaving {
        scrim = scrim.inert();
        panel = panel.inert();
    }
    vec![scrim, panel]
}
