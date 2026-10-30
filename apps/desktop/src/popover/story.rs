//! The storybook's Popover › Open and close: the model list (or the agent menu) opening over the
//! composer, with the Popover knobs beside it (or set to their Grow preset). O opens, C closes, T toggles three times quickly
//! (open, close, open, 80 ms apart) to catch an interrupted entrance, R replays.

use rune::Play;
use rune_knobs::Group;

use crate::App;
use crate::pickers::Kind;
use crate::stories::{Story, Trigger, replay};

const KEYS: &[Trigger] = &[
    replay(start),
    Trigger { key: 'o', label: "Open", run: |s, tx| if s.pickers.open.is_none() { toggle(s, tx) } },
    Trigger { key: 'c', label: "Close", run: |s, tx| if s.pickers.open.is_some() { toggle(s, tx) } },
    Trigger { key: 't', label: "Toggle quickly", run: quick },
];

/// The third sets the knobs to the Grow preset, to see one that moves.
const VARIANTS: &[(&str, usize)] = &[("Model list", 0), ("Agent menu", 1), ("Grow preset", 2)];

const KNOBS: &[Group] = &[super::motion::GROUP];

fn kind(s: &App) -> Kind {
    if s.variant == 1 { Kind::Agent } else { Kind::Model }
}

/// Opens or closes the story's popover with the motion its trigger and Esc use.
fn toggle(s: &mut App, tx: &mut Play<App>) {
    // Neither the search nor the composer takes focus: a focused field would keep every
    // letter, O, C and T included.
    if s.pickers.open.is_some() {
        s.close_picker(false);
        super::motion::closing(&s.knobs, tx);
    } else {
        s.open_picker(kind(s));
        s.pickers.search.focused = false;
        super::motion::opening(&s.knobs, tx);
    }
}

fn start(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.settings.show_agent = true;
    s.close_picker(false);
    if s.variant == 2 {
        s.knobs.apply(&super::motion::GROUP, "Grow");
    }
    tx.instant();
    tx.after(0.4, toggle);
}

fn quick(s: &mut App, tx: &mut Play<App>) {
    tx.clear();
    toggle(s, tx);
    tx.after(0.08, toggle);
    tx.after(0.16, toggle);
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::composer("Popover/Open and close", KEYS).knobs(KNOBS).variants(VARIANTS),
]);
