//! Typing and pasting with the composer unfocused (t1469).

use rune::clipboard::{ClipboardContent, ClipboardImage, MemoryClipboard};
use rune::{Key, KeyEvent, Mods, Runtime};

use crate::view::selection_tests::{drag, first_line, last_line, session, settle, text};
use crate::{App, Route};

/// The fixture's session with `draft` in the composer, its caret at the start, and nothing
/// focused, as after a click on the transcript.
fn unfocused(draft: &str) -> (Runtime<App>, MemoryClipboard) {
    let (mut rt, board) = session(draft);
    rt.update(|s, _| {
        s.composer.set_selection(0, 0);
        s.composer.focused = false;
    });
    settle(&mut rt);
    assert!(!rt.has_focus());
    (rt, board)
}

fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 120.0);
}

fn char(rt: &mut Runtime<App>, c: &str) {
    press(rt, Key::Text(c.into()), Mods::default());
}

fn cmd(c: char) -> (Key, Mods) {
    (Key::Command(c), Mods { cmd: true, ..Mods::default() })
}

fn paste(rt: &mut Runtime<App>) {
    let (key, mods) = cmd('v');
    press(rt, key, mods);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
}

#[test]
fn a_key_focuses_the_composer_and_lands_at_the_end() {
    let (mut rt, _) = unfocused("Fix the");
    char(&mut rt, " ");
    assert!(!rt.state.composer.focused, "Space leaves the composer alone");
    char(&mut rt, "!");
    assert!(rt.state.composer.focused);
    assert_eq!(rt.state.composer.text, "Fix the!");
    for c in [" ", "b", "u", "g"] {
        char(&mut rt, c);
    }
    assert_eq!(rt.state.composer.text, "Fix the! bug", "later keys follow at the caret");
}

#[test]
fn keys_before_the_field_takes_focus_are_neither_lost_nor_doubled() {
    let (mut rt, _) = unfocused("");
    for c in ["o", "k"] {
        rt.key(KeyEvent { key: Key::Text(c.into()), mods: Mods::default() });
    }
    rt.frame(1.0 / 120.0);
    char(&mut rt, "!");
    assert_eq!(rt.state.composer.text, "ok!");
}

#[test]
fn shift_and_option_characters_type() {
    let (mut rt, _) = unfocused("");
    press(&mut rt, Key::Text("H".into()), Mods { shift: true, ..Mods::default() });
    press(&mut rt, Key::Text("π".into()), Mods { alt: true, ..Mods::default() });
    assert_eq!(rt.state.composer.text, "Hπ");
}

#[test]
fn a_slash_opens_the_command_list_as_typed() {
    let (mut rt, _) = unfocused("");
    char(&mut rt, "/");
    assert_eq!(rt.state.composer.text, "/");
    assert!(rt.state.slash_active().is_some(), "the composer's change hook ran");
}

#[test]
fn cmd_v_pastes_text_at_the_end() {
    let (mut rt, board) = unfocused("Look at");
    board.set_content(ClipboardContent::new(" this log"));
    paste(&mut rt);
    assert!(rt.state.composer.focused);
    assert_eq!(rt.state.composer.text, "Look at this log");
}

#[test]
fn cmd_v_attaches_an_image() {
    let (mut rt, board) = unfocused("");
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    board.set_content(ClipboardContent::default().with_image(ClipboardImage::from_png(png).unwrap()));
    paste(&mut rt);
    assert!(rt.state.composer.focused);
    assert_eq!(rt.state.attach.files.len(), 1);
    assert_eq!(rt.state.attach.files[0].mime, "image/png");
    assert!(rt.state.composer.text.is_empty());
}

