//! Storybook story for ⌘O, Quick Open (`Projects/Quick open (⌘O)`): recent projects ahead of
//! recent sessions, and a search that ranks projects first, on one server or two.

use opencode_kit::model::{Project, ProjectIcon};
use opencode_kit::servers::ServerKey;
use rune::{El, Play, TextEdit, Tx};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::switcher::{RemoteSeed, SWITCHER};

const VARIANTS: &[(&str, usize)] = &[("Recent", 0), ("Searched", 1), ("Two servers", 2), ("Two servers, searched", 3)];

const QUERY: &str = "kiln";

const KEYS: &[Trigger] = &[
    replay(setup),
    Trigger {
        key: 'c',
        label: "Clear or fill query",
        run: |s, tx| {
            let typed = s.kernel.slots.get(&SWITCHER).query().is_empty();
            type_query(s, if typed { QUERY } else { "" }, tx);
        },
    },
];

fn init(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    let mut tx = Tx::default();
    s.open_quick_open(&mut tx);
    if *s.fixture_run.variant >= 2 {
        crate::switcher::servers::stage_remotes(s, vec![devbox()]);
    }
    if *s.fixture_run.variant % 2 == 1 {
        type_query(s, QUERY, &mut tx);
    }
}

fn setup(s: &mut App, _: &mut Play<'_, App>) {
    init(s);
}

fn type_query(s: &mut App, q: &str, tx: &mut Tx) {
    let sw = s.kernel.slots.get_mut(&SWITCHER);
    sw.search = TextEdit::new(q);
    sw.search.focused = true;
    s.poll_switcher(tx, false);
}

/// A second server with two projects of its own, one sharing a folder name with this one's.
fn devbox() -> RemoteSeed {
    let project = |id: &str, dir: &str| Project { id: id.into(), name: String::new(), directory: dir.into(), active: 1_792_659_000_000, icon: ProjectIcon::default() };
    let projects = vec![project("devbox-nebula", "/home/demo/src/nebula"), project("devbox-kiln", "/home/demo/src/kiln")];
    RemoteSeed {
        server: ServerKey::new("https://devbox.example.ts.net"),
        tag: Some("devbox".into()),
        conn: opencode_kit::client::Server::anonymous("https://devbox.example.ts.net"),
        known: Vec::new(),
        indexed: Default::default(),
        used: projects.clone(),
        other: Vec::new(),
        projects,
    }
}

/// An empty stage: the window's own overlay draws the palette, as ⌘O does.
fn view(_: &App, _: &Pal) -> El {
    rune::col().w_full().h_full()
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Projects/Quick open (⌘O)",
        scene: |s| {
            init(s);
            stage(s, view);
        },
        knobs: &[],
        keys: KEYS,
        variants: VARIANTS,
    },
]);
