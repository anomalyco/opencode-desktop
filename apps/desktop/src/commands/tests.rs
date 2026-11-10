use std::collections::HashSet;

use opencode_kit::client::Write;
use rune::{Key, KeyEvent, Mods, Runtime};

use crate::mentions::tests::{key, runtime, tap, typed};
use crate::switcher::SwitcherMode;
use crate::{App, Route};

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

/// The built-in plugins running: Terminal's commands are the Luau terminal plugin's (1b9a4d22).
fn with_plugins(rt: &mut Runtime<App>) {
    rt.update(|s, _| {
        crate::plugin_host::setup(s, std::sync::Arc::new(|| {}));
        crate::plugin_host::contrib_sync_luau(s);
    });
    rt.frame(1.0 / 60.0);
}

fn cmd_shift_p(rt: &mut Runtime<App>) {
    press(rt, Key::Command('p'), Mods { cmd: true, shift: true, ..Default::default() });
}

#[test]
fn ids_and_slashes_are_unique_and_desktops_are_there() {
    let mut rt = runtime();
    with_plugins(&mut rt);
    // /agent is offered while the agent shows, as Desktop's (c86d13b7).
    rt.state.settings.show_agent = true;
    let all = rt.state.commands();
    let ids: HashSet<&str> = all.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), all.len(), "one id each");
    let slashes: Vec<&str> = all.iter().filter_map(|c| c.slash).collect();
    assert_eq!(slashes.iter().collect::<HashSet<_>>().len(), slashes.len(), "one /name each");
    // /rename left Desktop's list (4a327249); /open and /terminal are the file and terminal plugins'.
    for want in ["new", "model", "agent", "compact", "terminal", "open", "export"] {
        assert!(slashes.contains(&want), "/{want} is listed in a session");
    }
    // Review, Files and Terminal are Luau plugins, whose commands are `<plugin>.<id>`; Toggle
    // sidebar isn't offered in Top tabs (42172690).
    let missing: Vec<_> = ["side", "settings", "zoom-in", "copy-session", "copy-project", "focus", "find", "review.open", "file.browse", "terminal.toggle", "browser"].into_iter().filter(|w| !ids.contains(w)).collect();
    assert!(missing.is_empty(), "missing commands: {missing:?} of {ids:?}");
}

#[test]
fn slash_runs_an_app_command_at_once() {
    let mut rt = runtime();
    tap(&mut rt, "prompt");
    typed(&mut rt, "/new");
    assert_eq!(rt.state.slash_active().as_deref(), Some("builtin.new"));
    key(&mut rt, Key::Enter);
    assert!(matches!(rt.state.route, Route::Draft(_)));
    assert!(rt.state.composer.text.is_empty());
}

#[test]
fn compact_and_export_go_to_the_server() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    let writes = app.fake_writes();
    app.route = Route::here(crate::fixture_ids::MAIN.into());
    app.composer = rune::TextEdit::new("/compact");
    app.send(&mut rune::Tx::default());
    assert!(matches!(writes.try_recv(), Ok(Write::Compact(s)) if s == crate::fixture_ids::MAIN));
    assert!(app.composer.text.is_empty());
    app.run_command("export", &mut rune::Tx::default());
    assert!(matches!(writes.try_recv(), Ok(Write::Export(s)) if s == crate::fixture_ids::MAIN));
}

#[test]
fn session_commands_leave_home() {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    app.route = Route::Home;
    let ids: Vec<&str> = app.commands().iter().map(|c| c.id).collect();
    for gone in ["compact", "export", "rename", "copy-session", "focus"] {
        assert!(!ids.contains(&gone), "{gone} needs a session");
    }
    assert!(ids.contains(&"new") && ids.contains(&"settings"));
}

