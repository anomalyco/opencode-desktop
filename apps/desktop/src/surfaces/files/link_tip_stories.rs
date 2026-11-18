//! Storybook stage for `Files/Link tips`: a reply whose links say where they go on hover.

use rune::{El, Play, col};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, replay, stage};

const REPLY: &str = "The plan is in [the proposal](/Users/demo/code/lumen/docs/proposal.md#L12-L20), \
following [Rust's guidance](https://doc.rust-lang.org/book/ch09-00-error-handling.html?x=1). \
Older notes are [here](link).";

fn setup(s: &mut App, _: &mut Play<'_, App>) {
    crate::open_scene(s);
    stage(s, card);
}

fn card(_: &App, p: &Pal) -> El {
    col().key("link-tips").w(520.0).px(20.0).py(20.0).child(crate::view::markdown_tinted(p, REPLY, 0.0, false, p.text, 480.0))
}

rune_register::register!(crate::stories::STORY_LIST: Story = [Story {
    path: "Files/Link tips",
    scene: |s| {
        crate::open_scene(s);
        stage(s, card);
    },
    knobs: &[],
    keys: &[replay(setup)],
    variants: &[],
}]);
