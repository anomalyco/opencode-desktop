//! Storybook stage (`⌘⇧D`) for `Files/Markdown file`: a Markdown file rendered, as source, and
//! after following a link in it.

use rune::{El, Play, Tx};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};

const PROPOSAL: &str = "docs/proposal.md";

const VARIANTS: &[(&str, usize)] = &[("Rendered", 0), ("Source", 1), ("Link click", 2)];

const KEYS: &[Trigger] = &[
    replay(setup),
    Trigger {
        key: 's',
        label: "Toggle source",
        run: |s, _| {
            let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
            let path = s.active_file_tab().and_then(|(_, p)| p).unwrap_or_else(|| PROPOSAL.into());
            s.toggle_rendered(&sid, &path, &mut Tx::default());
        },
    },
    Trigger {
        key: 'l',
        label: "Follow the first-principles link",
        run: |s, _| follow(s),
    },
];

fn follow(s: &mut App) {
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    s.open_link(&sid, Some(PROPOSAL), "first-principles.md#the-rule", &mut Tx::default());
}

pub fn init(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    s.side = Default::default();
    s.files = Default::default();
    let mut tx = Tx::default();
    s.open_workspace_file(PROPOSAL, false, None, &mut tx);
    s.panel_sidebar.opened = false;
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    match *s.variant {
        1 => s.toggle_rendered(&sid, PROPOSAL, &mut tx),
        2 => follow(s),
        _ => {}
    }
}

fn setup(s: &mut App, _play: &mut Play<'_, App>) {
    init(s);
}

fn card(s: &App, p: &Pal) -> El {
    crate::panels::view::story_card(s, p, 680.0, 540.0)
}

rune_register::register!(crate::stories::STORY_LIST: Story = [Story {
    path: "Files/Markdown file",
    scene: |s| {
        init(s);
        stage(s, card);
    },
    knobs: &[],
    keys: KEYS,
    variants: VARIANTS,
}]);
