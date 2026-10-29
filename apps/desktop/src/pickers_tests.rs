//! Choosing agent, model and effort in a live session: the keys work there, and the pick rides
//! on the next prompt only when it differs from what the session runs with.
use super::*;
use opencode_kit::client::Write;
use rune::{Mods, Runtime};

const SES: &str = "ses_fx0001aaaaaaaaaaaaaaaaaa";

/// The fixture's world (its models and sessions) on a live source whose writes land in a channel.
fn live() -> (Runtime<App>, std::sync::mpsc::Receiver<Write>) {
    let mut app = App::fixture_session();
    let writes = app.fake_writes();
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    (rt, writes)
}

fn press(rt: &mut Runtime<App>, key: Key, shift: bool) {
    let cmd = matches!(key, Key::Command(_));
    rt.key(KeyEvent { key, mods: Mods { cmd, shift, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
}

fn send(rt: &mut Runtime<App>, writes: &std::sync::mpsc::Receiver<Write>, text: &str) -> opencode_kit::client::Prompt {
    rt.update(|s, tx| {
        s.composer = rune::TextEdit::new(text);
        s.send(tx);
    });
    match writes.try_recv() {
        Ok(Write::Send { prompt, .. }) => prompt,
        _ => panic!("no prompt"),
    }
}

#[test]
fn the_keys_and_toolbar_choose_in_a_live_session() {
    let (mut rt, _) = live();
    assert_eq!(rt.state.route, Route::Session(SES.into()));
    press(&mut rt, Key::Command('\''), false);
    assert_eq!(rt.state.pickers.open, Some(Kind::Model), "⌘' opens the model list");
    press(&mut rt, Key::Escape, false);
    let (x, y) = rt.find("model").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.pickers.open, Some(Kind::Model), "the toolbar opens it too");
    press(&mut rt, Key::Escape, false);

    // high → default → low, as Desktop's cycle walks: the last effort wraps to the default.
    press(&mut rt, Key::Command('d'), true);
    assert_eq!(rt.state.selection().model.variant, "");
    press(&mut rt, Key::Command('d'), true);
    assert_eq!(rt.state.selection().model.variant, "low");
    assert_eq!(rt.state.pickers.open, None, "a cycle doesn't leave the list open");

    press(&mut rt, Key::Command('.'), false);
    assert_eq!(rt.state.selection().agent, "plan");
    press(&mut rt, Key::Command('.'), true);
    assert_eq!(rt.state.selection().agent, "build");
}

#[test]
fn a_pick_rides_on_the_next_prompt_and_nothing_extra_without_one() {
    let (mut rt, writes) = live();
    let plain = send(&mut rt, &writes, "hi");
    assert_eq!((&plain.agent, &plain.model), (&None, &None), "no pick, nothing to switch");
    assert!(plain.body().get("metadata").is_none_or(|m| m.get("agent").is_none() && m.get("model").is_none()));

    press(&mut rt, Key::Command('d'), true); // high → default
    let prompt = send(&mut rt, &writes, "go");
    assert_eq!(prompt.agent, None, "the agent didn't change");
    let model = prompt.model.clone().unwrap();
    assert_eq!((model.provider.as_str(), model.id.as_str(), model.variant.as_str()), ("anthropic", "claude-opus-5-5", ""));
    assert_eq!(prompt.body()["metadata"]["model"], serde_json::json!({"providerID": "anthropic", "modelID": "claude-opus-5-5"}));

    // The pick stays until the server says the session runs with it.
    assert_eq!(rt.state.selection().model.variant, "");
    rt.update(|s, _| {
        s.apply_event("session.model.selected", &serde_json::json!({"sessionID": SES, "model": {"id": "claude-opus-5-5", "providerID": "anthropic"}}));
    });
    assert_eq!(rt.state.session(SES).unwrap().model.variant, "");
    let after = send(&mut rt, &writes, "again");
    assert_eq!((after.agent, after.model), (None, None), "the session runs with it now");
}

#[test]
fn an_agent_switch_sends_its_model_and_a_queued_prompt_keeps_it() {
    let (mut rt, writes) = live();
    rt.update(|s, _| {
        s.apply_event("session.execution.started", &serde_json::json!({"sessionID": SES}));
    });
    assert!(rt.state.busy(SES));
    press(&mut rt, Key::Command('.'), false);
    rt.update(|s, tx| {
        s.composer = rune::TextEdit::new("after this");
        s.submit(true, tx);
    });
    let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("no queued prompt") };
    assert_eq!(prompt.delivery, Some(opencode_kit::model::Delivery::Queue));
    assert_eq!(prompt.agent.as_deref(), Some("plan"));
    assert_eq!(prompt.model.as_ref().map(|m| m.id.as_str()), Some("claude-opus-5-5"), "an agent switch can move the model, so it goes along");
}

#[test]
fn each_agent_brings_back_its_model_and_each_session_keeps_its_own() {
    let (mut rt, _) = live();
    let pick = |rt: &mut Runtime<App>, kind: Kind, key: &str| {
        rt.update(|s, _| {
            s.open_picker(kind);
            s.choose_picker(key);
        });
    };
    pick(&mut rt, Kind::Model, "anthropic:claude-sonnet-5-5");
    pick(&mut rt, Kind::Agent, "plan");
    assert_eq!(rt.state.selection().model.id, "claude-sonnet-5-5", "an agent with no model of its own keeps the current one");
    pick(&mut rt, Kind::Model, "openai:gpt-6-astra");
    pick(&mut rt, Kind::Agent, "build");
    assert_eq!(rt.state.selection().model.id, "claude-sonnet-5-5");
    pick(&mut rt, Kind::Agent, "plan");
    assert_eq!(rt.state.selection().model.id, "gpt-6-astra");

    // An effort comes back with its model.
    pick(&mut rt, Kind::Effort, "low");
    pick(&mut rt, Kind::Model, "anthropic:claude-sonnet-5-5");
    assert_eq!(rt.state.selection().model.variant, "");
    pick(&mut rt, Kind::Model, "openai:gpt-6-astra");
    assert_eq!(rt.state.selection().model.variant, "low");

    rt.state.route = Route::Session("ses_fx0003aaaaaaaaaaaaaaaaaa".into());
    let other = rt.state.selection();
    assert_eq!((other.agent.as_str(), other.model.id.as_str(), other.model.variant.as_str()), ("build", "claude-opus-5-5", "high"));
}

#[test]
fn a_new_session_starts_with_what_the_toolbar_shows() {
    let (mut rt, writes) = live();
    rt.update(|s, _| s.route = Route::Draft);
    rt.update(|s, tx| {
        s.composer = rune::TextEdit::new("start");
        s.send(tx);
    });
    let Ok(Write::Start { prompt, .. }) = writes.try_recv() else { panic!("no start") };
    assert_eq!(prompt.agent.as_deref(), Some("build"));
    assert_eq!(prompt.model.map(|m| m.id), Some("claude-opus-5-5".into()));
}

#[test]
fn the_default_list_shows_each_familys_newest_and_search_finds_the_rest() {
    let day = 24.0 * 3600.0 * 1000.0;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64;
    let models = serde_json::json!([
        { "id": "claude-a", "providerID": "anthropic", "name": "Claude A", "family": "claude", "time": { "released": now - 300.0 * day } },
        { "id": "claude-b", "providerID": "anthropic", "name": "Claude B (latest)", "family": "claude", "time": { "released": now - 20.0 * day } },
        { "id": "claude-c", "providerID": "anthropic", "name": "Claude C", "family": "claude", "time": { "released": now - 40.0 * day } },
        { "id": "old", "providerID": "anthropic", "name": "Old", "family": "claude", "status": "deprecated", "time": { "released": now - day } },
        { "id": "local", "providerID": "ollama", "name": "Local" },
    ]);
    let mut app = App::empty();
    app.pickers.load_live(&models, &serde_json::json!([{ "id": "build", "model": { "id": "claude-c", "providerID": "anthropic" } }]));
    assert!(app.pickers.models.iter().all(|m| m.id != "old"), "deprecated models are left out");
    assert_eq!(app.pickers.agent_models["build"].id, "claude-c");
    app.pickers.open = Some(Kind::Model);
    let shown: Vec<String> = app.picker_items().into_iter().map(|i| i.label).collect();
    // A new session's model is the agent's, and the current model always shows.
    assert_eq!(shown, ["Claude B", "Claude C", "Local"]);
    app.pickers.search = rune::TextEdit::new("claude a");
    assert_eq!(app.picker_items().len(), 1);
}
