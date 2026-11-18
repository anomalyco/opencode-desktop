//! Kit's capture of a short tool group shutting (t1107): one thought and one shell above five
//! prompts sent while it ran, and the next step still thinking. `--scene short-group`.

use crate::App;

/// Kit's capture: a turn of one thought and one shell, five prompts queued under it, and the
/// next step still thinking.
pub fn add_short_group(app: &mut App, sid: &str) {
    if let Some(t) = app.transcripts.get_mut(sid) {
        opencode_feature_timeline_extras::short_group::apply(t, &mut app.running, sid);
    }
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("short-group", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = (*s.route).clone() {
            add_short_group(s, &id);
        }
    }),
]);
