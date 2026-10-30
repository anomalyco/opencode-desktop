//! Storybook stories for opening and choosing projects (`Projects/Open project`).

use rune::{El, Play, Tx, col, row};

use crate::palette::Pal;
use crate::pickers::Kind;
use crate::stories::{Story, Trigger, replay, stage};
use crate::{App, Route, view};

pub const VARIANTS: &[(&str, usize)] = &[("Draft picker", 0), ("Opened session", 1)];

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger {
        key: 'o',
        label: "Open orbit folder",
        run: |s, tx| s.open_project_dir("/Users/demo/code/orbit", true, tx),
    },
    Trigger {
        key: 'p',
        label: "Toggle draft picker",
        run: |s, tx| {
            s.route = Route::Draft;
            s.open_picker(Kind::Project);
            tx.instant();
        },
    },
];

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    if s.variant == 1 {
        s.open_project_dir("/Users/demo/code/orbit", true, &mut Tx::default());
    } else {
        s.route = Route::Draft;
        s.open_picker(Kind::Project);
    }
}

fn story_view(s: &App, p: &Pal) -> El {
    let page = match &s.route {
        Route::Draft => view::draft_page(s, p, 720.0),
        Route::Session(id) => view::session_page(s, p, id),
        Route::Home => col().w_full().h_full(),
    };
    row()
        .key("story-open-project")
        .w_full()
        .max_w(980.0)
        .h_full()
        .fill(p.bg_deep)
        .radius(10.0)
        .clip_soft(0.0)
        .child(view::sidebar(s, p))
        .child(
            col()
                .key("story-panel")
                .grow(1.0)
                .shrink()
                .ml(8.0)
                .my(8.0)
                .mr(8.0)
                .radius(10.0)
                .fill(p.bg_base)
                .border(0.5, p.ring)
                .clip_soft(0.0)
                .child(page),
        )
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Projects/Open project", scene: |s| stage(s, story_view), knobs: &[], keys: KEYS, variants: VARIANTS },
]);
