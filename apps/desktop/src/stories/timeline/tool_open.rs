//! The storybook's Timeline › Open tool step story: the session beside its side region, shut,
//! with the edit step on screen. E opens the edit step in Changes, F the read step in Files at
//! its lines, and X does what Esc does; a click on a step's row opens it as in the app. R sets
//! it up again. Opening is instant, so nothing plays on its own.

use opencode_kit::model::{Message, Part};
use rune::{El, Play, el, row};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};

const EDIT: &str = "call_edit_50000";
const READ: &str = "call_read_28000";

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'e', label: "Open the edit step", run: |s, tx| s.press_tool_step(EDIT, tx) },
    Trigger { key: 'f', label: "Open the read step", run: |s, tx| s.press_tool_step(READ, tx) },
    Trigger {
        key: 'x',
        label: "Esc",
        run: |s, tx| {
            s.dismiss_tool_step(tx);
        },
    },
];

fn setup(s: &mut App) {
    crate::stories::scenes::open_scene(s);
    s.finish_stream();
    *s.kernel.slots.get_mut(&crate::panels::SIDE) = Default::default();
    *s.kernel.slots.get_mut(&crate::review::REVIEW) = Default::default();
    *s.workspace_panes.tool_open = None;
    s.timeline_fx.expanded.insert(format!("c-{EDIT}"));
    s.timeline_fx.expanded.insert("c-call_grep_27000".into());
    // The timeline ends on the turn that made the edit, so both steps show at its foot.
    let sid = crate::stories::sid(s);
    if let Some(t) = s.transcripts.get_mut(&sid)
        && let Some(at) = t.messages.iter().position(|m| matches!(m, Message::Assistant(a) if a.parts.iter().any(|p| matches!(p, Part::Tool { id, .. } if id == EDIT))))
    {
        t.messages.truncate(at + 1);
    }
}

fn open(s: &mut App, _: &mut Play<App>) {
    setup(s);
}

fn view(s: &App, p: &Pal) -> El {
    row()
        .key("tool-open-story")
        .gap(12.0)
        .h(600.0)
        .child(crate::stories::session_view(s, p).w(560.0).max_w(560.0))
        .child(if s.side_open() { crate::panels::view::story_card(s, p, 560.0, 600.0) } else { el().key("side-panel").size(560.0, 600.0) })
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Timeline/Open tool step",
        scene: |s| {
            setup(s);
            stage(s, view);
        },
        knobs: &[],
        keys: KEYS,
        variants: &[],
    },
]);
