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

const VARIANTS: &[(&str, usize)] = &[("Matches", 0), ("Stepping", 1), ("No matches", 2), ("Folded prompt", 3)];

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    if s.fixture_run.variant == 3 {
        let sid = crate::stories::sid(s);
        if let Some(t) = s.transcripts.get_mut(&sid) {
            t.messages.push(opencode_kit::model::Message::User {
                id: "msg_zzfold_story".into(),
                text: crate::stories::timeline::fold::LONG.into(),
            });
        }
    }
    s.open_find(tx);
    // Leave input unfocused on replay so single-letter story triggers (R, N, P, C) work;
    // clicking the field focuses it for live typing.
    let q = match *s.fixture_run.variant {
        2 => "notpresent",
        3 => "interface change",
        _ => "stream",
    };
    {
        let f = s.kernel.slots.get_mut(&crate::find::FIND);
        f.input = TextEdit::new(q);
        f.input.focused = false;
    }
    s.composer.focused = false;
    crate::find::typed(s, tx);
    if s.fixture_run.variant == 1 {
        tx.after(0.45, |s, tx| s.step_find(1, tx));
        tx.after(1.15, |s, tx| s.step_find(1, tx));
    }
}

fn toggle(s: &mut App, tx: &mut Play<App>) {
    if s.kernel.slots.get(&crate::find::FIND).is_open() {
        s.close_find(tx);
    } else {
        s.open_find(tx);
        {
            let f = s.kernel.slots.get_mut(&crate::find::FIND);
            f.input = TextEdit::new("stream");
            f.input.focused = false;
        }
        crate::find::typed(s, tx);
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Timeline/Find", scene: |s| stage(s, ticking_view), knobs: &[], keys: KEYS, variants: VARIANTS },
]);
