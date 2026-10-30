#![allow(dead_code)]
//! Keyboard and IME translation from Rune events into `terminal::KeyInput`, copy/paste, and scroll.

use rune::{Ime, Key, KeyEvent, Tx};
use terminal::{GhosttyKey, KeyAction, KeyInput, ScrollOp, SelectionOp, TerminalSession};

use crate::App;

/// Handles a key event while the terminal holds focus; returns `true` when consumed.
pub fn handle_key(
    s: &mut App,
    session: &TerminalSession,
    ev: &KeyEvent,
    viewport_rows: u16,
    tx: &mut Tx,
) -> bool {
    let m = ev.mods;

    // Leave app-level command palette (`⌘K`), settings (`⌘,`), session switcher, and panel toggles
    // to the outer shell unless the shortcut belongs to the terminal.
    if ev.is_command('c') && !m.alt && !m.ctrl && !m.shift {
        if let Some(sel) = session.selected_text() {
            tx.copy(sel);
            return true;
        }
        return false;
    }

    // `Ctrl+Shift+C` also copies selection (matches Desktop `terminal.tsx:477`).
    if m.ctrl
        && m.shift
        && !m.cmd
        && !m.alt
        && matches!(&ev.key, Key::Text(t) if t.eq_ignore_ascii_case("c"))
    {
        if let Some(sel) = session.selected_text() {
            tx.copy(sel);
            return true;
        }
    }

    if ev.is_command('v') && !m.alt && !m.ctrl && !m.shift {
        let ws_key = s.active_terminal_workspace_key();
        let tab_id = ws_key
            .as_ref()
            .and_then(|k| s.terminal.workspaces.get(k))
            .and_then(|w| w.active.clone());
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
                app.terminal.reset_blink(app.clock);
            }
        });
        return true;
    }

    if ev.is_command('a') && !m.alt && !m.ctrl && !m.shift {
        session.select(SelectionOp::All);
        return true;
    }

    // `⌘Backspace` kills the line (`\x15`, matching OpenCode Desktop `terminal-key-event.ts`).
    if m.cmd && !m.ctrl && !m.alt && !m.shift && ev.key == Key::Backspace {
        let mut ki = KeyInput::press(GhosttyKey::Backspace);
        ki.cmd = true;
        session.send_key(ki);
        s.terminal.reset_blink(s.clock);
        return true;
    }

    // Let other `⌘` shortcuts (`⌘J`, `⌘W`, `⌘T`, `⌘K`, `⌘1..9`, `⌘+`/`⌘-`, `⌘,`) bubble up.
    if m.cmd {
        return false;
    }

    // `Shift+PageUp` / `Shift+PageDown` / `Shift+Home` / `Shift+End` scroll terminal scrollback
    // when not on the alternate screen.
    if m.shift && !m.ctrl && !m.alt {
        let page = viewport_rows.saturating_sub(2).max(1) as isize;
        match ev.key {
            Key::PageUp => {
                session.scroll(ScrollOp::Delta(-page));
                return true;
            }
            Key::PageDown => {
                session.scroll(ScrollOp::Delta(page));
                return true;
            }
            Key::Home => {
                session.scroll(ScrollOp::Top);
                return true;
            }
            Key::End => {
                session.scroll(ScrollOp::Bottom);
                return true;
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
        s.terminal.reset_blink(s.clock);
        return true;
    }

    let Some(ki) = map_key_event(ev) else {
        return false;
    };
    session.select(SelectionOp::Clear);
    if !session.snapshot().scrollbar.at_bottom {
        session.scroll(ScrollOp::Bottom);
    }
    session.send_key(ki);
    s.terminal.reset_blink(s.clock);
    true
}

#[allow(dead_code)]
pub fn handle_ime(s: &mut App, session: &TerminalSession, ime: &Ime) -> bool {
    match ime {
        Ime::Commit(text) if !text.is_empty() => {
            session.select(SelectionOp::Clear);
            session.write_bytes(text.as_bytes().to_vec());
            s.terminal.reset_blink(s.clock);
            true
        }
        _ => false,
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
