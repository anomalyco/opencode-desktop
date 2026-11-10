use rune::{Key, KeyEvent, Mods, Shell};

use super::close::{CloseKey, CloseWindow, OTHERS, close_key, close_window};
use crate::{App, Route};

fn cmd_w(shift: bool) -> KeyEvent {
    KeyEvent { key: Key::Command('w'), mods: Mods { cmd: true, logo: true, shift, ..Mods::default() } }
}

/// The fixture's open session, with `others` other windows open.
fn runtime(others: usize) -> rune::Runtime<App> {
    OTHERS.with(|o| o.set(others));
    crate::fixture_runtime()
}

fn press(rt: &mut rune::Runtime<App>, shift: bool) {
    rt.key(cmd_w(shift));
    rt.frame(1.0 / 60.0);
}

fn go_home(rt: &mut rune::Runtime<App>) {
    rt.update(|s, tx| s.go_home(tx));
    rt.frame(1.0 / 60.0);
}

#[test]
fn cmd_w_closes_the_tab_in_front_then_an_empty_window_then_nothing() {
    assert_eq!(close_key(Some("a".into()), true, false), CloseKey::Tab("a".into()));
    assert_eq!(close_key(Some("a".into()), true, true), CloseKey::Tab("a".into()));
    // Home in a window with no tabs: the window, when another one is open.
    assert_eq!(close_key(None, false, true), CloseKey::Window);
    assert_eq!(close_key(None, false, false), CloseKey::Nothing, "the only window stays");
    // Home with tabs still listed: ⌘W never takes the window and its tabs with it.
    assert_eq!(close_key(None, true, true), CloseKey::Nothing);
}

#[test]
fn shift_cmd_w_closes_the_window_and_the_last_one_on_macos_hides() {
    assert_eq!(close_window(true, true), CloseWindow::Close);
    assert_eq!(close_window(true, false), CloseWindow::Close);
    assert_eq!(close_window(false, true), CloseWindow::Hide, "the app keeps running");
    assert_eq!(close_window(false, false), CloseWindow::Close, "Windows and Linux close it");
}

#[test]
fn cmd_w_on_a_session_closes_its_tab_not_the_window() {
    let mut rt = runtime(1);
    let open = rt.state.current_tab().expect("the fixture opens a tab");
    press(&mut rt, false);
    assert!(!rt.state.tabs.iter().any(|t| t.id == open), "its tab closed");
    assert_eq!(rt.shell_request, None, "the window stays");
}

#[test]
fn cmd_w_in_an_empty_window_closes_it_while_another_is_open() {
    let mut rt = runtime(1);
    rt.state.tabs.clear();
    go_home(&mut rt);
    press(&mut rt, false);
    assert_eq!(rt.shell_request, Some(Shell::Close));
}

#[test]
fn cmd_w_in_the_only_window_does_nothing() {
    let mut rt = runtime(0);
    rt.state.tabs.clear();
    go_home(&mut rt);
    press(&mut rt, false);
    assert_eq!(rt.shell_request, None);
    assert_eq!(rt.state.route, Route::Home);
}

#[test]
fn cmd_w_on_home_with_tabs_keeps_the_window_and_its_tabs() {
    let mut rt = runtime(1);
    let tabs = rt.state.tabs.len();
    go_home(&mut rt);
    press(&mut rt, false);
    assert_eq!((rt.shell_request, rt.state.tabs.len()), (None, tabs));
}

#[test]
fn shift_cmd_w_is_not_close_tab() {
    assert_eq!(crate::shortcuts::action(&cmd_w(true)), None, "⇧⌘W stays Close Window's");
    let mut rt = runtime(1);
    let tabs = rt.state.tabs.len();
    press(&mut rt, true);
    assert_eq!(rt.state.tabs.len(), tabs, "⇧⌘W closed no tab");
}

#[test]
fn close_window_closes_it_with_another_open_from_a_session_or_home() {
    for home in [false, true] {
        let mut rt = runtime(1);
        if home {
            go_home(&mut rt);
        }
        rt.update(|s, tx| crate::app_menu::run(s, crate::app_menu::Act::CloseWindow, tx));
        assert_eq!(rt.shell_request, Some(Shell::Close), "home: {home}");
    }
}

#[test]
fn close_tab_menu_item_does_what_cmd_w_does() {
    let mut rt = runtime(1);
    rt.state.tabs.clear();
    go_home(&mut rt);
    rt.update(|s, tx| crate::app_menu::run(s, crate::app_menu::Act::CloseTab, tx));
    assert_eq!(rt.shell_request, Some(Shell::Close));
}
