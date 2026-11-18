//! The narrow window's two drawers (`session_view::sheet`): the titlebar's tabs and the
//! session's More, each on screen while its [`Presence`] says so.

use rune::{Drag, El, Phase, Tx};

use super::machine::{MoreEvent, Presence, TabsEvent};
use crate::App;
use crate::palette::Pal;

/// Corvu's bottom drawer has two snap points (closed/open). A drag is owned by a specific
/// handle, not whichever drawer happens to exist when the finger comes up.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Pull {
    #[default]
    Rest,
    Held { key: &'static str, dy: f32, start: rune::time::Instant },
}

impl Pull {
    pub(super) fn clear(&mut self, key: &str) {
        if matches!(self, Self::Held { key: owner, .. } if *owner == key) { *self = Self::Rest; }
    }

    fn step(&mut self, key: &'static str, d: &Drag, height: f32, zoom: f32) -> bool {
        let offset = (d.dy / zoom).clamp(0.0, height.max(0.0));
        match (*self, d.phase) {
            (_, Phase::Down) => *self = Self::Held { key, dy: 0.0, start: rune::time::Instant::now() },
            (Self::Held { key: owner, start, .. }, Phase::Move) if owner == key => *self = Self::Held { key, dy: offset, start },
            (Self::Held { key: owner, start, .. }, Phase::Up) if owner == key => {
                *self = Self::Rest;
                let pixels_per_ms = offset / (start.elapsed().as_secs_f32() * 1000.0).max(1.0);
                return !d.cancelled() && offset * pixels_per_ms.max(1.0) > height * 0.5;
            }
            _ => {}
        }
        false
    }
}

/// Both drawers, over the whole window (`Drawer.Portal`), while the window is narrow.
pub fn layer(s: &App, p: &Pal, cx: &mut rune::Cx<App>) -> Vec<El> {
    if !s.narrow() {
        return Vec::new();
    }
    let n = &s.layout.narrow;
    let win_h = cx.layout_size("zoom").map_or(800.0 / *s.zoom, |[_, h]| h);
    let mut out = Vec::new();
    for (key, presence, content, dismiss) in [
        (super::tabs::SHEET, n.tabs_presence, (|s, p| super::tabs::drawer(s, p)) as fn(&App, &Pal) -> El, (|s, tx| s.tabs_drawer(TabsEvent::Changed { open: false }, tx)) as fn(&mut App, &mut rune::Tx)),
        (super::session::MORE_SHEET, n.more_presence, |s, p| super::session::more(s, p), |s, tx| s.more_drawer(MoreEvent::Close, tx)),
    ] {
        let leaving = match presence {
            Presence::Gone => continue,
            Presence::Shown => false,
            Presence::Leaving { .. } => true,
        };
        let travel = cx.layout_size(key).map_or(win_h, |[_, h]| h);
        let dy = match n.pull { Pull::Held { key: owner, dy, .. } if owner == key => Some(dy), _ => None };
        let mut handle = session_view::sheet::handle(p);
        if cx.touch_controls() {
            handle = handle.on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
                let zoom = *s.zoom;
                if s.layout.narrow.pull.step(key, d, travel, zoom) { dismiss(s, tx); }
                else if d.phase == Phase::Up { tx.animated(); } else { tx.instant(); }
            });
        }
        out.extend(session_view::sheet::draggable_sheet(p, key, leaving, travel, win_h, content(s, p), dismiss, dy, handle));
    }
    out
}
