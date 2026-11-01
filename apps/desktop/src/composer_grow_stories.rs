//! The storybook's Composer › Grows a line story: the empty composer holds one line, G types a
//! second and the toolbar rides down with the composer, D deletes back. R replays.

use rune::TextEdit;

use crate::stories::{Story, Trigger, replay};

const KEYS: &[Trigger] = &[
    replay(|s, _| s.composer = TextEdit::new("")),
    Trigger { key: 'g', label: "Type a second line", run: |s, _| s.composer = TextEdit::new("First line\nSecond line") },
    Trigger { key: 'd', label: "Delete back to one line", run: |s, _| s.composer = TextEdit::new("First line") },
];

rune_register::register!(crate::stories::STORY_LIST: Story = [Story::composer("Composer/Grows a line", KEYS)]);

