//! Storybook stages (`⌘⇧D`) for `Files/Pane` and `Files/Quick open (⌘P)`.

use rune::{El, Play, TextEdit};

use super::data::OPEN_TAB_KEY;
use crate::App;
use crate::palette::Pal;
use crate::panels::SessionPane;
use crate::stories::{Story, Trigger, replay, stage};

const FILES_VARIANTS: &[(&str, usize)] = &[
    ("Open file", 0),
    ("In-file search", 1),
    ("Open file launcher", 2),
];

const PALETTE_VARIANTS: &[(&str, usize)] = &[
    ("Recent and changed", 0),
    ("Query with :line jump", 1),
];

const FILES_KEYS: &[Trigger] = &[
    replay(setup_files),
    Trigger {
        key: 't',
        label: "Toggle file tree",
        run: |s, _| {
            s.workspace_panes.panel_sidebar.opened = !s.workspace_panes.panel_sidebar.opened;
        },
    },
];

const PALETTE_KEYS: &[Trigger] = &[replay(setup_palette)];

pub fn init_files(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    s.workspace_panes.side = Default::default();
    s.workspace_panes.files = Default::default();
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    s.ensure_session_review(&sid);
    let mut tx = rune::Tx::default();
    match *s.fixture_run.variant {
        0 => {
            s.open_workspace_file("src/net/stream.ts", false, Some(12), &mut tx);
        }
        1 => {
            s.open_workspace_file("src/net/backoff.ts", false, None, &mut tx);
            if let Some(ws) = s.workspace_panes.files.sessions.get_mut(&sid)
                && let Some(doc) = ws.docs.get_mut("src/net/backoff.ts")
            {
                doc.set_find_open(true);
                doc.find_input = TextEdit::new("healthySince");
                doc.sync_find_query();
            }
        }
        _ => {
            s.ensure_workspace_files(&sid);
            s.open_side_tab(OPEN_TAB_KEY, Some(SessionPane::Side), true, false, &mut tx);
        }
    }
}

fn setup_files(s: &mut App, _play: &mut Play<'_, App>) {
    init_files(s);
}

pub fn init_palette(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    s.workspace_panes.files = Default::default();
    let mut tx = rune::Tx::default();
    s.open_file_search(&mut tx);
    if s.fixture_run.variant == 1 {
        let sw = s.kernel.slots.get_mut(&crate::switcher::SWITCHER);
        sw.search = TextEdit::new("stream.ts:15");
        sw.search.focused = true;
        s.poll_switcher(&mut tx, false);
    }
}

fn setup_palette(s: &mut App, _play: &mut Play<'_, App>) {
    init_palette(s);
}

fn view_files_card(s: &App, p: &Pal) -> El {
    crate::panels::view::story_card(s, p, 680.0, 540.0)
}

fn view_palette_card(s: &App, p: &Pal) -> El {
    if !s.kernel.slots.get(&crate::switcher::SWITCHER).is_open() {
        return rune::col().w(480.0);
    }
    crate::switcher::dialog(s, p, 480.0, 480.0)
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Files/Pane",
        scene: |s| {
            init_files(s);
            stage(s, view_files_card);
        },
        knobs: &[],
        keys: FILES_KEYS,
        variants: FILES_VARIANTS,
    },
    Story {
        path: "Files/Quick open (⌘P)",
        scene: |s| {
            init_palette(s);
            stage(s, view_palette_card);
        },
        knobs: &[],
        keys: PALETTE_KEYS,
        variants: PALETTE_VARIANTS,
    },
]);
