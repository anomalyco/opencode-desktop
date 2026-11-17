//! Which keys stay with the app, and the engine's names for the rest.

use std::sync::LazyLock;

use rune::{Key, KeyEvent, Keybind};

/// The app's tab keys: Alt+↑/↓ (with Shift, unread ones) and Ctrl+Tab.
static TAB_KEYS: LazyLock<Keybind> =
    LazyLock::new(|| Keybind::new("alt+up, alt+down, shift+alt+up, shift+alt+down, ctrl+tab, ctrl+shift+tab"));

static ZOOM_IN_KEYS: LazyLock<Keybind> = LazyLock::new(|| Keybind::new("mod+=, mod+plus, mod+shift+=, mod+shift+plus"));
static ZOOM_OUT_KEYS: LazyLock<Keybind> = LazyLock::new(|| Keybind::new("mod+-, mod+shift+-, mod+_"));
static ZOOM_RESET_KEYS: LazyLock<Keybind> = LazyLock::new(|| Keybind::new("mod+0"));

/// Page zoom direction requested by `ev` (`packages/gui-plugins/src/browser/chromium.ts:124-130`).
pub(crate) fn page_zoom_dir(ev: &KeyEvent) -> Option<i32> {
    if ZOOM_IN_KEYS.matches(ev) {
        Some(1)
    } else if ZOOM_OUT_KEYS.matches(ev) {
        Some(-1)
    } else if ZOOM_RESET_KEYS.matches(ev) {
        Some(0)
    } else {
        None
    }
}

pub(crate) fn is_shell_shortcut(ev: &KeyEvent) -> bool {
    if TAB_KEYS.matches(ev) {
        return true;
    }
    match ev.key {
        Key::Command(c) => matches!(
            c.to_ascii_lowercase(),
            'k' | 'l' | 't' | 'n' | 'w' | 'b' | ',' | '[' | ']' | '1'..='9'
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
