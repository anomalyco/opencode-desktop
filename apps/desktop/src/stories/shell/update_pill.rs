//! Storybook stage for the Updater's pill (`Components/Update pill`): the titlebar's 20 pt pill at
//! the end of a titlebar strip and the sidebar footer's 28 pt one, the pointer's to hover. H and
//! T hover one, L leaves. `--playground 'Components/Update pill' --script apps/desktop/scripts/update-pill.json`
//! hovers each in turn.

use rune::{El, Play, Tx, col, row};

use crate::App;
use crate::palette::Pal;
use crate::plugin_host::ui::titlebar_pill::{self, DOWNLOAD, Pill};
use crate::stories::{Story, Trigger, replay, stage};

const SIDE: &str = "story-update-side";
const TOP: &str = "story-update-top";

const KNOBS: &[rune_knobs::Group] = &[titlebar_pill::GROUP];

const VARIANTS: &[(&str, usize)] = &[("Ready", 0), ("Installing", 1)];

const KEYS: &[Trigger] = &[
    replay(|s, _| s.plugins.titlebar_hover = None),
    Trigger { key: 'h', label: "Hover the sidebar's", run: |s, play| enter(s, SIDE, play) },
    Trigger { key: 't', label: "Hover the titlebar's", run: |s, play| enter(s, TOP, play) },
    Trigger { key: 'l', label: "Leave", run: leave },
];

fn enter(s: &mut App, key: &'static str, play: &mut Play<App>) {
    leave(s, play);
    titlebar_pill::hover(s, key, true, busy(s), play);
}

fn leave(s: &mut App, play: &mut Play<App>) {
    if let Some(key) = s.plugins.titlebar_hover {
        titlebar_pill::hover(s, key, false, busy(s), play);
    }
}

fn busy(s: &App) -> bool {
    *s.fixture_run.variant == 1
}

fn pill(s: &App, p: &Pal, key: &'static str, vertical: bool) -> El {
    let pill = Pill { key, label: "Update".into(), title: Some("Restart to update".into()), icon: &DOWNLOAD, busy: busy(s) };
    titlebar_pill::view(s, p, vertical, pill, Some(|_: &mut App, _: &mut Tx| {}))
}

/// A titlebar strip with the pill 12 before its end, over a sidebar's foot with the pill 8 in.
fn view(s: &App, p: &Pal) -> El {
    let top = row().key("story-update-titlebar").w(420.0).h(40.0).justify_end().center_y().pr(12.0).fill(p.bg_deep).child(pill(s, p, TOP, false));
    let side = col().key("story-update-sidebar").w(240.0).h(96.0).justify_end().items_start().padding(8.0).fill(p.bg_deep).child(pill(s, p, SIDE, true));
    col().key("story-update-pill").gap(40.0).items_center().child(top).child(side)
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Components/Update pill", scene: |s| stage(s, view), knobs: KNOBS, keys: KEYS, variants: VARIANTS },
]);
