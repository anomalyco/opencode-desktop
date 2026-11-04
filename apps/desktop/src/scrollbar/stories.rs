//! The storybook's Scroll bar story (Timeline › Scroll bar): the long session's timeline with
//! its overlay scroll bar. R glides up a few screens, rests so the bar fades, then glides back
//! to the end; U pages up, D pages down; S streams the last reply in, so the thumb settles to the
//! longer content. Hover the right edge to widen it, and drag the thumb.

use rune::Play;
use rune_knobs::Group;

use super::{FADE_DELAY, HOT_WIDTH, WIDTH};
use crate::App;
use crate::stories::{Story, Trigger, replay, scroll_key};

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'u', label: "Page up", run: |s, tx| tx.scroll_page(scroll_key(s), -1) },
    Trigger { key: 'd', label: "Page down", run: |s, tx| tx.scroll_page(scroll_key(s), 1) },
    Trigger { key: 's', label: "Stream the reply", run: crate::stories::stream_open },
];

const KNOBS: &[Group] = &[Group::new("Scroll bar", "stage", &[WIDTH, HOT_WIDTH, FADE_DELAY])];

/// Three pages up at once, a rest long enough for the bar to fade, then the glide back down.
fn open(_: &mut App, tx: &mut Play<App>) {
    tx.after(0.4, |s, tx| tx.scroll_page(scroll_key(s), -3));
    tx.after(2.6, |s, tx| tx.scroll_to_bottom(scroll_key(s)));
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::session("Timeline/Scroll bar", KEYS).knobs(KNOBS),
]);
