//! Keys from the app as CEF key events.

use cef::{BrowserHost, ImplBrowserHost, KeyEvent, KeyEventType, sys};

use crate::handlers::{ascii_vk, mac_key_code, named_codes, native_key_code};

pub fn send_key(
    host: &BrowserHost,
    down: bool,
    text: Option<&str>,
    named: Option<&str>,
    modifiers: u32,
) {
    if let Some(s) = text {
        for ch in s.chars() {
            let vk = ascii_vk(ch);
            let mc = native_key_code(vk, mac_key_code(ch));
            if down {
                let raw_down = KeyEvent {
                    type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_RAWKEYDOWN),
                    modifiers,
                    windows_key_code: vk,
                    native_key_code: mc,
                    character: ch as u16,
                    unmodified_character: ch.to_ascii_lowercase() as u16,
                    ..Default::default()
                };
                host.send_key_event(Some(&raw_down));
                let chr = KeyEvent {
                    type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_CHAR),
                    modifiers,
                    windows_key_code: ch as i32,
                    native_key_code: mc,
                    character: ch as u16,
                    unmodified_character: ch.to_ascii_lowercase() as u16,
                    ..Default::default()
                };
                host.send_key_event(Some(&chr));
            } else {
                let up = KeyEvent {
                    type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_KEYUP),
                    modifiers,
                    windows_key_code: vk,
                    native_key_code: mc,
                    character: ch as u16,
                    unmodified_character: ch.to_ascii_lowercase() as u16,
                    ..Default::default()
                };
                host.send_key_event(Some(&up));
            }
        }
        return;
    }
    let Some(name) = named else { return };
    let Some((vk, mc, ch)) = named_codes(name) else { return };
    let mc = native_key_code(vk, mc);
    let ty = if down {
        sys::cef_key_event_type_t::KEYEVENT_RAWKEYDOWN
    } else {
        sys::cef_key_event_type_t::KEYEVENT_KEYUP
    };
    let ev = KeyEvent {
        type_: KeyEventType::from(ty),
        modifiers,
        windows_key_code: vk,
        native_key_code: mc,
        character: ch.unwrap_or(0),
        unmodified_character: ch.unwrap_or(0),
        ..Default::default()
    };
    host.send_key_event(Some(&ev));
    if down && let Some(c) = ch {
        let chr = KeyEvent {
            type_: KeyEventType::from(sys::cef_key_event_type_t::KEYEVENT_CHAR),
            modifiers,
            windows_key_code: vk,
            native_key_code: mc,
            character: c,
            unmodified_character: c,
            ..Default::default()
        };
        host.send_key_event(Some(&chr));
    }
}
