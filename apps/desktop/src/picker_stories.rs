//! The storybook's Pickers story (Composer › Pickers): the open session's agent, model and
//! effort in the toolbar. Its variants open the model list or show a pick the next prompt will
//! switch the session to. M opens and closes the list, E cycles the effort (⇧⌘D), A the agent
//! (⌘.), R replays.

use rune::Play;

use crate::App;
use crate::pickers::Kind;
use crate::stories::{Trigger, replay};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'm', label: "Open or close the model list", run: |s, _| s.open_picker(Kind::Model) },
    Trigger { key: 'e', label: "Next effort", run: |s, _| cycle(s, Kind::Effort) },
    Trigger { key: 'a', label: "Next agent", run: |s, _| cycle(s, Kind::Agent) },
];

pub const VARIANTS: &[(&str, usize)] = &[("Model list", 0), ("Picked", 1)];

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    s.settings.show_agent = true;
    if s.variant == 1 {
        for (kind, key) in [(Kind::Agent, "plan"), (Kind::Model, "openai:gpt-6-astra"), (Kind::Effort, "low")] {
            s.open_picker(kind);
            s.choose_picker(key);
        }
        s.composer.focused = false;
    } else {
        s.open_picker(Kind::Model);
    }
}

fn cycle(s: &mut App, kind: Kind) {
    s.close_picker(false);
    let key = match kind { Kind::Agent => '.', _ => 'd' };
    let mods = rune::Mods { cmd: true, shift: kind == Kind::Effort, ..Default::default() };
    s.picker_key(&rune::KeyEvent { key: rune::Key::Command(key), mods }, &mut rune::Tx::default());
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::composer("Composer/Pickers", KEYS).variants(VARIANTS),
]);
