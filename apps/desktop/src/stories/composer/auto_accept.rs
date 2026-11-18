//! The storybook's Auto-accept story (Composer › Auto-accept): a permission is asked, the card waits for
//! an answer, and with auto-accept on (A, or ⇧⌘A) it's answered at once and the composer shows the mark.
//! R replays from the card; variants start with it Off or On.

use opencode_kit::model::Pending;
use rune::Play;

use crate::App;
use crate::stories::{Trigger, replay};

pub const VARIANTS: &[(&str, usize)] = &[("Off", 0), ("On", 1)];
const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'a', label: "Toggle auto-accept", run: |s, tx| {
        tx.clear();
        s.toggle_auto_accept(tx);
    } },
];

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    s.settings.auto_accept = false;
    s.pending.clear();
    let _ = s.add_pending(Pending::Permission {
        id: "per_story_auto".into(),
        session: crate::stories::sid(s),
        action: "bash".into(),
        resources: vec!["bun run test".into()],
        message: String::new(),
        input: Vec::new(),
    });
    s.settings.auto_accept = s.variant == 1;
    if s.settings.auto_accept {
        s.auto_answer("per_story_auto");
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Composer/Auto-accept", KEYS).variants(VARIANTS),
]);
