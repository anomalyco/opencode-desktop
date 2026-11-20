//! App-scheme link validation matching upstream `#63002`
//! (`packages/app/src/runtime/plugin/app-links.ts` and `packages/gui-plugins/src/browser/refusal-notice.ts`).
//!
//! Validates the ORIGINAL string without trimming away unsafe characters or accepting repaired
//! schemes: rejects control bytes, bidi overrides/isolates, percent-encoded controls, credentials,
//! and denied schemes; gates `mailto:` attachment params, `obsidian:` actions, and editor schemes.

/// Where an app-scheme link originated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppSchemeSource {
    /// Chat Markdown or terminal output.
    Conversation,
    /// A user-activated link inside an embedded browser page.
    Page,
}

/// Safe route for a non-web app-scheme URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppSchemeAction {
    /// Direct handoff to the system mail client (`mailto:` without attachment params).
    Mail,
    /// May offer an explicit "Open" action behind the 500 ms guarded review bar.
    Open,
    /// Copy-only (never launches an external OS handler).
    Copy,
}

const APPS: &[&str] = &[
    "mailto", "tel", "sms", "facetime", "slack", "msteams", "zoommtg", "zoomus",
    "discord", "spotify", "figma", "linear", "notion", "obsidian", "tg", "whatsapp",
    "x-github-client", "github-windows", "github-mac",
];

const PAGE_ONLY: &[&str] = &[
    "vscode", "vscode-insiders", "cursor", "zed", "jetbrains",
];

const DENIED: &[&str] = &[
    "http", "https", "file", "javascript", "data", "vbscript", "chrome", "chrome-extension",
    "oc", "oc-preview", "workspace", "about", "blob", "filesystem", "shell",
    "ms-msdt", "search-ms", "ms-officecmd",
];

/// Validates the original `value` and returns its safe route, or `None` when denied/malformed.
pub fn classify_app_link(value: &str, source: AppSchemeSource) -> Option<AppSchemeAction> {
    if value.is_empty() || value.len() > 2048 || value.trim() != value || has_unsafe_chars(value) {
        return None;
    }
    let colon = value.find(':')?;
    let raw_scheme = &value[..colon];
    if raw_scheme.is_empty()
        || !raw_scheme.as_bytes()[0].is_ascii_alphabetic()
        || !raw_scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return None;
    }
    let scheme = raw_scheme.to_ascii_lowercase();
    if DENIED.contains(&scheme.as_str()) || scheme.starts_with("opencode") {
        return None;
    }
    let tail = &value[colon + 1..];
    if let Some(after_slashes) = tail.strip_prefix("//") {
        let authority = after_slashes.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return None;
        }
    }
    if scheme == "mailto" {
        return Some(if mailto_has_attachment(tail) {
            AppSchemeAction::Copy
        } else {
            AppSchemeAction::Mail
        });
    }
    if source == AppSchemeSource::Conversation && scheme == "obsidian" {
        let ok_obsidian = tail == "//open"
            || tail.starts_with("//open?")
            || tail == "//search"
            || tail.starts_with("//search?");
        if !ok_obsidian {
            return Some(AppSchemeAction::Copy);
        }
    }
    let permitted = APPS.contains(&scheme.as_str())
        || (source == AppSchemeSource::Page && PAGE_ONLY.contains(&scheme.as_str()));
    let printable_bounded = value.len() <= 200 && value.bytes().all(|b| (0x21..=0x7e).contains(&b));
    Some(if permitted && printable_bounded {
        AppSchemeAction::Open
    } else {
        AppSchemeAction::Copy
    })
}

fn has_unsafe_chars(value: &str) -> bool {
    if value.chars().any(|c| {
        let n = c as u32;
        n <= 0x1f
            || (0x7f..=0x9f).contains(&n)
            || matches!(n, 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
    }) {
        return true;
    }
    let bytes = value.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'%' {
            let h = bytes[i + 1].to_ascii_lowercase();
            let l = bytes[i + 2].to_ascii_lowercase();
            let is_ctrl = (matches!(h, b'0' | b'1') && l.is_ascii_hexdigit())
                || (h == b'7' && l == b'f');
            if is_ctrl {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

fn mailto_has_attachment(tail: &str) -> bool {
    let Some((_, query_and_frag)) = tail.split_once('?') else { return false };
    let query = query_and_frag.split('#').next().unwrap_or(query_and_frag);
    query.split('&').any(|pair| {
        let key = pair.split('=').next().unwrap_or(pair);
        key.eq_ignore_ascii_case("attach") || key.eq_ignore_ascii_case("attachment")
    })
}
