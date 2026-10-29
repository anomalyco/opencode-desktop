//! The storybook's Composer › History story: the composer is empty over three sent prompts, then
//! ↑ ↑ ↑ walks back to the oldest and ↓ ↓ ↓ returns to the empty draft. R replays.

use rune::{Key, KeyEvent, Mods, Play, Step, TextEdit, Tx};

use crate::App;
use crate::prompt_history::{Entry, PromptHistory};
use crate::stories::{Trigger, replay};

pub const KEYS: &[Trigger] = &[replay(open)];

fn arrow(s: &mut App, tx: &mut Tx, key: Key) {
    s.history_key(&KeyEvent { key, mods: Mods::default() }, tx);
}

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.prompts = PromptHistory::default();
    for text in ["Add a retry budget to the stream reader", "Why does the sidebar jump on resize?", "Write a test for the fold toggle"] {
        s.prompts.add(Entry { text: text.into(), pills: Vec::new() }, false);
    }
    s.composer = TextEdit::new("");
    s.composer.focused = true;
    let steps: [(f64, Step<App>); 6] = [
        (0.8, |s, tx| arrow(s, tx, Key::Up)),
        (1.6, |s, tx| arrow(s, tx, Key::Up)),
        (2.4, |s, tx| arrow(s, tx, Key::Up)),
        (3.4, |s, tx| arrow(s, tx, Key::Down)),
        (4.2, |s, tx| arrow(s, tx, Key::Down)),
        (5.0, |s, tx| arrow(s, tx, Key::Down)),
    ];
    for (at, run) in steps {
        tx.after(at, run);
    }
}
