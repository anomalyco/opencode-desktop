//! Keyboard ownership and modal focus-trap rules for in-pane browser JS dialogs and HTTP auth
//! challenges (`packages/gui-plugins/src/browser/prompt-keys.ts` & `prompts.tsx`).

use std::sync::LazyLock;

use browser_pane::prompts::DialogGate;
use browser_pane::DialogKind;
use rune::{Key, KeyEvent, Keybind, Tx};

use super::{
    AUTH_CANCEL_KEY, AUTH_PASS_KEY, AUTH_SUBMIT_KEY, AUTH_USER_KEY, BROWSER_PROMPTS,
    PROMPT_CANCEL_KEY, PROMPT_INPUT_KEY, PROMPT_OK_KEY, PROMPT_STOP_KEY, clear_auth_fields,
    is_modal_key,
};
use crate::App;

static ESCAPE_HATCH_KEYS: LazyLock<Keybind> =
    LazyLock::new(|| Keybind::new("f6, shift+f6, mod+l"));

fn is_escape_hatch(ev: &KeyEvent) -> bool {
    // F6, Shift+F6, and Mod+L leave the prompt for the address bar without cancelling (`prompt-keys.ts:63-65`).
    ESCAPE_HATCH_KEYS.matches(ev)
}

/// System/window chords that always pass through a tab-modal prompt (`prompt-keys.ts:104-120`)
/// so a page can never trap the user from quitting or closing the window.
fn is_window_chord(ev: &KeyEvent) -> bool {
    let m = ev.mods;
    if cfg!(target_os = "macos") {
        if m.cmd && m.ctrl && !m.alt && matches!(&ev.key, Key::Command(c) if c.eq_ignore_ascii_case(&'f')) {
            return true;
        }
        if !m.cmd || m.ctrl {
            return false;
        }
        let ch = match &ev.key {
            Key::Command(c) => Some(c.to_ascii_lowercase()),
            Key::Text(s) if s == "`" => Some('`'),
            _ => None,
        };
        if !m.shift && !m.alt {
            return matches!(ch, Some('q' | 'h' | 'm' | '`'));
        }
        if m.alt && !m.shift {
            return ch == Some('h');
        }
        return m.shift && !m.alt && matches!(ch, Some('w' | 'n' | '`'));
    }
    if m.alt && !m.ctrl && !m.cmd && !m.shift && ev.key == Key::Function(4) {
        return true;
    }
    m.ctrl
        && m.shift
        && !m.alt
        && !m.cmd
        && matches!(&ev.key, Key::Command(c) if matches!(c.to_ascii_lowercase(), 'w' | 'n'))
}

fn is_text_field_focused(tx: &Tx) -> bool {
    matches!(tx.focused(), Some(PROMPT_INPUT_KEY | AUTH_USER_KEY | AUTH_PASS_KEY))
}

/// Keys that belong to a focused Rune `TextInput` inside the modal (`prompt-keys.ts:72-91`).
fn field_owns_key(ev: &KeyEvent) -> bool {
    let m = ev.mods;
    if matches!(ev.key, Key::Modifier { .. }) {
        return true;
    }
    // AltGr on Windows/Linux (`Ctrl+Alt` + text) types layout characters (`@`, `{`, `€`).
    if !cfg!(target_os = "macos") && m.ctrl && m.alt && !m.cmd && matches!(ev.key, Key::Text(_)) {
        return true;
    }
    let plain = !m.ctrl && !m.cmd && !m.alt;
    if plain {
        return !matches!(ev.key, Key::Function(_) | Key::Enter | Key::Escape | Key::Tab);
    }
    if cfg!(target_os = "macos") {
        if m.alt && !m.cmd && !m.ctrl {
            return true;
        }
        if m.ctrl && !m.cmd && !m.alt {
            return !matches!(ev.key, Key::Tab) && !matches!(&ev.key, Key::Text(s) if s == "`");
        }
    }
    let cmd = if cfg!(target_os = "macos") { m.cmd && !m.ctrl } else { m.ctrl && !m.cmd };
    if cmd && !m.alt {
        if matches!(
            ev.key,
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End | Key::Backspace | Key::Delete
        ) {
            return true;
        }
        if let Key::Command(c) = ev.key {
            let ch = c.to_ascii_lowercase();
            if m.shift {
                return matches!(ch, 'z' | 'v');
            }
            return matches!(ch, 'a' | 'c' | 'x' | 'v' | 'z') || (!cfg!(target_os = "macos") && ch == 'y');
        }
        if !cfg!(target_os = "macos") && !m.shift && ev.key == Key::Insert {
            return true;
        }
    }
    false
}

