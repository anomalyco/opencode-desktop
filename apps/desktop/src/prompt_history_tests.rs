use rune::{Key, KeyEvent, Mods, Runtime, TextEdit};

use super::{Entry, PromptHistory};
use crate::{App, Route};

fn entry(text: &str) -> Entry {
    Entry { text: text.into(), pills: Vec::new() }
}

fn history(texts: &[&str]) -> PromptHistory {
    let mut h = PromptHistory::default();
    for t in texts {
        h.add(entry(t), false);
    }
    h
}

fn up(h: &mut PromptHistory, c: &mut TextEdit) -> bool {
    h.step(c, &Route::Draft("draft_a".into()), false, true)
}

fn down(h: &mut PromptHistory, c: &mut TextEdit) -> bool {
    h.step(c, &Route::Draft("draft_a".into()), false, false)
}

#[test]
fn up_walks_back_newest_first_and_stops_at_the_oldest() {
    let mut h = history(&["one", "two", "three"]);
    let mut c = TextEdit::new("");
    for want in ["three", "two", "one"] {
        assert!(up(&mut h, &mut c));
        assert_eq!(c.text, want);
        assert_eq!((c.anchor, c.focus), (0, 0), "↑ leaves the caret at the start");
    }
    assert!(!up(&mut h, &mut c), "past the oldest the arrow is the caret's");
    assert_eq!(c.text, "one");
}

#[test]
fn down_walks_forward_to_the_draft_you_had() {
    let mut h = history(&["one", "two"]);
    let mut c = TextEdit::new("");
    assert!(up(&mut h, &mut c) && up(&mut h, &mut c));
    assert_eq!(c.text, "one");
    assert!(down(&mut h, &mut c));
    assert_eq!(c.text, "two");
    assert_eq!(c.focus, 3, "↓ leaves the caret at the end");
    assert!(down(&mut h, &mut c));
    assert_eq!((c.text.as_str(), c.focus), ("", 0));
    assert!(!down(&mut h, &mut c), "nothing newer than the draft");
}

#[test]
fn up_starts_only_on_an_empty_composer() {
    let mut h = history(&["sent"]);
    let mut c = TextEdit::new("half a thought");
    c.set_caret(0);
    assert!(!up(&mut h, &mut c), "a draft is never overwritten");
    assert_eq!(c.text, "half a thought");
    assert!(up(&mut h, &mut TextEdit::new("")));
}

#[test]
fn arrows_in_the_middle_of_a_recalled_prompt_are_the_carets() {
    let mut h = history(&["first line\nsecond line"]);
    let mut c = TextEdit::new("");
    assert!(up(&mut h, &mut c));
    c.set_caret(4);
    assert!(!up(&mut h, &mut c) && !down(&mut h, &mut c));
    c.set_caret(c.text.len());
    assert!(down(&mut h, &mut c), "at the end ↓ goes on");
    assert_eq!(c.text, "");
}

#[test]
fn editing_a_recalled_prompt_ends_the_walk() {
    let mut h = history(&["one", "two"]);
    let mut c = TextEdit::new("");
    assert!(up(&mut h, &mut c));
    c.insert("!");
    c.set_caret(0);
    assert!(!up(&mut h, &mut c), "edited text is a draft now");
    assert!(!down(&mut h, &mut c));
    assert_eq!(c.text, "!two");
}

#[test]
fn changing_page_or_mode_ends_the_walk() {
    let mut h = history(&["one", "two"]);
    let mut c = TextEdit::new("");
    assert!(up(&mut h, &mut c));
    c.set_caret(0);
    assert!(!h.step(&mut c, &Route::Home, false, true));
    assert!(!h.step(&mut c, &Route::Draft("draft_a".into()), true, true), "shell mode has its own, empty, list");
}

#[test]
fn shell_mode_keeps_its_own_list() {
    let mut h = history(&["prompt"]);
    h.add(entry("ls -la"), true);
    let mut c = TextEdit::new("");
    assert!(h.step(&mut c, &Route::Draft("draft_a".into()), true, true));
    assert_eq!(c.text, "ls -la");
}

