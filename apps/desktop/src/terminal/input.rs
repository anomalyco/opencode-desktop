//! Keyboard and IME translation from Rune events into `terminal::KeyInput`, copy/paste, and scroll.

use rune::{Ime, Key, KeyEvent, Tx};
use terminal::{
    GhosttyKey, KeyAction, KeyInput, ScrollOp, SelectionOp, StreamHandle, TerminalSession,
};

use crate::App;
use crate::keymap::Bind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    Ignored,
    Consumed,
    ConsumedResetBlink,
}

#[allow(clippy::too_many_arguments)]
pub fn dispatch_key(
    session: &TerminalSession,
    stream: Option<&StreamHandle>,
    fixture_dir: Option<&str>,
    ws_key: Option<String>,
    tab_id: Option<String>,
    ev: &KeyEvent,
    viewport_rows: u16,
    tx: &mut Tx,
) -> KeyOutcome {
    let m = ev.mods;

    // ⌘C, and Ctrl+Shift+C everywhere (Desktop `terminal.tsx:477`), copy the selection; ⌘C with
    // none leaves the key to the app. Off macOS Ctrl+C stays the shell's interrupt.
    if Bind::TerminalCopy.matches_tx(ev, tx) {
        if let Some(sel) = session.selected_text() {
            tx.copy(sel);
            return KeyOutcome::Consumed;
        }
        if crate::keymap::logo_held(m) {
            return KeyOutcome::Ignored;
        }
    }

    if Bind::TerminalPaste.matches_tx(ev, tx) {
        paste_clipboard(ws_key, tab_id, tx);
        return KeyOutcome::Consumed;
    }

    if Bind::TerminalSelectAll.matches_tx(ev, tx) {
        session.select(SelectionOp::All);
        return KeyOutcome::Consumed;
    }

    // `⌘Backspace` kills the line (`\x15`, matching OpenCode Desktop `terminal-key-event.ts`).
    if Bind::TerminalKillLine.matches_tx(ev, tx) {
        let mut ki = KeyInput::press(GhosttyKey::Backspace);
        ki.cmd = true;
        session.send_key(ki);
        return KeyOutcome::ConsumedResetBlink;
    }

    // Other ⌘ chords bubble up (`⌘J`, `⌘W`, `⌘T`, `⌘K`, `⌘1..9`, `⌘,`), as Desktop's
    // `consumeTerminalMetaKey` keeps them from the shell. Off macOS Ctrl is the shell's key, so a
    // Ctrl chord leaves only when it is one of the app's commands (Ctrl+T, Ctrl+K, Ctrl+`). Alt
    // chords do the same: Desktop's keymap listens on the document's capture phase, so its
    // Alt+Up and Alt+Down switch sessions before the terminal sees them.
    if crate::keymap::logo_held(m) || ((m.cmd || m.alt) && crate::keymap::takes_from_pane(ev, tx)) {
        return KeyOutcome::Ignored;
    }

    // `Shift+PageUp` / `Shift+PageDown` / `Shift+Home` / `Shift+End` scroll terminal scrollback.
    if m.shift && !m.ctrl && !m.alt {
        let page = viewport_rows.saturating_sub(2).max(1) as isize;
        match ev.key {
            Key::PageUp => {
                session.scroll(ScrollOp::Delta(-page));
                return KeyOutcome::Consumed;
            }
            Key::PageDown => {
                session.scroll(ScrollOp::Delta(page));
                return KeyOutcome::Consumed;
            }
            Key::Home => {
                session.scroll(ScrollOp::Top);
                return KeyOutcome::Consumed;
            }
            Key::End => {
                session.scroll(ScrollOp::Bottom);
                return KeyOutcome::Consumed;
            }
            _ => {}
        }
    }

    // `Shift+Enter` sends `\x1b\r` unless Kitty keyboard protocol is active (matches Kit's Ghostty config).
    if m.shift
        && !m.ctrl
        && !m.alt
        && ev.key == Key::Enter
        && session.snapshot().kitty_keyboard_flags == 0
    {
        session.write_bytes(b"\x1b\r".to_vec());
        session.scroll(ScrollOp::Bottom);
        return KeyOutcome::ConsumedResetBlink;
    }

    let Some(ki) = map_key_event(ev) else {
        return KeyOutcome::Ignored;
    };
    session.select(SelectionOp::Clear);
    if !session.snapshot().scrollbar.at_bottom {
        session.scroll(ScrollOp::Bottom);
    }
    session.send_key(ki);

    if let (Some(dir), Some(st)) = (fixture_dir, stream) {
        echo_fixture_key(st, ev, super::dir_name(dir));
    }

    KeyOutcome::ConsumedResetBlink
}