fn cycle_focus(order: &[&'static str], tx: &mut Tx, reverse: bool) {
    if order.is_empty() {
        return;
    }
    let cur = tx.focused().and_then(|f| order.iter().position(|&k| k == f));
    let next = match (cur, reverse) {
        (Some(i), false) => (i + 1) % order.len(),
        (Some(0), true) => order.len() - 1,
        (Some(i), true) => i - 1,
        (None, false) => 0,
        (None, true) => order.len() - 1,
    };
    tx.focus(order[next], true);
}

/// Handles a key event while focus is inside the JS dialog or HTTP auth card (runs on the card's
/// `.on_key(...)` before Rune's `focus_key`, `TextEdit`, and window `on_key`, and also as the
/// `MODAL`-band fallback while the browser pane holds focus).
pub(super) fn handle_modal_key(s: &mut App, tab_id: u64, ev: &KeyEvent, tx: &mut Tx, from_card: bool) -> bool {
    if is_window_chord(ev) || is_escape_hatch(ev) {
        return false;
    }
    let plain = !ev.mods.ctrl && !ev.mods.alt && !ev.mods.cmd;
    let repeat = tx.key_repeat();
    let focused = tx.focused();
    let text_focused = is_text_field_focused(tx);

    let dialog_info = s.workspace_panes.browser.host.tab(tab_id).and_then(|t| match &t.prompts.dialog {
        DialogGate::Open { dialog, repeat: is_repeat, input, .. } => {
            Some((dialog.id, dialog.kind, *is_repeat, input.is_live()))
        }
        DialogGate::Idle { .. } => None,
    });
    if let Some((dialog_id, kind, is_repeat, armed)) = dialog_info {
        if plain && ev.key == Key::Escape {
            if !repeat {
                let stop = s.kernel.slots.get(&BROWSER_PROMPTS).stop_dialogs;
                s.workspace_panes
                    .browser
                    .host
                    .answer_dialog(tab_id, dialog_id, false, String::new(), stop);
            }
            return true;
        }
        if plain && ev.key == Key::Tab {
            let mut order: Vec<&'static str> = Vec::with_capacity(4);
            if kind == DialogKind::Prompt {
                order.push(PROMPT_INPUT_KEY);
            }
            if is_repeat && kind != DialogKind::Leave {
                order.push(PROMPT_STOP_KEY);
            }
            if kind != DialogKind::Alert {
                order.push(PROMPT_CANCEL_KEY);
            }
            order.push(PROMPT_OK_KEY);
            cycle_focus(&order, tx, ev.mods.shift);
            return true;
        }
        if plain && !ev.mods.shift && ev.key == Key::Enter {
            if repeat {
                return true;
            }
            if focused == Some(PROMPT_STOP_KEY) {
                let ui = s.kernel.slots.get_mut(&BROWSER_PROMPTS);
                ui.stop_dialogs = !ui.stop_dialogs;
                return true;
            }
            if !armed {
                return true;
            }
            let accept = focused != Some(PROMPT_CANCEL_KEY);
            let ui = s.kernel.slots.get(&BROWSER_PROMPTS);
            let val = if accept && kind == DialogKind::Prompt {
                ui.prompt_input.text.clone()
            } else {
                String::new()
            };
            let stop = ui.stop_dialogs;
            s.workspace_panes.browser.host.answer_dialog(tab_id, dialog_id, accept, val, stop);
            return true;
        }
        if plain && matches!(&ev.key, Key::Text(t) if t == " ") && !text_focused {
            if !repeat && focused == Some(PROMPT_STOP_KEY) {
                let ui = s.kernel.slots.get_mut(&BROWSER_PROMPTS);
                ui.stop_dialogs = !ui.stop_dialogs;
                return true;
            }
            if repeat || !armed {
                return true;
            }
            return false;
        }
        if from_card && text_focused && field_owns_key(ev) {
            return false;
        }
        return true;
    }

    let auth_info = s
        .workspace_panes
        .browser
        .host
        .tab(tab_id)
        .and_then(|t| t.prompts.auth.active().map(|a| (a.challenge.id, a.input.is_live())));
    if let Some((auth_id, armed)) = auth_info {
        if plain && ev.key == Key::Escape {
            if !repeat {
                clear_auth_fields(s);
                s.workspace_panes.browser.host.answer_auth(tab_id, auth_id, None);
            }
            return true;
        }
        if plain && ev.key == Key::Tab {
            let order = [AUTH_USER_KEY, AUTH_PASS_KEY, AUTH_CANCEL_KEY, AUTH_SUBMIT_KEY];
            cycle_focus(&order, tx, ev.mods.shift);
            return true;
        }
        if plain && !ev.mods.shift && ev.key == Key::Enter {
            if repeat {
                return true;
            }
            if focused == Some(AUTH_USER_KEY) && s.kernel.slots.get(&BROWSER_PROMPTS).auth_pass.text.is_empty() {
                tx.focus(AUTH_PASS_KEY, true);
                return true;
            }
            if !armed {
                return true;
            }
            if focused == Some(AUTH_CANCEL_KEY) {
                clear_auth_fields(s);
                s.workspace_panes.browser.host.answer_auth(tab_id, auth_id, None);
            } else {
                let ui = s.kernel.slots.get(&BROWSER_PROMPTS);
                let creds = (ui.auth_user.text.clone(), ui.auth_pass.text.clone());
                clear_auth_fields(s);
                s.workspace_panes.browser.host.answer_auth(tab_id, auth_id, Some(creds));
            }
            return true;
        }
        if plain && matches!(&ev.key, Key::Text(t) if t == " ") && !text_focused && (repeat || !armed) {
            return true;
        }
        if from_card && text_focused && field_owns_key(ev) {
            return false;
        }
        return true;
    }
    false
}

/// `MODAL`-band key isolation fallback when the browser pane holds focus behind an open modal.
pub(super) fn modal_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !s.side_open() || s.narrow() {
        return false;
    }
    let Some(tab_id) = s.active_browser_tab_id() else { return false };
    if !s
        .workspace_panes
        .browser
        .host
        .tab(tab_id)
        .is_some_and(|t| t.prompts.has_blocking_modal())
    {
        return false;
    }
    let inside = is_modal_key(tx.focused())
        || tx.focused() == Some("browser-surface")
        || s.focused_browser_pane(false).is_some();
    if !inside {
        return false;
    }
    handle_modal_key(s, tab_id, ev, tx, false)
}
