use super::*;

pub(crate) fn open_scene(s: &mut App) {
    let id = s.fixture["open"].as_str().map(str::to_owned).or_else(|| s.sessions.first().map(|x| x.id.clone()));
    if let Some(id) = id {
        s.route = Route::Session(id.clone());
    }
}

pub(crate) fn busy_scene(s: &mut App) {
    open_scene(s);
    s.finish_stream();
    if let Route::Session(id) = s.route.clone() {
        if let Some(Message::Assistant(a)) = s.transcripts.get_mut(&id).and_then(|t| t.messages.last_mut()) {
            a.completed = None;
        }
        s.running.insert(id);
    }
}

/// A named scene: `--snap SCENE` and `--list-scenes`.
pub(crate) type Scene = (&'static str, fn(&mut App));

/// The app's scenes in the order `--list-scenes` shows them. One not named here goes after these, by
/// module path.
const ORDER: &[&str] = &[
    "home",
    "no-server",
    "session",
    "streaming",
    "busy",
    "busy-deliver",
    "queue",
    "queue-3",
    "queue-question",
    "queue-question-3",
    "queue-question-morph",
    "draft",
    "settings",
    "settings-updates",
    "settings-sidebar-closed",
    "question",
    "question-morph",
    "question-long",
    "sidebar-closed",
    "status",
    "tab-badges",
    "demo",
];

pub(crate) fn all() -> Vec<Scene> {
    crate::stories::ordered(rune_register::collect(&SCENE_LIST), ORDER, |s| s.0)
}

rune_register::slice!(pub(crate) SCENE_LIST: Scene);

rune_register::register!(SCENE_LIST: Scene = [
    ("home", |_| {}),
    ("session", |s| {
        open_scene(s);
        s.finish_stream();
    }),
    ("streaming", |s| {
        open_scene(s);
        // Playback starts on the first poll.
        if let Some(id) = s.fixture["open"].as_str().map(str::to_owned) {
            s.start_stream_if_due(&id);
        }
    }),
    // The open session's turn still running, so Enter steers and ⌘⏎ queues.
    ("busy", busy_scene),
    // Busy, then the server takes the steer at 5.6 s and the queued prompts at 6.6 and
    // 7.6 s: with `scripts/steer-dock.json`, a steer and two queued prompts delivered.
    ("busy-deliver", |s| {
        busy_scene(s);
        let Route::Session(id) = s.route.clone() else { return };
        let next = |kind: &str| serde_json::json!({ "sessionID": id, "next": kind });
        s.playback = [(5.6, "session.inbox.delivered".into(), next("steer")), (6.6, "session.inbox.delivered".into(), next("queue")), (7.6, "session.inbox.delivered".into(), next("queue"))].into();
    }),
    ("draft", |s| s.route = Route::Draft),
    ("settings", |s| {
        open_scene(s);
        s.finish_stream();
        s.open_settings();
    }),
    // The storybook (⌘⇧D)'s stories are scenes too, under their paths ("Timeline/Send"):
    // `--playground PATH [--variant NAME]` opens one with its motion playing (`stories.rs`).
    ("sidebar-closed", |s| {
        open_scene(s);
        s.finish_stream();
        s.sidebar_open = false;
    }),
    ("status", |s| {
        open_scene(s);
        s.finish_stream();
        for e in s.fixture["status_events"].as_array().cloned().unwrap_or_default() {
            s.playback.push_back((e["t"].as_f64().unwrap_or(0.0), e["type"].as_str().unwrap_or("").into(), e["data"].clone()));
        }
    }),
]);
