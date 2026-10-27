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
        ("open-before", |s| open_story::scene(s, true)),
        ("open-after", |s| open_story::scene(s, false)),
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
        // The storybook (⌘⇧D) on a story: `story-morph` is the dock opening and closing on a
        // loop (pick the curve with `--preset NAME`, a Question dock preset in `knobs.rs`);
        // `story` alone opens the first. Scripts replay a story with its trigger keys.
        ("story", |s| stories::scene(s, "Composer")),
        ("story-morph", |s| stories::scene(s, "Morph")),
        ("story-long", |s| stories::scene_variant(s, "Question", crate::stories::QUESTION, 3)),
        ("story-typing", |s| stories::scene_variant(s, "Question", crate::stories::QUESTION, 4)),
        ("story-custom", |s| stories::scene_variant(s, "Custom answer", crate::stories::CUSTOM_ANSWER, 0)),
        ("story-custom-long", |s| stories::scene_variant(s, "Custom answer", crate::stories::CUSTOM_ANSWER, 1)),
        ("story-custom-left", |s| stories::scene_variant(s, "Custom answer", crate::stories::CUSTOM_ANSWER, 2)),
        ("story-pending", |s| stories::scene_variant(s, "Composer", crate::stories::COMPOSER, 2)),
        ("story-glow", |s| stories::scene(s, "Arrival glow")),
        ("story-send", |s| stories::scene(s, "Send")),
        ("story-stream", |s| stories::scene(s, "Streaming text")),
        ("story-tools", |s| stories::scene(s, "Tool rows")),
        // Question answers, one scene per variant: one, several, multi-select, typed, dismissed.
        ("story-answers-one", |s| crate::question_stories::scene(s, 0)),
        ("story-answers-several", |s| crate::question_stories::scene(s, 1)),
        ("story-answers-multi", |s| crate::question_stories::scene(s, 2)),
        ("story-answers-typed", |s| crate::question_stories::scene(s, 3)),
        ("story-answers-dismissed", |s| crate::question_stories::scene(s, 4)),
        ("story-working", |s| stories::scene(s, "Working")),
        ("story-steer", |s| stories::scene(s, "Steer and Pending")),
        ("story-queue", |s| stories::scene(s, "Queue dock")),
        ("story-subagent", |s| stories::scene(s, "Subagent")),
        ("story-images", |s| stories::scene(s, "Images")),
        ("story-jump", |s| stories::scene(s, "Jump to latest")),
        ("story-actions", |s| stories::scene(s, "Turn actions")),
        ("story-dictation", |s| stories::scene(s, "Dictation")),
        // Three agents' timeline rows in each ⌘⇧K Subagent rows style, light and dark.
        ("story-rows-today", |s| agent_stories::scene(s, 0, false)),
        ("story-rows-status", |s| agent_stories::scene(s, 1, false)),
        ("story-rows-expanding", |s| agent_stories::scene(s, 2, false)),
        ("story-rows-grouped", |s| agent_stories::scene(s, 3, false)),
        ("story-rows-today-dark", |s| agent_stories::scene(s, 0, true)),
        ("story-rows-status-dark", |s| agent_stories::scene(s, 1, true)),
        ("story-rows-expanding-dark", |s| agent_stories::scene(s, 2, true)),
        ("story-rows-grouped-dark", |s| agent_stories::scene(s, 3, true)),
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
