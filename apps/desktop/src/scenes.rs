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

pub(crate) const SCENES: &[(&str, fn(&mut App))] = &[
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
        // Busy with prompts queued above the composer or above the question dock;
        // `queue-question-morph` answers the question at 1.5 s and asks again at 2.5 s.
        ("queue", |s| queue::scene(s, 1, false)),
        ("queue-3", |s| queue::scene(s, 3, false)),
        ("queue-question", |s| queue::scene(s, 1, true)),
        ("queue-question-3", |s| queue::scene(s, 3, true)),
        ("queue-question-morph", |s| {
            queue::scene(s, 1, true);
            let q = s.fixture["question"].clone();
            let replied = serde_json::json!({ "id": q["id"], "sessionID": q["sessionID"] });
            s.playback = [(1.5, "form.replied".into(), replied), (2.5, "form.created".into(), q)].into();
        }),
        ("draft", |s| s.route = Route::Draft),
        ("settings", |s| {
            open_scene(s);
            s.finish_stream();
            s.open_settings();
        }),
        ("question", |s| {
            open_scene(s);
            s.finish_stream();
            let q = s.fixture["question"].clone();
            s.apply_event("form.created", &q);
        }),
        // Answered elsewhere at 1.5 s, asked again at 2.5 s: the morph both ways.
        ("question-morph", |s| {
            open_scene(s);
            s.finish_stream();
            let q = s.fixture["question"].clone();
            s.apply_event("form.created", &q);
            let replied = serde_json::json!({ "id": q["id"], "sessionID": q["sessionID"] });
            s.playback = [(1.5, "form.replied".into(), replied), (2.5, "form.created".into(), q)].into();
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
];
