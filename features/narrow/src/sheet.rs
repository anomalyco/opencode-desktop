//! The narrow window's two drawers (`opencode_ui::sheet`): the titlebar's tabs and the
//! session's More, each on screen while its [`Presence`] says so.

use rune::{Cx, Drag, El, Phase, Tx};
use opencode_ui::palette::Pal;

use crate::host::NarrowHost;
use crate::machine::{MoreEvent, Presence, TabsEvent};

/// Corvu's bottom drawer has two snap points (closed/open). A drag is owned by a specific
/// handle, not whichever drawer happens to exist when the finger comes up.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Pull {
    #[default]
    Rest,
    Held { key: &'static str, dy: f32, start: rune::time::Instant },
}

impl Pull {
    pub(crate) fn clear(&mut self, key: &str) {
        if matches!(self, Self::Held { key: owner, .. } if *owner == key) {
            *self = Self::Rest;
        }
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

fn dismiss_tabs_dyn(host: &mut dyn NarrowHost, tx: &mut Tx) {
    crate::tabs_drawer(host, TabsEvent::Changed { open: false }, tx);
}

fn dismiss_more_dyn(host: &mut dyn NarrowHost, tx: &mut Tx) {
    crate::more_drawer(host, MoreEvent::Close, tx);
}

fn drag_sheet(host: &mut dyn NarrowHost, key: &'static str, d: &Drag, travel: f32, dismiss: fn(&mut dyn NarrowHost, &mut Tx), tx: &mut Tx) {
    let zoom = host.zoom();
    if host.narrow_state_mut().pull.step(key, d, travel, zoom) {
        dismiss(host, tx);
    } else if d.phase == Phase::Up {
        tx.animated();
    } else {
        tx.instant();
    }
}

fn dismiss_tabs<H: NarrowHost + 'static>(s: &mut H, tx: &mut Tx) {
    dismiss_tabs_dyn(s, tx);
}

fn dismiss_more<H: NarrowHost + 'static>(s: &mut H, tx: &mut Tx) {
    dismiss_more_dyn(s, tx);
}

/// Both drawers, over the whole window (`Drawer.Portal`), while the window is narrow.
pub fn layer<H: NarrowHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    if !crate::narrow(s) {
        return Vec::new();
    }
    let n = s.narrow_state();
    let (tabs_presence, more_presence, pull) = (n.tabs_presence, n.more_presence, n.pull);
    let win_h = cx.layout_size("zoom").map_or(800.0 / s.zoom(), |[_, h]| h);
    let mut out = Vec::new();
    for (key, presence, content, dismiss_h, dismiss_dyn) in [
        (
            crate::tabs::SHEET,
            tabs_presence,
            crate::tabs::drawer::<H> as fn(&H, &Pal) -> El,
            dismiss_tabs::<H> as fn(&mut H, &mut Tx),
            dismiss_tabs_dyn as fn(&mut dyn NarrowHost, &mut Tx),
        ),
        (
            crate::session::MORE_SHEET,
            more_presence,
            crate::session::more::<H> as fn(&H, &Pal) -> El,
            dismiss_more::<H> as fn(&mut H, &mut Tx),
            dismiss_more_dyn as fn(&mut dyn NarrowHost, &mut Tx),
        ),
    ] {
        let leaving = match presence {
            Presence::Gone => continue,
            Presence::Shown => false,
            Presence::Leaving { .. } => true,
        };
        let travel = cx.layout_size(key).map_or(win_h, |[_, h]| h);
        let dy = match pull {
            Pull::Held { key: owner, dy, .. } if owner == key => Some(dy),
            _ => None,
        };
        let mut handle = opencode_ui::sheet::handle(p);
        if cx.touch_controls() {
            handle = handle.on_drag(move |s: &mut H, d: &mut Drag, tx: &mut Tx| {
                drag_sheet(s, key, d, travel, dismiss_dyn, tx);
            });
        }
        out.extend(opencode_ui::sheet::draggable_sheet(p, key, leaving, travel, win_h, content(s, p), dismiss_h, dy, handle));
    }
    out
}
