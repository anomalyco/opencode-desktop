//! The storybook's Scroll bar story (Timeline › Scroll bar): the long session's timeline with
//! Rune's scroll bar, which follows System Settings' "Show scroll bars". R glides up a few
//! screens, rests so the bar fades, then glides back to the end; U pages up, D pages down; S
//! streams the last reply in, so the thumb takes the longer content. Drag the thumb.

use rune::Play;

use crate::App;
use crate::stories::{Story, Trigger, replay, scroll_key};

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'u', label: "Page up", run: |s, tx| tx.scroll_page(scroll_key(s), -1) },
    Trigger { key: 'd', label: "Page down", run: |s, tx| tx.scroll_page(scroll_key(s), 1) },
    Trigger { key: 's', label: "Stream the reply", run: crate::stories::stream_open },
];

/// Three pages up at once, a rest long enough for the bar to fade, then the glide back down.
fn open(_: &mut App, tx: &mut Play<App>) {
    tx.after(0.4, |s, tx| tx.scroll_page(scroll_key(s), -3));
    tx.after(2.6, |s, tx| tx.scroll_to_bottom(scroll_key(s)));
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::session("Timeline/Scroll bar", KEYS),
]);
