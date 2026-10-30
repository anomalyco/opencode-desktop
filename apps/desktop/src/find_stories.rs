//! The storybook's Find story (`Timeline/Find`): the session header's Find bar over the
//! timeline, highlighting every match across prompts, prose, inline code and code fences, and
//! stepping through them.

use rune::{Play, TextEdit};

use crate::App;
use crate::stories::{Story, Trigger, replay, stage, ticking_view};

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'n', label: "Next match", run: |s, tx| s.step_find(1, tx) },
    Trigger { key: 'p', label: "Previous match", run: |s, tx| s.step_find(-1, tx) },
    Trigger { key: 'c', label: "Open or close Find", run: toggle },
];

const VARIANTS: &[(&str, usize)] = &[("Matches", 0), ("Stepping", 1), ("No matches", 2)];

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.open_find(tx);
    // Leave input unfocused on replay so single-letter story triggers (R, N, P, C) work;
    // clicking the field focuses it for live typing.
    let q = if s.variant == 2 { "notpresent" } else { "stream" };
    s.find.input = TextEdit::new(q);
    s.find.input.focused = false;
    s.composer.focused = false;
    crate::find::typed(s, tx);
    if s.variant == 1 {
        tx.after(0.45, |s, tx| s.step_find(1, tx));
        tx.after(1.15, |s, tx| s.step_find(1, tx));
    }
}

fn toggle(s: &mut App, tx: &mut Play<App>) {
    if s.find.open {
        s.close_find(false, tx);
    } else {
        s.open_find(tx);
        s.find.input = TextEdit::new("stream");
        s.find.input.focused = false;
        crate::find::typed(s, tx);
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Timeline/Find", scene: |s| stage(s, ticking_view), knobs: &[], keys: KEYS, variants: VARIANTS },
]);
