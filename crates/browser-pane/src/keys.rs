//! Which keys stay with the app, and the engine's names for the rest.

use rune::{Key, KeyEvent, Mods};

pub(crate) fn is_shell_shortcut(ev: &KeyEvent) -> bool {
    let m: Mods = ev.mods;
    if m.alt && !m.cmd && !m.ctrl && matches!(ev.key, Key::Up | Key::Down) {
        return true;
    }
    if m.ctrl && !m.cmd && !m.alt && ev.key == Key::Tab {
        return true;
    }
    match ev.key {
        Key::Command(c) => matches!(
            c.to_ascii_lowercase(),
            'k' | 'l' | 't' | 'n' | 'w' | 'b' | ',' | '[' | ']' | '1'..='9' | '=' | '+' | '-' | '_' | '0'
        ),
        _ => false,
    }
}

pub(crate) fn named_key_str(key: &Key) -> Option<&'static str> {
    match key {
        Key::Enter => Some("Enter"),
        Key::Backspace => Some("Backspace"),
        Key::Delete => Some("Delete"),
        Key::Tab => Some("Tab"),
        Key::Escape => Some("Escape"),
        Key::Left => Some("ArrowLeft"),
        Key::Right => Some("ArrowRight"),
        Key::Up => Some("ArrowUp"),
        Key::Down => Some("ArrowDown"),
        Key::Home => Some("Home"),
        Key::End => Some("End"),
        Key::PageUp => Some("PageUp"),
        Key::PageDown => Some("PageDown"),
        _ => None,
    }
}
