use crate::fixture_ids::{MAIN, CHILD, THIRD as OTHER};
use rune::{Key, KeyEvent, Mods, PhysicalKeyCode, Runtime};

use crate::history::{Place, bracket};
use crate::{App, Route};

fn rt() -> Runtime<App> {
    let mut rt = App::fixture_session().into_runtime();
    rt.frame(1.0 / 60.0);
    rt
}

fn cmd(c: char) -> KeyEvent {
    KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } }
}

fn press(rt: &mut Runtime<App>, c: char) {
    rt.key(cmd(c));
    rt.frame(1.0 / 60.0);
}

fn go(rt: &mut Runtime<App>, f: impl FnOnce(&mut App, &mut rune::Tx)) {
    rt.update(|s, tx| f(s, tx));
    rt.frame(1.0 / 60.0);
}

fn route(rt: &Runtime<App>) -> Route {
    rt.state.route.clone()
}

fn session(id: &str) -> Route {
    Route::Session(id.into())
}

#[test]
fn back_and_forward_walk_session_home_session_subagent() {
    let mut rt = rt();
    go(&mut rt, |s, tx| s.open(OTHER, tx));
    go(&mut rt, |s, tx| {
        s.route = Route::Home;
        tx.settle();
    });
    go(&mut rt, |s, tx| s.open(MAIN, tx));
    go(&mut rt, |s, tx| s.open(CHILD, tx));
    press(&mut rt, '[');
    assert_eq!(route(&rt), session(MAIN));
    press(&mut rt, '[');
    assert_eq!(route(&rt), Route::Home);
    press(&mut rt, '[');
    assert_eq!(route(&rt), session(OTHER));
    press(&mut rt, ']');
    assert_eq!(route(&rt), Route::Home);
    press(&mut rt, ']');
    press(&mut rt, ']');
    assert_eq!(route(&rt), session(CHILD));
    press(&mut rt, ']');
    assert_eq!(route(&rt), session(CHILD), "nothing ahead");
}

#[test]
fn the_first_page_has_home_behind_it() {
    let mut rt = rt();
    assert_eq!(route(&rt), session(MAIN));
    press(&mut rt, '[');
    assert_eq!(route(&rt), Route::Home);
    press(&mut rt, '[');
    assert_eq!(route(&rt), Route::Home, "nothing behind");
    press(&mut rt, ']');
    assert_eq!(route(&rt), session(MAIN));
}

#[test]
fn a_new_move_clears_what_was_ahead() {
    let mut rt = rt();
    go(&mut rt, |s, tx| s.open(OTHER, tx));
    press(&mut rt, '[');
    assert_eq!(route(&rt), session(MAIN));
    go(&mut rt, |s, tx| s.new_session(tx));
    press(&mut rt, ']');
    assert!(matches!(route(&rt), Route::Draft(_)), "OTHER is no longer ahead");
    press(&mut rt, '[');
    assert_eq!(route(&rt), session(MAIN));
}

#[test]
fn settings_is_an_entry() {
    let mut rt = rt();
    go(&mut rt, |s, _| s.open_settings());
    assert!(rt.state.settings.open);
    press(&mut rt, '[');
    assert!(!rt.state.settings.open);
    assert_eq!(route(&rt), session(MAIN));
    press(&mut rt, ']');
    assert!(rt.state.settings.open);
}

#[test]
fn closing_a_tab_drops_its_entries() {
    let mut rt = rt();
    go(&mut rt, |s, tx| s.open(CHILD, tx));
    go(&mut rt, |s, tx| s.open(OTHER, tx));
    go(&mut rt, |s, tx| s.close_tab(MAIN, tx));
    let places = rt.state.nav.places();
    let has = |id: &str| places.iter().any(|p: &Place| p.route == session(id));
    assert!(!has(MAIN) && !has(CHILD), "its page and its subagent are gone: {places:?}");
    assert!(has(OTHER));
    for _ in 0..4 {
        press(&mut rt, '[');
        assert_ne!(route(&rt), session(MAIN));
        assert_ne!(route(&rt), session(CHILD));
    }
    assert_eq!(route(&rt), Route::Home);
}

#[test]
fn the_shifted_braces_and_other_layouts() {
    let key = |c, shift| KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, shift, ..Mods::default() } };
    assert_eq!(bracket(&key('[', false), None), Some(false));
    assert_eq!(bracket(&key(']', false), None), Some(true));
    // A layout that types no ASCII there (⌘ with the keys left of Return on a German one).
    assert_eq!(bracket(&key('ü', false), Some(PhysicalKeyCode::BracketLeft)), Some(false));
    assert_eq!(bracket(&key('+', false), Some(PhysicalKeyCode::BracketRight)), None, "ASCII is what it says");
    assert_eq!(bracket(&key('ü', false), Some(PhysicalKeyCode::BracketRight)), Some(true));
    assert_eq!(bracket(&key('ü', false), None), None);
    assert_eq!(bracket(&key('ü', false), Some(PhysicalKeyCode::KeyA)), None);
    // Desktop binds mod+[ without Shift, so ⇧⌘[ is a different chord.
    assert_eq!(bracket(&key('[', true), None), None);
    let alt = KeyEvent { key: Key::Command('['), mods: Mods { cmd: true, alt: true, ..Mods::default() } };
    assert_eq!(bracket(&alt, None), None);
    assert_eq!(bracket(&KeyEvent { key: Key::Command('['), mods: Mods::default() }, None), None);
}

#[test]
fn the_menu_goes_back_too() {
    let mut rt = rt();
    go(&mut rt, |s, tx| s.open(OTHER, tx));
    go(&mut rt, |s, tx| {
        s.navigate(false, tx);
    });
    assert_eq!(route(&rt), session(MAIN));
    assert_eq!(crate::history::menu_items().len(), 2);
}

mod question {
    use super::*;

    fn asked() -> Runtime<App> {
        let mut rt = rt();
        let q = rt.state.fixture["question"].clone();
        rt.update(|s, _| {
            // The keyboard-first dock's ⌘[; Classic's is Alt+← off a Mac, as Desktop's.
            s.settings.new_question_ui = true;
            s.apply_event("form.created", &q);
        });
        rt.frame(1.0 / 60.0);
        rt
    }

    #[test]
    fn the_dock_keeps_command_bracket_until_its_first_question() {
        let mut rt = asked();
        go(&mut rt, |s, tx| s.open(OTHER, tx));
        go(&mut rt, |s, tx| s.open(MAIN, tx));
        assert!(rt.state.question_shown());
        rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        press(&mut rt, '[');
        assert_eq!(route(&rt), session(MAIN), "⌘[ stepped the dock back");
        press(&mut rt, '[');
        assert_eq!(route(&rt), session(OTHER), "on the first question it goes back through the history");
    }
}

#[test]
fn the_story_plays_three_backs_and_a_forward() {
    let mut rt = crate::stories::story("Navigation/Back and forward", None);
    for _ in 0..(7.5 * 60.0) as usize {
        rt.frame(1.0 / 60.0);
    }
    let world = crate::stories::world(&rt);
    assert_eq!(world.route, Route::Home);
    assert_eq!(world.nav.places().len(), 6);
}
