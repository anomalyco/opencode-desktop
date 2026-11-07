//! Header › Narrow window: the session header over the composer at four widths, from the
//! narrowest panel a window can leave up to a roomy one, so what drops and folds shows side by
//! side (p263). S gives the header the closed sidebar's room; A shows the agent's picker.

use super::*;
use crate::stories::{Story, Trigger, replay, stage};

const WIDTHS: [f32; 4] = [200.0, 260.0, 330.0, 420.0];

const KEYS: &[Trigger] = &[
    replay(|s, _| s.finish_stream()),
    Trigger { key: 's', label: "Sidebar open or closed", run: |s, _| s.sidebar_open = !s.sidebar_open },
    Trigger { key: 'a', label: "Show the agent", run: |s, _| s.settings.show_agent = !s.settings.show_agent },
];

fn view(s: &App, p: &Pal) -> El {
    let Route::Session(id) = &s.route else { return el() };
    let (info, t) = (s.session(id), s.transcripts.get(id));
    let mut widths = col().key("narrow-widths").gap(16.0).items_center();
    for w in WIDTHS {
        let scope = format!("w{}", w as u32);
        widths = widths.child(
            col().key(format!("{scope}-frame")).w(w).radius(10.0).fill(p.bg_base).box_shadow(Pal::ring_out(p.ring)).clip_soft(0.0)
                .child(header_in(s, p, info, t, &scope))
                .child(col().key(format!("{scope}-dock")).w_full().px(DOCK_SIDE).pb(DOCK_SIDE).child(composer_in(s, p, &scope))),
        );
    }
    widths
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Header/Narrow window", scene: |s| stage(s, view), knobs: &[], keys: KEYS, variants: &[] },
]);
