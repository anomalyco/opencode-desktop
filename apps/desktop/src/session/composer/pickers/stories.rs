//! The storybook's Pickers story (Composer › Pickers): the open session's agent, model and
//! effort in the toolbar. Its variants open the model list or show a pick the next prompt will
//! switch the session to, Recent (the current model first) and Favorites leading it, a search
//! that mistypes Gemini, or one that finds nothing. M opens and closes the list, E cycles the
//! effort (⇧⌘D), A the agent (⌘.), F stars the lit model (⌘F), T types a typo, R replays.

use crate::pickers::PickerHost as _;
use rune::{Play, TextEdit};

use crate::App;
use crate::pickers::Kind;
use crate::stories::{Trigger, replay};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'm', label: "Open or close the model list", run: |s, _| s.open_picker(Kind::Model) },
    Trigger { key: 'e', label: "Next effort", run: |s, _| cycle(s, Kind::Effort) },
    Trigger { key: 'a', label: "Next agent", run: |s, _| cycle(s, Kind::Agent) },
    Trigger { key: 'f', label: "Star or unstar the lit model", run: |s, _| {
        if let Some(key) = s.picker_active() { s.toggle_favorite(&key); }
    } },
    Trigger { key: 't', label: "Search a typo (gemiin)", run: |s, _| search(s, "gemiin") },
];

pub const VARIANTS: &[(&str, usize)] = &[
    ("Model list", 0),
    ("Picked", 1),
    ("Recent and Favorites", 2),
    ("A typo in the search", 3),
    ("An empty search", 4),
];

/// The model list open on `query`, as if typed.
fn search(s: &mut App, query: &str) {
    if s.composer_ext.pickers.open().is_some() { s.close_picker(false); }
    s.open_picker(Kind::Model);
    s.composer_ext.pickers.search = TextEdit::new(query);
    s.composer_ext.pickers.search.focused = true;
}

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    s.settings.show_agent = true;
    if s.fixture_run.variant == 1 {
        for (kind, key) in [(Kind::Agent, "plan"), (Kind::Model, "openai:gpt-6-astra"), (Kind::Effort, "low")] {
            s.open_picker(kind);
            s.choose_picker(key);
        }
        s.composer.focused = false;
    } else if s.fixture_run.variant == 2 {
        s.composer_ext.pickers.prefs = Default::default();
        for key in ["google:gemini-4-argon", "openai:gpt-6-astra", "anthropic:claude-opus-5-5"] {
            s.open_picker(Kind::Model);
            s.choose_picker(key);
        }
        s.composer_ext.pickers.prefs.toggle_favorite("openai:gpt-6-astra");
        s.open_picker(Kind::Model);
    } else if s.fixture_run.variant == 3 {
        search(s, "gemiin");
    } else if s.fixture_run.variant == 4 {
        search(s, "zzzz");
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