fn paste_clipboard(ws_key: Option<String>, tab_id: Option<String>, tx: &mut Tx) {
    tx.read_clipboard(move |app: &mut App, content, _tx| {
        let text = content.text();
        if text.is_empty() {
            return;
        }
        if let (Some(wk), Some(tid)) = (&ws_key, &tab_id)
            && let Some(tab) = app
                .terminal
                .workspaces
                .get(wk)
                .and_then(|w| w.tabs.iter().find(|t| &t.id == tid))
        {
            tab.session.paste(text);
            if tab.fixture_shell
                && let Some(st) = &tab.stream
            {
                st.feed(text.as_bytes());
            }
            app.terminal.reset_blink(*app.clock);
        }
    });
}

pub fn dispatch_ime(
    session: &TerminalSession,
    stream: Option<&StreamHandle>,
    fixture_shell: bool,
    ime: &Ime,
) -> bool {
    match ime {
        Ime::Commit(text) if !text.is_empty() => {
            session.select(SelectionOp::Clear);
            session.write_bytes(text.as_bytes().to_vec());
            if fixture_shell
                && let Some(st) = stream
            {
                st.feed(text.as_bytes());
            }
            true
        }
        _ => false,
    }
}

fn echo_fixture_key(st: &StreamHandle, ev: &KeyEvent, short_dir: &str) {
    if ev.mods.ctrl || ev.mods.alt || ev.mods.cmd {
        return;
    }
    match &ev.key {
        Key::Enter => st.feed(
            format!(
                "\r\n\x1b[1;34m~/{short_dir}\x1b[0m \x1b[38;5;245mgit:(\x1b[1;36mmain\x1b[0;38;5;245m)\x1b[0m \x1b[1;32m\u{276f}\x1b[0m "
            )
            .as_bytes(),
        ),
        Key::Backspace => st.feed(b"\x08 \x08"),
        Key::Text(s) if !s.is_empty() => st.feed(s.as_bytes()),
        _ => {}
    }
}

fn map_key_event(ev: &KeyEvent) -> Option<KeyInput> {
    let m = ev.mods;
    let (key, text, unshifted) = match &ev.key {
        Key::Enter => (GhosttyKey::Enter, None, Some('\r' as u32)),
        Key::Backspace => (GhosttyKey::Backspace, None, Some(0x7f)),
        Key::Delete => (GhosttyKey::Delete, None, None),
        Key::Tab => (GhosttyKey::Tab, None, Some('\t' as u32)),
        Key::Escape => (GhosttyKey::Escape, None, Some(0x1b)),
        Key::Up => (GhosttyKey::ArrowUp, None, None),
        Key::Down => (GhosttyKey::ArrowDown, None, None),
        Key::Left => (GhosttyKey::ArrowLeft, None, None),
        Key::Right => (GhosttyKey::ArrowRight, None, None),
        Key::Home => (GhosttyKey::Home, None, None),
        Key::End => (GhosttyKey::End, None, None),
        Key::PageUp => (GhosttyKey::PageUp, None, None),
        Key::PageDown => (GhosttyKey::PageDown, None, None),
        Key::Function(n @ 1..=12) => {
            let fk = match n {
                1 => GhosttyKey::F1,
                2 => GhosttyKey::F2,
                3 => GhosttyKey::F3,
                4 => GhosttyKey::F4,
                5 => GhosttyKey::F5,
                6 => GhosttyKey::F6,
                7 => GhosttyKey::F7,
                8 => GhosttyKey::F8,
                9 => GhosttyKey::F9,
                10 => GhosttyKey::F10,
                11 => GhosttyKey::F11,
                _ => GhosttyKey::F12,
            };
            (fk, None, None)
        }
        // Off macOS a Ctrl chord arrives as `Key::Command` (Ctrl is the primary key there); the shell
        // reads it as the control character.
        Key::Command(c) => {
            let (gk, base_cp) = char_to_ghostty_key(*c);
            (gk, None, Some(base_cp))
        }
        Key::Text(s) if !s.is_empty() => {
            let mut chars = s.chars();
            let first = chars.next()?;
            if chars.next().is_none() {
                let (gk, base_cp) = char_to_ghostty_key(first);
                let txt = (!m.ctrl && !m.alt).then(|| s.clone());
                (gk, txt, Some(base_cp))
            } else {
                (GhosttyKey::Unidentified, Some(s.clone()), Some(first as u32))
            }
        }
        _ => return None,
    };

    Some(KeyInput {
        key,
        action: KeyAction::Press,
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        cmd: false,
        text,
        unshifted_codepoint: unshifted,
    })
}

