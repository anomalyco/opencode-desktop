//! Desktop's narrow-window drawer (`mobile-drawer.tsx`, `mobile-drawer.css`): a sheet from the
//! window's bottom over a scrim, 16 pt top corners on the deep background with the overlay
//! elevation, a 28 pt handle row with its 32 × 4 pill, 12 pt sides and foot. It slides up over
//! 240 ms and back down over 200 on `cubic-bezier(0.2, 0.8, 0.2, 1)`; the scrim fades in over
//! 240 ms ease-out and out over 200 ease-in.

use rune::{Curve, Easing, El, Spring, SpringPair, Transition, Tx, col, el};

use super::machine::{Presence, SLIDE_IN, SLIDE_OUT};
use crate::App;
use crate::palette::{Elevate as _, Pal};

const SLIDE: Easing = Easing::cubic(0.2, 0.8, 0.2, 1.0);

/// Both drawers, over the whole window (`Drawer.Portal`), while the window is narrow.
pub fn layer(s: &App, p: &Pal, cx: &mut rune::Cx<App>) -> Vec<El> {
    if !s.narrow() {
        return Vec::new();
    }
    let n = &s.layout.narrow;
    let win_h = cx.layout_size("zoom").map_or(800.0, |[_, h]| h / s.zoom);
    let mut out = Vec::new();
    if n.tabs_presence.mounted() {
        let h = cx.layout_size(super::tabs::SHEET).map(|[_, h]| h);
        out.extend(sheet(p, super::tabs::SHEET, n.tabs_presence, h, win_h, super::tabs::drawer(s, p), |s, tx| {
            s.tabs_drawer(super::machine::TabsEvent::Changed { open: false }, tx)
        }));
    }
    if n.more_presence.mounted() {
        let h = cx.layout_size(super::session::MORE_SHEET).map(|[_, h]| h);
        out.extend(sheet(p, super::session::MORE_SHEET, n.more_presence, h, win_h, super::session::more(s, p), |s, tx| {
            s.more_drawer(super::machine::MoreEvent::Close, tx)
        }));
    }
    out
}

/// The scrim and the sheet keyed `key`, holding `content`, while `presence` shows it. `h` is the
/// sheet's height at the last layout and `win_h` the window's, both in shell points; a press on
/// the scrim runs `dismiss`.
pub fn sheet(p: &Pal, key: &str, presence: Presence, h: Option<f32>, win_h: f32, content: El, dismiss: fn(&mut App, &mut Tx)) -> Vec<El> {
    let leaving = match presence {
        Presence::Gone => return Vec::new(),
        Presence::Shown => false,
        Presence::Leaving { .. } => true,
    };
    let fade = if leaving { Curve::tween(SLIDE_OUT as f32, Easing::cubic(0.42, 0.0, 1.0, 1.0)) } else { Curve::tween(SLIDE_IN as f32, Easing::cubic(0.0, 0.0, 0.58, 1.0)) };
    let mut scrim = el()
        .key(format!("{key}-scrim"))
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(p.scrim)
        .opacity(if leaving { 0.0 } else { 1.0 })
        .opacity_motion(fade)
        .transition(Transition::asymmetric(Transition::opacity().springs(SpringPair::new(Spring::new(SLIDE_IN as f32, 0.0), Spring::NONE)), Transition::none()))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| dismiss(s, tx));
    let travel = h.unwrap_or(win_h);
    let slide = Curve::tween(if leaving { SLIDE_OUT } else { SLIDE_IN } as f32, SLIDE);
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
        .offset_motion(slide)
        .transition(Transition::asymmetric(
            Transition::move_edge(rune::Edge::Bottom).springs(SpringPair::new(Spring::new(SLIDE_IN as f32, 0.0), Spring::NONE)),
            Transition::none(),
        ))
        // Presses inside stay inside.
        .on_press(|_: &mut App| {})
        .child(
            el().key("handle").w_full().h(28.0).min_h(28.0).center().child(el().size(32.0, 4.0).radius(2.0).fill(p.q.border_strong)),
        )
        .child(content);
    if leaving {
        scrim = scrim.inert();
        panel = panel.inert();
    }
    vec![scrim, panel]
}
