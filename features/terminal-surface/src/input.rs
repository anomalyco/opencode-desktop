//! Keyboard and IME translation from Rune events into `terminal::KeyInput`, copy/paste, and scroll.

use opencode_ui::keymap::{Bind, logo_held, takes_from_pane};
use rune::{Ime, Key, KeyEvent, Tx};
use terminal::{
    GhosttyKey, KeyAction, KeyInput, ScrollOp, SelectionOp, StreamHandle, TerminalSession,
};

use crate::{TerminalSurfaceHost, TerminalTab, dir_name};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    Ignored,
    Consumed,
    ConsumedResetBlink,
    /// Open the terminal's find bar.
    OpenFind,
    /// Read the system clipboard and paste into the active terminal.
    Paste,
}

pub fn dispatch_key(
    session: &TerminalSession,
    fixture: Option<(&StreamHandle, &str)>,
    ev: &KeyEvent,
    viewport_rows: u16,
    option_meta: bool,
    tx: &mut Tx,
) -> KeyOutcome {
    // macOS Option as Meta (a setting): the chord is the key it's on, unmodified, with Alt, which
    // the key encoder sends as Esc and the key. A key that types no single character keeps Option.
    let meta = meta_key(ev, option_meta, tx);
    let ev = meta.as_ref().unwrap_or(ev);

    let m = ev.mods;

    // F6 and Shift+F6 leave the terminal for the next or previous pane (the app's F6 cycle), so
    // Tab and every other key stay the shell's and the keyboard is never trapped here; Ctrl+F6
    // gives the shell its F6.
    if ev.key == Key::Function(6) {
        if !m.ctrl {
            return KeyOutcome::Ignored;
        }
        session.send_key(KeyInput::press(GhosttyKey::F6));
        return KeyOutcome::ConsumedResetBlink;
    }
    // ⌘C, and Ctrl+Shift+C everywhere (Desktop `terminal.tsx:477`), copy the selection; ⌘C with
    // none leaves the key to the app. Off macOS Ctrl+C stays the shell's interrupt.
    if Bind::TerminalCopy.matches_tx(ev, tx) {
        if session.selected_text().is_some() {
            copy_selection(session, tx);
            return KeyOutcome::Consumed;
        }
        if logo_held(m) {
            return KeyOutcome::Ignored;
        }
    }

    // A character the layout made with AltGr or Option is text, even where its chord is also an
    // app command's (Ctrl+Alt+[ is MessagePrev, and AltGr+8 types "[" on a German layout).
    if meta.is_none()
        && let Some(text) = layout_text(ev)
    {
        session.select(SelectionOp::Clear);
        if !session.snapshot().scrollbar.at_bottom {
            session.scroll(ScrollOp::Bottom);
        }
        let first = text.chars().next().unwrap_or(' ');
        let (key, base) = char_to_ghostty_key(first);
        session.send_key(KeyInput {
            key,
            action: KeyAction::Press,
            shift: false,
            ctrl: false,
            alt: false,
            cmd: false,
            text: Some(text.to_owned()),
            unshifted_codepoint: Some(base),
        });
        if let Some((st, dir)) = fixture {
            echo_fixture_key(st, &KeyEvent { key: ev.key.clone(), mods: rune::Mods::default() }, dir_name(dir));
        }
        return KeyOutcome::ConsumedResetBlink;
    }

    // Ctrl+Break: Windows' console interrupt. Unlike Ctrl+C it can't be ignored, so it stops a
    // program even under a server whose shells start with Ctrl+C turned off.
    if ev.key == Key::Pause && m.ctrl && !m.alt && !m.shift {
        session.send_break();
        return KeyOutcome::ConsumedResetBlink;
    }

    // Find: Ctrl+F (⌘F); a program on the alternate screen keeps Ctrl+F (vim, less page with it),
    // and Ctrl+Shift+F opens it there.
    let primary = if cfg!(target_os = "macos") { m.cmd } else { m.ctrl };
    if primary && !m.alt && ev.is_command('f') && (m.shift || !session.snapshot().alt_screen) {
        tx.instant();
        return KeyOutcome::OpenFind;
    }

    // Ctrl+Up / Ctrl+Down (⌘ on macOS) jump between the shell's prompts (OSC 133 marks).
    if primary && !m.alt && !m.shift && matches!(ev.key, Key::Up | Key::Down) && !session.snapshot().alt_screen {
        session.scroll(ScrollOp::Prompt(if ev.key == Key::Up { -1 } else { 1 }));
        return KeyOutcome::Consumed;
    }

    if Bind::TerminalPaste.matches_tx(ev, tx) {
        return KeyOutcome::Paste;
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
    if logo_held(m) || ((m.cmd || m.alt) && takes_from_pane(ev, tx)) {
        return KeyOutcome::Ignored;
    }

    // Shift+arrows move the end of a selection by a cell or a line, as in Windows Terminal;
    // with no selection they're the program's (a shell's own selection, an editor's).
    if m.shift && !m.ctrl && !m.alt && !m.cmd && matches!(ev.key, Key::Left | Key::Right | Key::Up | Key::Down) && session.selected_text().is_some() {
        let (dx, dy) = match ev.key {
            Key::Left => (-1, 0),
            Key::Right => (1, 0),
            Key::Up => (0, -1),
            _ => (0, 1),
        };
        session.select(SelectionOp::Nudge { dx, dy });
        return KeyOutcome::Consumed;
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

    if let Some((st, dir)) = fixture {
        echo_fixture_key(st, ev, dir_name(dir));
    }

    KeyOutcome::ConsumedResetBlink
}

/// Copies the selection as text, with an HTML version beside it (colours and styles inline) for
/// documents and mail.
pub fn copy_selection(session: &TerminalSession, tx: &mut Tx) {
    let Some(text) = session.selected_text() else { return };
    match session.selected_html() {
        Some(html) => tx.copy_rich(rune::ClipboardContent::new(text).with_html(html)),
        None => tx.copy(text),
    }
}

/// Thin generic clipboard helper for hosts implementing [`TerminalSurfaceHost::read_clipboard_for_paste`].
pub fn read_clipboard_into_terminal<H: TerminalSurfaceHost + 'static>(ws_key: String, tab_id: String, tx: &mut Tx) {
    tx.read_clipboard(move |app: &mut H, content, tx| {
        let text = content.text();
        if text.is_empty() {
            return;
        }
        crate::paste::paste_into_terminal(app, &ws_key, &tab_id, text.to_owned(), tx);
    });
}

/// What the input method is composing at the terminal's cursor, drawn over the grid until it
/// commits; `cursor` is the byte range of its caret or selection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Preedit {
    pub text: String,
    pub cursor: Option<(usize, usize)>,
}

