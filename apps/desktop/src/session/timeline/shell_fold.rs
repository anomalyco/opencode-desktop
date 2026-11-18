//! Kit's capture of a tool group that opened out of sight (p277): a reply whose first part is
//! one shell call, then a long answer, the session still working. `--scene shell-fold`.

use crate::App;

/// A turn of one shell call and a long answer, the next step still thinking.
pub fn add_shell_fold(app: &mut App, sid: &str) {
    if let Some(t) = app.transcripts.get_mut(sid) {
        opencode_feature_timeline_extras::shell_fold::apply(t, &mut app.running, sid);
    }
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("shell-fold", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = (*s.route).clone() {
            add_shell_fold(s, &id);
        }
    }),
]);
