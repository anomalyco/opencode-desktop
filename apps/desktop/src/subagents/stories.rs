//! A finished subagent's page: its timeline, and the composer that answers it.

use rune::Tx;

use crate::fixture_ids::CHILD;
use crate::stories::{Story, Trigger};
use crate::{App, Route};

/// The fixture's finished reviewer, opened from its parent.
pub fn open_child(s: &mut App) {
    crate::scenes::open_scene(s);
    s.finish_stream();
    s.route = Route::here(CHILD.into());
}

const KEYS: &[Trigger] = &[Trigger { key: 'w', label: "It works on a reply, or finishes", run: |s, tx| work(s, tx) }];

fn work(s: &mut App, tx: &mut Tx) {
    tx.instant();
    if !s.running.remove(CHILD) {
        s.running.insert(CHILD.into());
    }
}

/// Side tabs with the open session's subagent running and two more tabs working, so the
/// sidebar's spinners, the subagent capsule's and the card's sit on one screen.
fn busy_glyphs(s: &mut App) {
    crate::scenes::open_scene(s);
    s.finish_stream();
    crate::scenes::side_tabs(s);
    s.running.extend([CHILD, crate::fixture_ids::THIRD, crate::fixture_ids::SIXTH].map(String::from));
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("subagent", open_child), ("busy-glyphs", busy_glyphs)]);

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Composer/Subagent reply", scene: |s| {
        open_child(s);
        crate::stories::stage(s, crate::stories::composer_view);
    }, knobs: &[], keys: KEYS, variants: &[] },
]);
