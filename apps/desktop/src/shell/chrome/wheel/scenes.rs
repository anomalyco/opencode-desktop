//! Regions a wheel passes over that aren't the timeline: a reply ending in a
//! code block, or a table, wider than the timeline (it scrolls sideways), and the `/` command list
//! long enough to scroll. `--scene scroll-wide-code`, `scroll-wide-table`, `scroll-commands`.

use opencode_kit::model::{Assistant, Message, Part};
use rune::{TextEdit, Tx};
use serde_json::json;

use crate::App;

/// A question, then a reply ending in a code block (`table` false) or a table wider than the timeline.
fn add_wide(app: &mut App, sid: &str, table: bool) {
    let Some(t) = app.transcripts.get_mut(sid) else { return };
    let code = (1..=14)
        .map(|n| format!("let stage_{n:02} = pipeline.stage(\"build-{n:02}\").timeout(Duration::from_secs({n})).retry(3).on_error(|e| warn!(\"stage {n} failed: {{e}}\")); // keeps the cache warm for the next stage"))
        .collect::<Vec<_>>()
        .join("\n");
    let head = (1..=12).map(|c| format!("Column {c}")).collect::<Vec<_>>().join(" | ");
    let rule = ["---"; 12].join(" | ");
    let rows = (1..=6)
        .map(|r| format!("| {} |", (1..=12).map(|c| format!("row {r}, value {c}")).collect::<Vec<_>>().join(" | ")))
        .collect::<Vec<_>>()
        .join("\n");
    let text = if table { format!("Its stages:\n\n| {head} |\n| {rule} |\n{rows}") } else { format!("The pipeline:\n\n```rust\n{code}\n```") };
    t.messages.push(Message::User { id: "msg_zzscrollwide0".into(), text: "Show me the pipeline and its stages".into() });
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzscrollwide1".into(), completed: Some(1), parts: vec![Part::Text { text }], ..Default::default() }));
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("scroll-wide-code", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = (*s.route).clone() {
            add_wide(s, &id, false);
        }
    }),
    ("scroll-wide-table", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = (*s.route).clone() {
            add_wide(s, &id, true);
        }
    }),
    // The organized sidebar closed and peeking over the session: its list is raised over the timeline.
    ("scroll-sidebar-peek", |s| {
        crate::open_scene(s);
        s.finish_stream();
        crate::scenes::side_tabs(s);
        s.settings.sidebar_organize = crate::settings::Organize::Projects;
        if let Some(id) = s.used_projects().first().map(|p| p.id.clone()) {
            let key = crate::view::sidebar_organized::project_key(s, &id);
            s.project_state.organize_expanded.insert(key);
        }
        *s.sidebar_open = false;
        s.layout.peek = crate::layout_state::SidebarPeekState::Peeking;
    }),
    ("scroll-commands", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let list: Vec<_> = (1..=40).map(|n| json!({ "name": format!("task-{n:02}"), "description": format!("Run task {n} of the build") })).collect();
        s.kernel.slots.get_mut(&crate::add_menu::ADD_MENU).slash.commands = crate::slash::commands(&json!(list));
        *s.composer = TextEdit::new("");
        s.composer.focused = true;
        s.composer.insert("/");
        s.suggest_input(&mut Tx::default());
    }),
]);
