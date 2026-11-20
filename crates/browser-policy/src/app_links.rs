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
        return Some(if mailto_query_allowed(tail) {
            AppSchemeAction::Mail
        } else {
            AppSchemeAction::Copy
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

/// Explicit allow-list of URLs permitted to reach the OS shell automatically or without the
/// guarded app-link review bar (`open_external`, toasts, sheets, Settings, Linux updater,
/// `System:OpenExternal`): strictly credential-free `http:`/`https:` or RFC 6068 allow-listed
/// `mailto:` (`to`, `cc`, `bcc`, `subject`, `body` only). Nothing outside this allow-list may
/// reach the OS unconfirmed.
pub fn is_safe_os_external_url(value: &str) -> bool {
    if value.is_empty() || value.len() > 2048 || value.trim() != value || has_unsafe_chars(value) {
        return false;
    }
    if value.bytes().any(|b| b.is_ascii_whitespace()) {
        return false;
    }
    let is_http = value.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://"))
        || value.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"));
    if is_http {
        return crate::destination_origin(value).is_some();
    }
    if value.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:")) {
        return classify_app_link(value, AppSchemeSource::Conversation) == Some(AppSchemeAction::Mail);
    }
    false
}

/// Explicit allow-list of app-scheme URLs permitted to launch ONLY after the user confirms the
/// 500 ms guarded app-link review bar (single-use review token bound to the exact displayed URL):
/// strictly the 23 schemes in `APPS` (+ `PAGE_ONLY` when `source == AppSchemeSource::Page`)
/// including RFC 6068 allow-listed `mailto:`. Never accepts `http`/`https`/`file`/`ms-*`/denied.
pub fn is_confirmed_app_scheme_url(value: &str, source: AppSchemeSource) -> bool {
    matches!(
        classify_app_link(value, source),
        Some(AppSchemeAction::Open | AppSchemeAction::Mail)
    )
}

fn has_unsafe_chars(value: &str) -> bool {
    if value.chars().any(|c| {
        let n = c as u32;
        n <= 0x1f
            || (0x7f..=0x9f).contains(&n)
            || matches!(
                n,
                0x061c
                    | 0x200b..=0x200f
                    | 0x202a..=0x202e
                    | 0x2060
                    | 0x2066..=0x2069
                    | 0xfeff
            )
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

fn percent_decode_query_key(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            let val = ((hi << 4) | lo) as u8;
            if val <= 0x1f || val == 0x7f {
                return None;
            }
            out.push(val);
            i += 3;
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    if decoded.chars().any(|c| {
        let n = c as u32;
        n <= 0x1f
            || (0x7f..=0x9f).contains(&n)
            || matches!(
                n,
                0x061c
                    | 0x200b..=0x200f
                    | 0x202a..=0x202e
                    | 0x2060
                    | 0x2066..=0x2069
                    | 0xfeff
            )
    }) {
        return None;
    }
    Some(decoded)
}

/// Strict RFC 6068 allow-list of `mailto:` query parameter keys (`to`, `cc`, `bcc`, `subject`,
/// `body`). Rejects every other key — including client-specific attachment/action keys (`attach`,
/// `attachment`, `attachment-url`, `send-now`, `x-attach`, `attach-file`, `body-file`), double-
/// encoded keys, malformed `%XX`, and control/bidi bytes — failing closed to `Copy`.
fn mailto_query_allowed(tail: &str) -> bool {
    let Some((_, query_and_frag)) = tail.split_once('?') else { return true };
    let query = query_and_frag.split('#').next().unwrap_or(query_and_frag);
    if query.is_empty() {
        return true;
    }
    query.split(['&', ';']).all(|pair| {
        if pair.is_empty() {
            return true;
        }
        let raw_key = pair.split('=').next().unwrap_or(pair);
        let Some(decoded) = percent_decode_query_key(raw_key) else {
            return false;
        };
        let key = decoded.trim().to_ascii_lowercase();
        matches!(key.as_str(), "to" | "cc" | "bcc" | "subject" | "body")
    })
}