fn char_to_ghostty_key(ch: char) -> (GhosttyKey, u32) {
    let lower = ch.to_ascii_lowercase();
    let gk = match lower {
        'a' => GhosttyKey::A,
        'b' => GhosttyKey::B,
        'c' => GhosttyKey::C,
        'd' => GhosttyKey::D,
        'e' => GhosttyKey::E,
        'f' => GhosttyKey::F,
        'g' => GhosttyKey::G,
        'h' => GhosttyKey::H,
        'i' => GhosttyKey::I,
        'j' => GhosttyKey::J,
        'k' => GhosttyKey::K,
        'l' => GhosttyKey::L,
        'm' => GhosttyKey::M,
        'n' => GhosttyKey::N,
        'o' => GhosttyKey::O,
        'p' => GhosttyKey::P,
        'q' => GhosttyKey::Q,
        'r' => GhosttyKey::R,
        's' => GhosttyKey::S,
        't' => GhosttyKey::T,
        'u' => GhosttyKey::U,
        'v' => GhosttyKey::V,
        'w' => GhosttyKey::W,
        'x' => GhosttyKey::X,
        'y' => GhosttyKey::Y,
        'z' => GhosttyKey::Z,
        '0' | ')' => return (GhosttyKey::Digit0, '0' as u32),
        '1' | '!' => return (GhosttyKey::Digit1, '1' as u32),
        '2' | '@' => return (GhosttyKey::Digit2, '2' as u32),
        '3' | '#' => return (GhosttyKey::Digit3, '3' as u32),
        '4' | '$' => return (GhosttyKey::Digit4, '4' as u32),
        '5' | '%' => return (GhosttyKey::Digit5, '5' as u32),
        '6' | '^' => return (GhosttyKey::Digit6, '6' as u32),
        '7' | '&' => return (GhosttyKey::Digit7, '7' as u32),
        '8' | '*' => return (GhosttyKey::Digit8, '8' as u32),
        '9' | '(' => return (GhosttyKey::Digit9, '9' as u32),
        ' ' => return (GhosttyKey::Space, ' ' as u32),
        '-' | '_' => return (GhosttyKey::Minus, '-' as u32),
        '=' | '+' => return (GhosttyKey::Equal, '=' as u32),
        '[' | '{' => return (GhosttyKey::BracketLeft, '[' as u32),
        ']' | '}' => return (GhosttyKey::BracketRight, ']' as u32),
        '\\' | '|' => return (GhosttyKey::Backslash, '\\' as u32),
        ';' | ':' => return (GhosttyKey::Semicolon, ';' as u32),
        '\'' | '"' => return (GhosttyKey::Quote, '\'' as u32),
        ',' | '<' => return (GhosttyKey::Comma, ',' as u32),
        '.' | '>' => return (GhosttyKey::Period, '.' as u32),
        '/' | '?' => return (GhosttyKey::Slash, '/' as u32),
        '`' | '~' => return (GhosttyKey::Backquote, '`' as u32),
        _ => return (GhosttyKey::Unidentified, ch as u32),
    };
    (gk, lower as u32)
}