#[test]
fn cmd_v_attaches_files_focused_or_not() {
    let dir = std::env::temp_dir().join(format!("type-to-compose-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("notes.txt");
    std::fs::write(&file, "hello").unwrap();
    let attached = |rt: &Runtime<App>| rt.state.attach.files.len() + rt.state.attach.paths.len();
    let (mut rt, board) = unfocused("");
    board.set_content(ClipboardContent::new("notes.txt").with_files([file.clone()]));
    paste(&mut rt);
    assert_eq!(attached(&rt), 1, "a file attaches");
    assert!(rt.state.composer.text.is_empty(), "its name isn't typed");
    // The focused composer's own paste attaches files the same way.
    rt.update(|s, _| s.attach.files.clear());
    rt.update(|s, _| s.attach.paths.clear());
    settle(&mut rt);
    assert!(rt.has_focus());
    rt.update(|_, tx| tx.request_paste());
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(attached(&rt), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn shortcuts_and_navigation_keys_stay_theirs() {
    let (mut rt, _) = unfocused("draft");
    for key in [Key::Up, Key::Down, Key::Left, Key::Right, Key::Enter, Key::Tab] {
        press(&mut rt, key.clone(), Mods::default());
        assert!(!rt.state.composer.focused, "{key:?}");
    }
    press(&mut rt, Key::Text("l".into()), Mods { ctrl: true, ..Mods::default() });
    assert!(rt.state.composer.focused, "⌃L still focuses through its own command");
    assert_eq!(rt.state.composer.text, "draft", "and types nothing");
    let (mut rt, _) = unfocused("draft");
    let (key, mods) = cmd('k');
    press(&mut rt, key, mods);
    assert!(rt.state.switcher.open, "⌘K opens the palette");
    char(&mut rt, "x");
    assert_eq!(rt.state.composer.text, "draft");
    assert!(!rt.state.composer.focused);
    assert_eq!(rt.state.switcher.search.text, "x", "the palette's field types");
}

#[test]
fn open_fields_panels_and_dialogs_keep_their_keys() {
    let cases: [(&str, fn(&mut App, &mut rune::Tx)); 4] = [
        ("find", |s, tx| s.open_find(tx)),
        ("settings", |s, tx| s.toggle_settings(tx)),
        ("side panel", |s, _| s.side.focused = true),
        ("project menu", |s, _| s.project_state.home_menu_open = true),
    ];
    for (name, open) in cases {
        let (mut rt, board) = unfocused("draft");
        rt.update(open);
        settle(&mut rt);
        char(&mut rt, "x");
        board.set_content(ClipboardContent::new("pasted"));
        paste(&mut rt);
        assert_eq!(rt.state.composer.text, "draft", "{name}");
        assert!(!rt.state.composer.focused, "{name}");
    }
}

#[test]
fn pages_without_a_composer_ignore_stray_keys() {
    let (mut rt, _) = unfocused("");
    rt.update(|s, _| s.route = Route::Home);
    settle(&mut rt);
    char(&mut rt, "x");
    assert!(!rt.state.composer.focused);
    assert!(rt.state.composer.text.is_empty());
}

#[test]
fn the_new_session_page_takes_typing_too() {
    let (mut rt, _) = unfocused("");
    rt.update(|s, tx| s.new_session(tx));
    settle(&mut rt);
    rt.update(|s, _| s.composer.focused = false);
    settle(&mut rt);
    assert!(matches!(rt.state.route, Route::Draft(_)));
    char(&mut rt, "a");
    assert!(rt.state.composer.focused);
    assert_eq!(rt.state.composer.text, "a");
}

#[test]
fn a_transcript_selection_still_copies() {
    let (mut rt, board) = unfocused("");
    let para = text(&rt, "Treat silence as failure");
    drag(&mut rt, first_line(&para, 0.0), last_line(&para));
    let (key, mods) = cmd('c');
    press(&mut rt, key, mods);
    let copied = board.text().map(|t| t.to_string()).unwrap_or_default();
    assert!(copied.starts_with("Treat silence as failure"), "{copied:?}");
    assert!(!rt.state.composer.focused, "⌘C doesn't move focus");
    assert!(rt.state.composer.text.is_empty());
}
