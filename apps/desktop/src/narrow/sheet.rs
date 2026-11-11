//! The narrow window's two drawers (`session_view::sheet`): the titlebar's tabs and the
//! session's More, each on screen while its [`Presence`] says so.

use rune::El;

use super::machine::{MoreEvent, Presence, TabsEvent};
use crate::App;
use crate::palette::Pal;

/// Both drawers, over the whole window (`Drawer.Portal`), while the window is narrow.
pub fn layer(s: &App, p: &Pal, cx: &mut rune::Cx<App>) -> Vec<El> {
    if !s.narrow() {
        return Vec::new();
    }
    let n = &s.layout.narrow;
    let win_h = cx.layout_size("zoom").map_or(800.0 / s.zoom, |[_, h]| h);
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
        out.extend(session_view::sheet::sheet(p, key, leaving, travel, win_h, content(s, p), dismiss));
    }
    out
}
