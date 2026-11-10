use std::sync::mpsc::Receiver;

use opencode_kit::client::Write;
use rune::{Key, KeyEvent, Mods, Runtime};

use crate::session_menu::{Line, Spot, Target, lines};
use crate::{App, Route};

/// The fixture app on its open session, with a channel standing in for the server's writes.
fn app() -> (Runtime<App>, Receiver<Write>) {
    let mut app = App::fixture_session();
    let writes = app.fake_writes();
    let mut rt = app.into_runtime();
    frames(&mut rt, 90);
    (rt, writes)
}

fn frames(rt: &mut Runtime<App>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

fn go(rt: &mut Runtime<App>, route: Route) {
    rt.update(|s, tx| {
        s.route = route;
        tx.settle();
    });
    frames(rt, 90);
}

fn press(rt: &mut Runtime<App>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn double_click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is on screen"));
    press(rt, x, y);
    press(rt, x, y);
    frames(rt, 3);
}

fn right_click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} is on screen"));
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn type_text(rt: &mut Runtime<App>, text: &str) {
    rt.key(KeyEvent { key: Key::Text(text.into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

fn press_key(rt: &mut Runtime<App>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    frames(rt, 2);
}

fn tab_key(rt: &Runtime<App>, i: usize) -> String {
    format!("t-{}", rt.state.tabs[i].id)
}

fn titled(rt: &Runtime<App>, id: &str) -> String {
    rt.state.session(id).unwrap().title.clone()
}

fn ids(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|l| match l {
            Line::Item(i) => format!("{}{}", i.title, if i.enabled { "" } else { " (off)" }),
            Line::Sep => "—".into(),
        })
        .collect()
}

fn pick(rt: &mut Runtime<App>, id: &str) {
    rt.resolve_popup(Some(id)).unwrap_or_else(|e| panic!("{id}: {e:?}"));
    frames(rt, 3);
}

fn open_menu(rt: &mut Runtime<App>, key: &str, items: &[&str]) {
    right_click(rt, key);
    let menu = rt.popup_menu().unwrap_or_else(|| panic!("a right-click on {key} opens a menu"));
    for id in items {
        assert!(menu.find(id).is_some(), "{key}'s menu has {id}");
    }
}

#[test]
fn each_menu_lists_what_desktop_lists_in_its_order() {
    let (rt, _) = app();
    let id = rt.state.tabs[0].id.clone();
    let of = |spot| ids(&lines(&rt.state, &Target::new(&id, spot)));
    let more = if rt.state.tabs.len() > 1 { "" } else { " (off)" };
    assert_eq!(of(Spot::Tab), ["Rename", "Close tab", &*format!("Close other tabs{more}"), &*format!("Close tabs to the right{more}"), "—", "Delete session…"]);
    assert_eq!(of(Spot::Row), ["Rename", "Export…", "—", "Delete…"]);
    assert_eq!(of(Spot::Header), ["Rename", "Export…", "—", "Delete…"]);
    // A tab whose session hasn't loaded can be closed, and nothing else.
    assert_eq!(ids(&lines(&rt.state, &Target::new("ses_unknown", Spot::Tab))), ["Rename (off)", "Close tab", "Close other tabs", "Close tabs to the right (off)", "—", "Delete session… (off)"]);
    assert!(lines(&rt.state, &Target::new("ses_unknown", Spot::Row)).is_empty());
}

#[test]
fn a_subagents_header_has_no_menu() {
    let (mut rt, _) = app();
    let child = rt.state.sessions.iter().find(|s| s.parent.is_some()).map(|s| s.id.clone()).expect("the fixture has a subagent");
    assert!(lines(&rt.state, &Target::new(&child, Spot::Header)).is_empty());
    go(&mut rt, Route::here(child));
    assert!(rt.elements().iter().all(|e| e.exiting || e.key.as_deref() != Some("header-more")), "no ⋯ button on a subagent");
}

#[test]
fn double_clicking_a_tab_edits_its_title_and_enter_saves_it() {
    let (mut rt, writes) = app();
    let id = rt.state.tabs[1].id.clone();
    let before = titled(&rt, &id);
    let k = tab_key(&rt, 1);
    double_click(&mut rt, &k);
    assert_eq!(rt.state.menus.renaming(), Some(Target::new(&id, Spot::Tab)));
    assert_eq!(rt.state.menus.edit.text, before, "it starts on the current title");
    assert_eq!(rt.state.menus.edit.selected(), before, "all of it selected");
    assert!(rt.find("rename").is_some(), "the title is a field");

    type_text(&mut rt, "  Reconnect jitter  ");
    press_key(&mut rt, Key::Enter);
    assert_eq!(rt.state.menus.renaming(), None);
    assert_eq!(titled(&rt, &id), "Reconnect jitter", "trimmed, and shown at once");
    let Ok(Write::Rename { session, title }) = writes.try_recv() else { panic!("no rename sent") };
    assert_eq!((session.as_str(), title.as_str()), (id.as_str(), "Reconnect jitter"));
    assert!(writes.try_recv().is_err(), "one request");
}

#[test]
fn escape_cancels_a_rename_and_sends_nothing() {
    let (mut rt, writes) = app();
    let id = rt.state.tabs[1].id.clone();
    let before = titled(&rt, &id);
    let k = tab_key(&rt, 1);
    double_click(&mut rt, &k);
    type_text(&mut rt, "Something else");
    press_key(&mut rt, Key::Escape);
    assert_eq!(rt.state.menus.renaming(), None);
    assert_eq!(titled(&rt, &id), before);
    assert!(writes.try_recv().is_err());
}

#[test]
fn an_empty_or_unchanged_title_sends_nothing() {
    let (mut rt, writes) = app();
    let id = rt.state.tabs[1].id.clone();
    let k = tab_key(&rt, 1);
    double_click(&mut rt, &k);
    press_key(&mut rt, Key::Enter);
    assert!(writes.try_recv().is_err(), "unchanged");
    let k = tab_key(&rt, 1);
    double_click(&mut rt, &k);
    press_key(&mut rt, Key::Backspace);
    press_key(&mut rt, Key::Enter);
    assert!(writes.try_recv().is_err(), "empty");
    assert!(!titled(&rt, &id).is_empty());
}

#[test]
fn a_click_away_saves_a_tab_title() {
    let (mut rt, writes) = app();
    let id = rt.state.tabs[1].id.clone();
    let k = tab_key(&rt, 1);
    double_click(&mut rt, &k);
    type_text(&mut rt, "Saved by clicking away");
    let (x, y) = rt.find("search").or_else(|| rt.find("sidebar-header")).unwrap_or((600.0, 400.0));
    press(&mut rt, x, y);
    frames(&mut rt, 3);
    assert_eq!(rt.state.menus.renaming(), None, "the edit ended");
    assert_eq!(titled(&rt, &id), "Saved by clicking away");
    assert!(matches!(writes.try_recv(), Ok(Write::Rename { .. })));
}

#[test]
fn right_clicking_a_tab_opens_its_menu_and_each_item_acts() {
    let (mut rt, writes) = app();
    let id = rt.state.tabs[1].id.clone();
    let key = tab_key(&rt, 1);

    open_menu(&mut rt, &key, &["rename", "close", "delete"]);
    assert_eq!(rt.state.menus.on, Some(Target::new(&id, Spot::Tab)), "the row is marked while it shows");
    pick(&mut rt, "rename");
    assert_eq!(rt.state.menus.on, None);
    assert_eq!(rt.state.menus.renaming(), Some(Target::new(&id, Spot::Tab)));
    press_key(&mut rt, Key::Escape);

    let count = rt.state.tabs.len();
    open_menu(&mut rt, &key, &["close"]);
    pick(&mut rt, "close");
    assert_eq!(rt.state.tabs.len(), count - 1);
    assert!(rt.state.session(&id).is_some(), "closing a tab keeps the session");
    assert!(writes.try_recv().is_err());
}

#[test]
fn delete_asks_first_then_removes_the_session_and_its_tab() {
    let (mut rt, writes) = app();
    let id = rt.state.tabs[1].id.clone();
    let count = rt.state.tabs.len();
    let k = tab_key(&rt, 1);
    open_menu(&mut rt, &k, &["delete"]);
    pick(&mut rt, "delete");
    assert_eq!(rt.state.menus.confirming().as_deref(), Some(id.as_str()));
    assert!(rt.find("delete-confirm").is_some(), "the confirmation shows");
    assert!(writes.try_recv().is_err(), "nothing is sent yet");

    press_key(&mut rt, Key::Escape);
    assert_eq!(rt.state.menus.confirming(), None, "Esc cancels");
    assert!(rt.state.session(&id).is_some());

    let k = tab_key(&rt, 1);
    open_menu(&mut rt, &k, &["delete"]);
    pick(&mut rt, "delete");
    let (x, y) = rt.find("delete-cancel").unwrap();
    press(&mut rt, x, y);
    assert_eq!(rt.state.menus.confirming(), None, "Cancel cancels");
    assert!(writes.try_recv().is_err());

    let k = tab_key(&rt, 1);
    open_menu(&mut rt, &k, &["delete"]);
    pick(&mut rt, "delete");
    let (x, y) = rt.find("delete-confirm").unwrap();
    press(&mut rt, x, y);
    frames(&mut rt, 3);
    assert_eq!(rt.state.menus.confirming(), None);
    assert!(rt.state.session(&id).is_none() && rt.state.tabs.iter().all(|t| t.id != id));
    assert_eq!(rt.state.tabs.len(), count - 1);
    let Ok(Write::Delete(sent)) = writes.try_recv() else { panic!("no delete sent") };
    assert_eq!(sent, id);
}

#[test]
fn a_home_rows_menu_renames_exports_and_deletes() {
    let (mut rt, writes) = app();
    go(&mut rt, Route::Home);
    let id = rt.state.sessions.iter().find(|s| s.parent.is_none()).map(|s| s.id.clone()).unwrap();
    let key = format!("r-{id}");
    open_menu(&mut rt, &key, &["rename", "export", "delete"]);
    assert_eq!(rt.state.menus.on, Some(Target::new(&id, Spot::Row)));
    pick(&mut rt, "export");
    assert!(matches!(writes.try_recv(), Ok(Write::Export(s)) if s == id));

    open_menu(&mut rt, &key, &["rename"]);
    pick(&mut rt, "rename");
    assert_eq!(rt.state.menus.renaming(), Some(Target::new(&id, Spot::Row)));
    type_text(&mut rt, "Row rename");
    press_key(&mut rt, Key::Enter);
    assert_eq!(titled(&rt, &id), "Row rename");
    let Ok(Write::Rename { session, title }) = writes.try_recv() else { panic!("no rename sent") };
    assert_eq!((session, title.as_str()), (id.clone(), "Row rename"));

    open_menu(&mut rt, &key, &["delete"]);
    pick(&mut rt, "delete");
    assert_eq!(rt.state.menus.confirming(), Some(id));
}

#[test]
fn a_home_row_that_loses_focus_cancels_as_desktops_does() {
    let (mut rt, writes) = app();
    go(&mut rt, Route::Home);
    let id = rt.state.sessions.iter().find(|s| s.parent.is_none()).map(|s| s.id.clone()).unwrap();
    let before = titled(&rt, &id);
    open_menu(&mut rt, &format!("r-{id}"), &["rename"]);
    pick(&mut rt, "rename");
    type_text(&mut rt, "Not saved");
    press(&mut rt, 640.0, 790.0);
    frames(&mut rt, 3);
    assert_eq!(rt.state.menus.renaming(), None);
    assert_eq!(titled(&rt, &id), before);
    assert!(writes.try_recv().is_err());
}

#[test]
fn the_headers_title_renames_on_a_click_and_its_menu_opens_two_ways() {
    let (mut rt, writes) = app();
    let Route::Session { id, .. } = rt.state.route.clone() else { panic!("open session") };
    let id = rt.state.root_of(&id).to_owned();
    open_menu(&mut rt, "title", &["rename", "export", "delete"]);
    assert_eq!(rt.state.menus.on, Some(Target::new(&id, Spot::Header)));
    pick(&mut rt, "export");
    assert!(matches!(writes.try_recv(), Ok(Write::Export(s)) if s == id));

    // The ⋯ button anchors the same menu to itself.
    let (x, y) = rt.find("header-more").unwrap();
    press(&mut rt, x, y);
    let menu = rt.popup_menu().expect("the ⋯ button opens the menu");
    assert!(menu.find("rename").is_some() && menu.find("delete").is_some());
    pick(&mut rt, "rename");
    assert_eq!(rt.state.menus.renaming(), Some(Target::new(&id, Spot::Header)));
    press_key(&mut rt, Key::Escape);

    let (x, y) = rt.find("title").unwrap();
    press(&mut rt, x, y);
    assert_eq!(rt.state.menus.renaming(), Some(Target::new(&id, Spot::Header)), "a click on the title edits it");
    type_text(&mut rt, "Header rename");
    press_key(&mut rt, Key::Enter);
    let Ok(Write::Rename { title, .. }) = writes.try_recv() else { panic!("no rename sent") };
    assert_eq!(title, "Header rename");
}

#[test]
fn the_headers_rename_field_hugs_its_title_so_the_more_button_moves_only_with_typing() {
    let (mut rt, _) = app();
    let x = |rt: &Runtime<App>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key} is drawn")).rect[0];
    let rest = x(&rt, "header-more");
    let (tx, ty) = rt.find("title").unwrap();
    press(&mut rt, tx, ty);
    frames(&mut rt, 3);
    assert!(rt.state.menus.renaming().is_some());
    assert!((x(&rt, "header-more") - rest).abs() < 0.5, "⋯ stays where it was: {rest} → {}", x(&rt, "header-more"));

    press_key(&mut rt, Key::End);
    type_text(&mut rt, " and more");
    let typed = x(&rt, "header-more");
    assert!(typed > rest + 20.0, "typing pushes ⋯ over: {rest} → {typed}");
    for _ in 0..5 {
        press_key(&mut rt, Key::Backspace);
    }
    let deleted = x(&rt, "header-more");
    assert!(deleted < typed && deleted > rest, "deleting pulls it back: {typed} → {deleted}");

    type_text(&mut rt, &" a title far too long for the header".repeat(8));
    let long = x(&rt, "header-more");
    assert!(long + 28.0 <= 1280.0 && long > typed, "a long title stops at the header's room: {long}");
}

#[test]
fn control_r_renames_the_open_session() {
    let (mut rt, _) = app();
    let Route::Session { id, .. } = rt.state.route.clone() else { panic!("open session") };
    rt.key(KeyEvent { key: Key::Text("r".into()), mods: Mods { ctrl: true, ..Mods::default() } });
    frames(&mut rt, 2);
    assert_eq!(rt.state.menus.renaming(), Some(Target::new(&id, Spot::Header)));
}


/// Tabs into the list `key` is in (rows in a focus group take the arrows), then steps to it.
fn focus(rt: &mut Runtime<App>, key: &str) {
    let on = |rt: &Runtime<App>| rt.elements().iter().find(|e| e.focused).and_then(|e| e.key.clone());
    let prefix = &key[..2];
    for _ in 0..40 {
        if on(rt).is_some_and(|k| k.starts_with(prefix)) {
            break;
        }
        press_key(rt, Key::Tab);
    }
    for _ in 0..12 {
        if on(rt).as_deref() == Some(key) {
            return;
        }
        press_key(rt, Key::Down);
    }
    panic!("{key} never took focus, {:?} has it", on(rt));
}

fn menu_key(rt: &mut Runtime<App>, key: Key, shift: bool) {
    rt.key(KeyEvent { key, mods: Mods { shift, ..Mods::default() } });
    frames(rt, 2);
}

#[test]
fn the_menu_key_and_shift_f10_open_the_same_menu_as_a_right_click() {
    let (mut rt, _) = app();
    let key = tab_key(&rt, 0);
    focus(&mut rt, &key);
    menu_key(&mut rt, Key::ContextMenu, false);
    let menu = rt.popup_menu().expect("the Menu key opens the focused tab's menu");
    for id in ["rename", "close", "delete"] {
        assert!(menu.find(id).is_some(), "{id}");
    }
    assert_eq!(rt.state.menus.on, Some(Target::new(&rt.state.tabs[0].id.clone(), Spot::Tab)));
    let count = rt.state.tabs.len();
    pick(&mut rt, "close");
    assert_eq!(rt.state.tabs.len(), count - 1, "closing ran");

    let (mut rt, _) = app();
    go(&mut rt, Route::Home);
    let id = rt.state.sessions.iter().find(|s| s.parent.is_none()).map(|s| s.id.clone()).unwrap();
    focus(&mut rt, &format!("r-{id}"));
    menu_key(&mut rt, Key::Function(10), true);
    let menu = rt.popup_menu().expect("⇧F10 opens the focused row's menu");
    assert!(menu.find("export").is_some() && menu.find("delete").is_some());
    pick(&mut rt, "rename");
    assert_eq!(rt.state.menus.renaming(), Some(Target::new(&id, Spot::Row)));
}
