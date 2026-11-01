//! The storybook's Settings › Page story: the whole page with its Back to app button. B presses
//! the button (it closes the page in the real app), R replays.

use rune::{El, col};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Trigger, replay};

pub const KEYS: &[Trigger] = &[
    replay(|_, _| {}),
    Trigger { key: 'b', label: "Press Back to app", run: |s, tx| { tx.instant(); s.close_settings() } },
];

fn view(s: &App, p: &Pal) -> El {
    col().key("settings-story").w(720.0).h(560.0).fill(p.bg_base).child(crate::settings::page(s, p))
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story { path: "Settings/Page", scene: |s| crate::stories::stage(s, view), knobs: &[], keys: KEYS, variants: &[] },
]);

// Settings with the sidebar hidden: the back button sits right after the sidebar toggle.
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("settings-sidebar-closed", |s| {
        crate::open_scene(s);
        s.finish_stream();
        s.sidebar_open = false;
        s.open_settings();
    }),
]);
