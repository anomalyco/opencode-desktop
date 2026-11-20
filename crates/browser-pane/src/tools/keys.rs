//! Agent keyboard chord parser and clipboard/context-menu accelerator guard
//! (`packages/gui-plugins/src/browser/keys.ts:11-20`, `chromium.ts:2510-2560`).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdpKeySpec {
    pub key: String,
    pub code: String,
    pub text: Option<String>,
    pub modifiers: u32,
    pub windows_virtual_key_code: u32,
}

/// Refuses clipboard and native context-menu shortcuts before a CDP key event reaches Blink
/// (`keys.ts:11-20`, matching `browser_policy::check_cdp`).
pub fn agent_key_refusal(key: &str, modifiers: u32, virtual_code: u32) -> Option<&'static str> {
    let shortcuts = (modifiers & 6) != 0; // Control (2) | Meta (4)
    let ctrl = (modifiers & 2) != 0;
    let shift = (modifiers & 8) != 0;
    let is_cvx = matches!(key, "c" | "C" | "v" | "V" | "x" | "X") || matches!(virtual_code, 67 | 86 | 88);
    let is_insert = key.eq_ignore_ascii_case("Insert") || virtual_code == 45;
    let is_del_or_f10 = key.eq_ignore_ascii_case("Delete")
        || virtual_code == 46
        || key.eq_ignore_ascii_case("F10")
        || virtual_code == 121;
    let is_menu = key.eq_ignore_ascii_case("ContextMenu") || virtual_code == 93;
    if (shortcuts && is_cvx) || (is_insert && (shift || ctrl)) || (shift && is_del_or_f10) || is_menu {
        Some("Agent input cannot use clipboard or context-menu shortcuts. Use fill/type to enter text and evaluate to read page text.")
    } else {
        None
    }
}

/// Parses a Playwright/OpenCode key or chord (`"Enter"`, `"ArrowDown"`, `"Control+A"`,
/// `"Meta+Shift+Z"`) into CDP `Input.dispatchKeyEvent` fields and validates against
/// [`agent_key_refusal`].
pub fn parse_key_chord(raw: &str) -> Result<CdpKeySpec, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("browser.press requires a non-empty key.".into());
    }
    let parts: Vec<&str> = trimmed.split('+').collect();
    let mut modifiers = 0u32;
    let main = if parts.len() == 1 {
        parts[0]
    } else {
        for &m in &parts[..parts.len() - 1] {
            match m.to_ascii_lowercase().as_str() {
                "alt" | "option" => modifiers |= 1,
                "control" | "ctrl" => modifiers |= 2,
                "meta" | "cmd" | "command" | "super" => modifiers |= 4,
                "shift" => modifiers |= 8,
                other => return Err(format!("Unsupported modifier '{other}' in key chord '{raw}'.")),
            }
        }
        parts[parts.len() - 1]
    };

    let (key, code, text, vk) = match main {
        "Enter" | "Return" => ("Enter".into(), "Enter".into(), Some("\r".into()), 13),
        "Tab" => ("Tab".into(), "Tab".into(), None, 9),
        "Escape" | "Esc" => ("Escape".into(), "Escape".into(), None, 27),
        "Backspace" => ("Backspace".into(), "Backspace".into(), None, 8),
        "Delete" => ("Delete".into(), "Delete".into(), None, 46),
        "Insert" => ("Insert".into(), "Insert".into(), None, 45),
        "Home" => ("Home".into(), "Home".into(), None, 36),
        "End" => ("End".into(), "End".into(), None, 35),
        "PageUp" => ("PageUp".into(), "PageUp".into(), None, 33),
        "PageDown" => ("PageDown".into(), "PageDown".into(), None, 34),
        "ArrowUp" => ("ArrowUp".into(), "ArrowUp".into(), None, 38),
        "ArrowDown" => ("ArrowDown".into(), "ArrowDown".into(), None, 40),
        "ArrowLeft" => ("ArrowLeft".into(), "ArrowLeft".into(), None, 37),
        "ArrowRight" => ("ArrowRight".into(), "ArrowRight".into(), None, 39),
        "Space" | " " => (" ".into(), "Space".into(), Some(" ".into()), 32),
        "ContextMenu" => ("ContextMenu".into(), "ContextMenu".into(), None, 93),
        f if f.starts_with('F') && f[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) => {
            let n = f[1..].parse::<u32>().unwrap_or(1);
            (f.to_owned(), f.to_owned(), None, 111 + n)
        }
        single if single.chars().count() == 1 => {
            let ch = single.chars().next().unwrap_or(' ');
            let (code, vk) = if ch.is_ascii_alphabetic() {
                let up = ch.to_ascii_uppercase();
                (format!("Key{up}"), up as u32)
            } else if ch.is_ascii_digit() {
                (format!("Digit{ch}"), ch as u32)
            } else {
                (String::new(), 0)
            };
            let text = if (modifiers & 7) == 0 { Some(single.to_owned()) } else { None };
            (single.to_owned(), code, text, vk)
        }
        other => (other.to_owned(), other.to_owned(), None, 0),
    };

    if let Some(msg) = agent_key_refusal(&key, modifiers, vk) {
        return Err(msg.into());
    }
    Ok(CdpKeySpec { key, code, text, modifiers, windows_virtual_key_code: vk })
}
