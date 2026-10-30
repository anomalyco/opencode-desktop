//! The storybook's Commands story (Composer › Commands): the one command list as `/` shows it
//! over the composer, as ⇧⌘P shows it alone, and in ⌘K's search. R replays; S, P and E switch
//! between them.

use rune::{El, Play, TextEdit, Tx, col};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};

const VARIANTS: &[(&str, usize)] = &[("Slash", 0), ("Commands", 1), ("Search", 2)];

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 's', label: "Type / in the composer", run: |s, tx| show(s, 0, tx) },
    Trigger { key: 'p', label: "⇧⌘P", run: |s, tx| show(s, 1, tx) },
    Trigger { key: 'e', label: "⌘K, typed \"toggle\"", run: |s, tx| show(s, 2, tx) },
];

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    show(s, s.variant, tx);
}

fn show(s: &mut App, variant: usize, tx: &mut Tx) {
    s.variant = variant;
    s.close_switcher(false);
    s.composer = TextEdit::new("");
    tx.instant();
    match variant {
        0 => {
            s.composer.focused = true;
            s.composer.insert("/");
            s.add.slash.closed = None;
        }
        1 => s.open_command_search(tx),
        _ => {
            s.open_switcher();
            s.switcher.search = TextEdit::new("toggle");
            s.switcher.search.focused = true;
        }
    }
}

fn view(s: &App, p: &Pal) -> El {
    if s.switcher.open {
        return crate::switcher::dialog(s, p, 640.0, 480.0);
    }
    col().key("story-commands").w_full().max_w(820.0).pt(340.0).child(crate::stories::composer_view(s, p))
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Composer/Commands", scene: |s| {
        s.finish_stream();
        show(s, s.variant, &mut Tx::default());
        stage(s, view);
    }, knobs: &[], keys: KEYS, variants: VARIANTS },
]);