#[test]
fn cmd_shift_p_searches_commands_alone() {
    let mut rt = runtime();
    rt.update(|s, tx| s.run_command("commands", tx));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.switcher.open && rt.state.switcher.mode == SwitcherMode::Commands);
    let entries = rt.state.switcher_entries();
    assert!(entries.len() > 20, "{} commands", entries.len());
    assert!(entries.iter().all(|e| e.group == "Commands" && e.key.starts_with("command:")));
    assert!(!entries.iter().any(|e| e.key == "command:commands"), "it doesn't list itself");
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("sw-command:new")));
    typed(&mut rt, "zoom");
    let keys: Vec<String> = rt.state.switcher_entries().into_iter().map(|e| e.key).collect();
    assert_eq!(keys, ["command:zoom-in", "command:zoom-out"]);
    // ⇧⌘P is the palette's own second chord (`DEFAULT_PALETTE_KEYBIND`).
    cmd_shift_p(&mut rt);
    assert!(rt.state.switcher.open && rt.state.switcher.mode == SwitcherMode::All, "⇧⌘P opens Search everything");
}

#[test]
fn palette_runs_commands_with_their_keys_shown() {
    let mut rt = runtime();
    with_plugins(&mut rt);
    press(&mut rt, Key::Command('k'), Mods { cmd: true, ..Default::default() });
    typed(&mut rt, "copy session");
    let entries = rt.state.switcher_entries();
    assert_eq!(entries[0].key, "command:copy-session");
    rt.update(|s, _| s.switcher.search = rune::TextEdit::new("toggle terminal"));
    rt.frame(1.0 / 60.0);
    assert!(rt.elements().iter().any(|e| e.path.contains("sw-command:terminal") && e.key.as_deref() == Some("hint")), "⌘J shows as keycaps");
    rt.update(|s, _| s.switcher.search = rune::TextEdit::new("copy session"));
    rt.frame(1.0 / 60.0);
    key(&mut rt, Key::Enter);
    assert!(!rt.state.switcher.open);
    assert_eq!(rt.state.status, "Copied");
}

#[test]
fn ctrl_l_focuses_the_composer() {
    let mut rt = runtime();
    rt.update(|s, _| s.composer.focused = false);
    rt.frame(1.0 / 60.0);
    press(&mut rt, Key::Text("l".into()), Mods { ctrl: true, ..Default::default() });
    assert!(rt.state.composer.focused);
}

#[test]
fn the_commands_story_shows_each_list() {
    // Toggle sidebar isn't offered in Top tabs, the default layout since 42172690.
    for (variant, key) in [("Slash", "command-builtin.new"), ("Commands", "sw-command:new"), ("Search", "sw-command:side")] {
        let rt = crate::stories::story("Composer/Commands", Some(variant));
        let w = crate::stories::world(&rt);
        assert!(rt.playground_element(key).is_some(), "{variant}: {key} on stage; composer {:?} focused {} switcher {}", w.composer.text, w.composer.focused, w.switcher.open);
    }
}

#[test]
fn undo_redo_and_fork_show_when_they_can_run() {
    let mut rt = runtime();
    let listed = |rt: &Runtime<App>, id: &str| rt.state.commands().iter().any(|c| c.id == id);
    assert!(rt.state.can_undo_session(), "the fixture session has prompts");
    assert!(listed(&rt, "undo") && listed(&rt, "fork"));
    assert!(!listed(&rt, "redo"), "nothing to redo yet");
    tap(&mut rt, "prompt");
    typed(&mut rt, "/undo");
    assert_eq!(rt.state.slash_active().as_deref(), Some("builtin.undo"));
    key(&mut rt, Key::Enter);
    assert!(rt.state.can_redo_session(), "/undo reverted the last prompt");
    assert!(listed(&rt, "redo"));
    rt.update(|s, tx| s.run_command("redo", tx));
    rt.frame(1.0 / 60.0);
    assert!(!listed(&rt, "redo"));
    rt.update(|s, _| s.route = Route::Home);
    assert!(!listed(&rt, "undo") && !listed(&rt, "fork"), "Home has no session to rewind");
}
