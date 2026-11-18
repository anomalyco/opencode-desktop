//! The storybook's Timeline › Fade into composer story (p248): the timeline runs down under the
//! subagent capsule and fades out into the composer's top edge. R starts a subagent and reads a
//! page up, so the reply passes under the capsule; P starts or finishes the subagent; T makes the
//! composer four lines tall and back; B goes back to the end, where the last line stands clear.

use rune::{Play, TextEdit};
use rune_knobs::Group;
use serde_json::json;

use crate::App;
use crate::stories::fixture_ids::CHILD;
use crate::knobs::{CAPSULE_FILL, COMPOSER_FADE};
use crate::stories::{Story, Trigger, emit, replay, sid};

const KNOBS: &[Group] = &[Group::new("Composer fade", "stage", &[COMPOSER_FADE, CAPSULE_FILL])];

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'p', label: "Start or finish the subagent", run: toggle },
    Trigger { key: 't', label: "Tall composer or back", run: |s, _| tall(s) },
    Trigger { key: 'b', label: "Back to latest", run: |s, tx| tx.scroll_to_bottom(scroll(s)) },
];

fn scroll(s: &App) -> String {
    format!("scroll-{}", sid(s))
}

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.running.remove(CHILD);
    emit(s, "session.execution.started", json!({ "sessionID": CHILD }), tx);
    tx.after(0.6, |s, tx| tx.scroll_page(scroll(s), -1));
}

fn toggle(s: &mut App, tx: &mut Play<App>) {
    let ty = if s.running.contains(CHILD) { "session.execution.succeeded" } else { "session.execution.started" };
    emit(s, ty, json!({ "sessionID": CHILD }), tx);
}

fn tall(s: &mut App) {
    let text = if s.composer.text.is_empty() { "A longer prompt\nthat runs\nonto four\nlines" } else { "" };
    *s.composer = TextEdit::new(text);
}

rune_register::register!(crate::stories::STORY_LIST: Story = [Story::session("Timeline/Fade into composer", KEYS).knobs(KNOBS)]);
