//! App-scheme link validation matching official `origin/dev` `b9a60d9d`
//! (`packages/app/src/runtime/plugin/app-links.ts:22-29, 35-88` and
//! `packages/gui-plugins/src/browser/refusal-notice.ts:3-36, 44-55`), plus two stricter
//! Rune-desktop rules (`ours.md`):
//! 1. Strict RFC 3986 URI character confinement (`is_strict_rfc3986_uri`) refusing `"`, spaces,
//!    `\`, `< > ` ^ { | }`, controls, bidi, non-ASCII, and malformed `%XX` before any OS handoff or
//!    review bar (`mailto:` included — no exemption).
//! 2. Strict RFC 6068 query-key allow-list for `mailto:` (`to`, `cc`, `bcc`, `subject`, `body`
//!    only; anything else downgrades to `Copy` instead of official's two-key `attach`/`attachment`
//!    blocklist).

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
    /// Direct handoff to the system mail client (`mailto:` passing RFC 3986 + RFC 6068 allow-list;
    /// the OS mail client's compose window displays every recipient).
    Mail,
    /// May offer an explicit "Open" action behind the 500 ms guarded review bar (subject to
    /// official's `fits()` full-visibility check and 60 s token TTL).
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

/// Returns `true` ONLY if `value` consists strictly of RFC 3986 URI characters
/// (`unreserved` / `gen-delims` / `sub-delims` / valid `%XX` excluding `%00`–`%1F` and `%7F`),
/// within `1..=2048` bytes.
///
/// Explicitly refuses `"`, spaces/whitespace, `\`, `< > ` ^ { | }`, controls, bidi marks,
/// non-ASCII bytes, and truncated/malformed `%` sequences so no URI handed to `ShellExecuteExW`
/// (`"%1"` substitution), `/usr/bin/open`, or `xdg-open` can ever break out of quotes or inject
/// command-line flags. Applies uniformly to EVERY scheme — including `mailto:`.
pub fn is_strict_rfc3986_uri(value: &str) -> bool {
    if value.is_empty() || value.len() > 2048 {
        return false;
    }
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            if i + 2 >= bytes.len() {
                return false;
            }
            let Some(hi) = (bytes[i + 1] as char).to_digit(16) else {
                return false;
            };
            let Some(lo) = (bytes[i + 2] as char).to_digit(16) else {
                return false;
            };
            let decoded = ((hi << 4) | lo) as u8;
            if decoded <= 0x1f || decoded == 0x7f {
                return false;
            }
            i += 3;
            continue;
        }
        let allowed = b.is_ascii_alphanumeric()
            || matches!(
                b,
                // RFC 3986 unreserved
                b'-' | b'.' | b'_' | b'~'
                // RFC 3986 gen-delims
                | b':' | b'/' | b'?' | b'#' | b'[' | b']' | b'@'
                // RFC 3986 sub-delims
                | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
            );
        if !allowed {
            return false;
        }
        i += 1;
    }
    true
}

/// Validates the original `value` and returns its safe route, or `None` when denied/malformed.
/// Any URI violating strict RFC 3986 character rules (`"`, space, `\`, control, non-ASCII, bad
/// `%XX`) is refused immediately (`None`) before a review bar or OS open can occur.
pub fn classify_app_link(value: &str, source: AppSchemeSource) -> Option<AppSchemeAction> {
    if !is_strict_rfc3986_uri(value) || has_unsafe_chars(value) {
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
        if tail.is_empty() || tail.starts_with("//") {
            return None;
        }
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
    Some(if permitted && value.len() <= 200 {
        AppSchemeAction::Open
    } else {
        AppSchemeAction::Copy
    })
}

/// Explicit allow-list of URLs permitted to reach the OS shell automatically or without the
/// guarded app-link review bar (`open_external`, toasts, sheets, Settings, Linux updater,
/// `System:OpenExternal`): strictly RFC 3986 credential-free `http:`/`https:` or RFC 6068
/// allow-listed `mailto:`. Nothing outside this allow-list may reach the OS unconfirmed.
pub fn is_safe_os_external_url(value: &str) -> bool {
    if !is_strict_rfc3986_uri(value) || has_unsafe_chars(value) {
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
/// 500 ms guarded native review bar: strictly RFC 3986 URIs whose scheme is in `APPS` (+
/// `PAGE_ONLY` when `source == AppSchemeSource::Page`), including RFC 6068 allow-listed `mailto:`.
pub fn is_confirmed_app_scheme_url(value: &str, source: AppSchemeSource) -> bool {
    is_strict_rfc3986_uri(value)
        && matches!(
            classify_app_link(value, source),
            Some(AppSchemeAction::Open | AppSchemeAction::Mail)
        )
}

/// Strict RFC 6068 query-key allow-list (`to`, `cc`, `bcc`, `subject`, `body` only): stricter than
/// official's two-key `attach`/`attachment` blocklist (`app-links.ts:24`), closing proprietary
/// client keys (`attachment-url`, `send-now`, `x-attach`, `body-file`, etc.). Preserves `+`
/// verbatim (no `+` -> space, no key `.trim()`) and fails closed to `Copy` on any other key or
/// invalid percent-decoded key.
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
        let Some(decoded) = percent_decode_strict(raw_key) else {
            return false;
        };
        let key = decoded.to_ascii_lowercase();
        matches!(key.as_str(), "to" | "cc" | "bcc" | "subject" | "body")
    })
}

fn has_unsafe_chars(value: &str) -> bool {
    value.chars().any(|c| {
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
    })
}

fn percent_decode_strict(raw: &str) -> Option<String> {
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
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    if has_unsafe_chars(&decoded) {
        return None;
    }
    Some(decoded)
}