/// The input method's events for the terminal that has the keyboard: a composition shows at the
/// cursor; committed text goes to the shell, a multi-line commit (dictation, clipboard history)
/// as a paste so a shell that asked for bracketed paste doesn't run it line by line.
pub fn dispatch_ime(tab: &mut TerminalTab, ime: &Ime) -> bool {
    match ime {
        Ime::Preedit(text, cursor, ..) => {
            let next = (!text.is_empty()).then(|| Preedit { text: text.clone(), cursor: *cursor });
            let changed = tab.preedit != next;
            tab.preedit = next;
            changed || !text.is_empty()
        }
        Ime::Commit(text) if !text.is_empty() => {
            tab.preedit = None;
            let session = &tab.session;
            session.select(SelectionOp::Clear);
            if !session.snapshot().scrollbar.at_bottom {
                session.scroll(ScrollOp::Bottom);
            }
            if text.contains(['\n', '\r']) {
                session.paste(text.clone());
            } else {
                session.write_bytes(text.as_bytes().to_vec());
            }
            if let Some(st) = tab.backend.fixture_stream() {
                st.feed(text.as_bytes());
            }
            true
        }
        Ime::Disabled => {
            tab.preedit = None;
            false
        }
        _ => false,
    }
}

/// `ev` as Meta and the key Option is on, when `on` and `ev` is an Option chord on macOS.
fn meta_key(ev: &KeyEvent, on: bool, tx: &Tx) -> Option<KeyEvent> {
    let m = ev.mods;
    if !(on && cfg!(target_os = "macos") && m.alt && !m.ctrl && !m.cmd && matches!(ev.key, Key::Text(_))) {
        return None;
    }
    let base = tx.key_base().filter(|c| !c.is_control())?;
    let base = if m.shift { base.to_uppercase().collect() } else { base.to_string() };
    Some(KeyEvent { key: Key::Text(base), mods: m })
}

/// A character the keyboard layout typed with AltGr (Ctrl+Alt on Windows, where Rune hands a
/// Ctrl chord over as text only when the layout made a character with it) or with macOS's
/// Option: text, the modifiers spent on it. Alt alone elsewhere stays Meta, as in every
/// terminal; Linux's AltGr arrives with no Alt at all.
fn layout_text(ev: &KeyEvent) -> Option<&str> {
    let Key::Text(s) = &ev.key else { return None };
    let m = ev.mods;
    let spent = if cfg!(target_os = "macos") { m.alt && !m.ctrl && !m.cmd } else { m.ctrl && m.alt };
    (spent && !s.is_empty() && !s.chars().any(char::is_control)).then_some(s.as_str())
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