#[test]
fn empty_history_does_nothing() {
    let mut h = PromptHistory::default();
    let mut c = TextEdit::new("");
    assert!(!up(&mut h, &mut c) && !down(&mut h, &mut c));
}

#[test]
fn adding_skips_blanks_and_repeats_and_keeps_a_hundred() {
    let mut h = PromptHistory::default();
    h.add(entry("  \n"), false);
    h.add(entry("a"), false);
    h.add(entry("a"), false);
    assert_eq!(h.entries(false).len(), 1);
    for i in 0..150 {
        h.add(entry(&i.to_string()), false);
    }
    assert_eq!(h.entries(false).len(), 100);
    assert_eq!(h.entries(false)[0].text, "149");
}

#[test]
fn pills_come_back_as_pills() {
    let mut c = TextEdit::new("");
    c.insert("ask ");
    c.insert_pill("agent:explore", "@explore");
    c.insert(" now");
    let mut h = PromptHistory::default();
    h.add(Entry::of(&c), false);
    let mut next = TextEdit::new("");
    assert!(up(&mut h, &mut next));
    assert_eq!(next.text, "ask @explore now");
    assert_eq!(next.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(), [(4..12, "agent:explore".to_owned())]);
}

#[test]
fn entries_round_trip_through_json() {
    let e = Entry { text: "see @src/a.rs".into(), pills: vec![(4..13, "file:src/a.rs".into())] };
    assert_eq!(Entry::from_json(&e.json()), Some(e));
    let bad = serde_json::json!({ "text": "ab", "pills": [[0, 9, "x"], [1, 1, "y"]] });
    assert_eq!(Entry::from_json(&bad), Some(entry("ab")));
}

fn rt() -> Runtime<App> {
    let mut app = App::fixture_session();
    app.prompts.add(entry("an old prompt"), false);
    app.composer.focused = true;
    let mut rt = app.into_runtime();
    rt.frame(1.0 / 60.0);
    rt
}

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) -> bool {
    let mut handled = false;
    rt.update(|s, tx| handled = s.history_key(&KeyEvent { key, mods }, tx));
    handled
}

#[test]
fn the_composer_recalls_with_the_arrows() {
    let mut rt = rt();
    assert!(press(&mut rt, Key::Up, Mods::default()));
    assert_eq!(rt.state.composer.text, "an old prompt");
    assert!(press(&mut rt, Key::Down, Mods::default()));
    assert_eq!(rt.state.composer.text, "");
}

#[test]
fn an_open_popover_keeps_the_arrows() {
    let mut rt = rt();
    rt.state.suggest.state = crate::suggestions::machine::Popover::Context(Default::default());
    assert!(!press(&mut rt, Key::Up, Mods::default()));
    rt.state.suggest.state = Default::default();
    rt.state.add.menu = crate::add_menu::Menu::Open { active: None, keyboard: false };
    assert!(!press(&mut rt, Key::Up, Mods::default()));
    rt.state.add.menu = crate::add_menu::Menu::Closed;
    rt.state.composer.insert("/");
    assert!(!press(&mut rt, Key::Up, Mods::default()), "the / popover is showing");
    assert_eq!(rt.state.composer.text, "/");
}

#[test]
fn modified_arrows_and_an_unfocused_composer_are_left_alone() {
    let mut rt = rt();
    assert!(!press(&mut rt, Key::Up, Mods { shift: true, ..Mods::default() }));
    rt.state.composer.focused = false;
    assert!(!press(&mut rt, Key::Up, Mods::default()));
}

#[test]
fn recording_waits_for_the_send_to_clear_the_composer() {
    let mut app = App::empty();
    app.composer = TextEdit::new("kept");
    app.record_sent(|_| {});
    assert!(app.prompts.entries(false).is_empty(), "a send that didn't go leaves nothing");
    app.record_sent(|s| s.composer.clear());
    assert_eq!(app.prompts.entries(false)[0].text, "kept");
}
