//! Storybook: a tab dragged in from another window, over this window's strip. The tabs past the
//! slot it would take step aside on the strip's reorder spring, the gap follows the pointer, and
//! letting go puts the tab in the gap, in front.

use rune::{El, Play, col};
use rune_kit::shell::TabOrientation;

use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::tabs::Tab;

const VARIANTS: &[(&str, usize)] = &[("Top tabs", 0), ("Side tabs", 1)];

const KEYS: &[Trigger] = &[
    replay(arrive),
    Trigger { key: 'l', label: "Leave the strip", run: |s, tx| { s.tab_incoming(None, tx); } },
    Trigger { key: 'd', label: "Let go", run: let_go },
];

/// Where along the strip the dragged tab passes, as shares of its length.
const PATH: &[(f64, f32)] = &[(0.3, 0.9), (0.6, 0.62), (0.9, 0.38), (1.2, 0.2), (1.5, 0.05), (1.9, 0.3)];

fn scene(s: &mut App) {
    s.tabs = roots(s).into_iter().take(3).collect();
    if let Some(first) = s.tabs.first() {
        s.route = crate::Route::Session(first.id.clone());
    }
    stage(s, view);
}

pub(crate) fn roots(s: &App) -> Vec<Tab> {
    s.sessions.iter().filter(|x| x.parent.is_none()).map(|x| Tab::session(x.id.clone(), x.title.clone(), x.directory.clone())).collect()
}

/// The tab that comes in: the fixture's fourth session, never already on the strip.
pub(crate) fn incoming(s: &App) -> Option<Tab> {
    roots(s).into_iter().find(|t| !s.tabs.iter().any(|x| x.id == t.id))
}

/// A point `share` of the way along the strip, across its middle.
pub(crate) fn along(s: &App, share: f32) -> Option<[f32; 2]> {
    let r = s.window.strip?;
    Some(if s.variant == 1 { [r[0] + r[2] / 2.0, r[1] + share * r[3]] } else { [r[0] + share * r[2], r[1] + r[3] / 2.0] })
}

fn arrive(s: &mut App, tx: &mut Play<App>) {
    scene(s);
    s.tab_incoming(None, tx);
    for &(at, share) in PATH {
        tx.after(at, move |s, tx| {
            let point = along(s, share);
            s.tab_incoming(point, tx);
        });
    }
    tx.after(2.4, let_go);
}

fn let_go(s: &mut App, tx: &mut Play<App>) {
    let (Some(tab), Some(at)) = (incoming(s), s.tab_drag.incoming_slot().and(along(s, 0.3))) else { return };
    s.receive_tab(tab, None, Some(at), tx);
}

fn view(s: &App, p: &Pal) -> El {
    let side = s.variant == 1;
    let orientation = if side { TabOrientation::Vertical } else { TabOrientation::Horizontal };
    let strip = crate::view::tab_strip(s, p, "story-tabs", orientation, (!side).then_some(640.0), None);
    let frame = col().key("story-strip").fill(p.bg_deep).radius(10.0).padding(8.0);
    if side { frame.w(s.sidebar_w).h(240.0).child(strip) } else { frame.w(660.0).child(strip) }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Windows/Tab from another window", scene, knobs: &[], keys: KEYS, variants: VARIANTS },
]);
