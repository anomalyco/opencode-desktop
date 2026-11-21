//! Shared browser URL policy, workspace `file://` confinement, address resolution, and CDP
//! allow-list enforced identically by `browser-pane` (app) and `opencode-browser-engine` (sidecar).
//!
//! Matches OpenCode Desktop (`packages/gui-plugins/src/browser/policy.ts`, `address.ts`,
//! `pane.ts:103-107,162,334-340`, `chromium.ts:432-446`) plus Rune's stricter physical
//! symlink/junction confinement (matching `#63002`'s `disk/contained-file.ts`) and 22-method
//! DevTools allow-list.
//!
//! - Top-level pages may open `about:blank`, `http:`/`https:` without embedded credentials, and
//!   local `file:` URLs physically inside the session's allowed workspace roots (`file_roots`).
//! - Sub-frames keep Chromium's own rules for `blob:`, `data:`, `about:srcdoc` and web frames,
//!   while any `file:` sub-frame must stay within `file_roots`.
//! - Sub-resources (`img`, `script`, `link`, `fetch`, `XHR`, redirects, downloads) may never read
//!   a `file:` URL outside `file_roots`.

#![cfg_attr(target_arch = "wasm32", forbid(unsafe_code))]
#![cfg_attr(not(target_arch = "wasm32"), deny(unsafe_code))]

mod app_links;
mod download;
mod local_fs;
mod local_path;
pub mod preview;
mod workspace_root;
pub use app_links::{
    AppSchemeAction, AppSchemeSource, classify_app_link, is_confirmed_app_scheme_url,
    is_safe_os_external_url, is_strict_rfc3986_uri,
};
pub use download::{
    MAX_DOWNLOAD_NAME_CHARS, download_url_allowed, is_bidi_control, is_dangerous_download,
    is_dangerous_download_extension, is_dangerous_download_mime, sanitize_download_mime,
    sanitize_download_name,
};
pub use local_fs::{
    LocalPathError, VerifiedLocalPath, is_local_fixed_drive, is_macos_bundle, resolve_local_root,
    reveal_in_file_manager, verify_local_path, verify_within_root,
};
pub use local_path::{
    is_allowed_local_reparse_tag, is_clsid_folder_name, is_macos_bundle_path, is_raw_unc_href,
    is_unc_or_device_path, is_unc_or_device_path_for, is_unix_autofs_path, safe_local_reveal_path,
};
pub use workspace_root::{
    is_safe_local_workspace_root, is_safe_workspace_root, resolve_local_home,
    resolve_safe_local_workspace_root,
};
use workspace_root::path_within_root;

/// Why a navigation was refused (`policy.ts:68-83`, `chromium.ts:442`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `http:` or `https:` URL carrying a username or password (`browser.address.credentials`).
    Credentials,
    /// Non-web or outside-workspace URL when workspace files are allowed (`browser.address.workspace`).
    Workspace,
    /// Non-web URL when no workspace files are allowed (`browser.address.web`).
    Web,
    /// Generic client block (`ERR_BLOCKED_BY_CLIENT`).
    Blocked,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Credentials => "browser.address.credentials",
            Self::Workspace => "browser.address.workspace",
            Self::Web => "browser.address.web",
            Self::Blocked => "ERR_BLOCKED_BY_CLIENT",
        }
    }

    /// Error text returned to agent tools (`policy.ts:55-63`).
    pub fn agent_message(self, has_roots: bool) -> &'static str {
        match self {
            Self::Credentials => "URLs with a user name or password are not supported. Remove them from the URL.",
            Self::Workspace | Self::Web | Self::Blocked => {
                if has_roots {
                    "Only HTTP, HTTPS, about:blank, and file URLs inside the workspace are supported."
                } else {
                    "Only HTTP, HTTPS, and about:blank URLs are supported."
                }
            }
        }
    }
}

/// Origin of an allowed `http:` or `https:` destination without credentials (`policy.ts:3-8`).
pub fn destination_origin(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let (scheme, rest) = split_http_scheme(trimmed)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('\\')
        || authority.contains('@')
        || authority.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let host_port = parse_host_port(authority)?;
    Some(format!("{scheme}://{host_port}"))
}

/// Whether `input` is an `http:` or `https:` URL refused solely for embedded credentials (`policy.ts:80`).
pub fn is_http_with_credentials(input: &str) -> bool {
    let trimmed = input.trim();
    let Some((_, rest)) = split_http_scheme(trimmed) else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((_, host_port)) = authority.rsplit_once('@') else { return false };
    parse_host_port(host_port).is_some()
}

pub fn is_file_scheme(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.len() >= 5 && trimmed.as_bytes()[..5].eq_ignore_ascii_case(b"file:")
}

/// Whether `stem` (the part before the first `.`, ignoring trailing spaces) is a reserved Windows
/// DOS device name, including the superscript port forms `COM¹`–`COM³` and `LPT¹`–`LPT³`.
pub fn is_windows_device_name(stem: &str) -> bool {
    let trimmed = stem.trim_end();
    let mut chars = trimmed.chars();
    let prefix: String = chars.by_ref().take(3).map(|c| c.to_ascii_lowercase()).collect();
    if matches!(prefix.as_str(), "com" | "lpt")
        && let Some(digit) = chars.next()
        && chars.next().is_none()
        && matches!(digit, '0'..='9' | '\u{00B9}' | '\u{00B2}' | '\u{00B3}')
    {
        return true;
    }
    matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "con" | "conin$" | "conout$" | "prn" | "aux" | "nul"
    )
}

/// Decoded, dot-resolved local filesystem path of a `file://` URL with no host (`policy.ts:10-25`,
/// `#63002` `policy.ts:32-61`), or `None` for remote/UNC hosts, encoded separators, or invalid paths.
pub fn local_file_path(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.len() < 8 || !trimmed.as_bytes()[..7].eq_ignore_ascii_case(b"file://") {
        return None;
    }
    let after = &trimmed[7..];
    let raw_path = after.split(['?', '#']).next().unwrap_or(after);
    // Empty authority requires a single leading slash (`file:///...`); reject `file://host/...` and `file:////unc/...`.
    if !raw_path.starts_with('/') || raw_path.starts_with("//") {
        return None;
    }
    if contains_escaped_sep_or_nul(raw_path) {
        return None;
    }
    let decoded = percent_decode(raw_path)?;
    if decoded.chars().any(char::is_control) {
        return None;
    }
    let normalized = decoded.replace('\\', "/");
    if cfg!(windows) {
        resolve_windows_file_path(&normalized)
    } else {
        resolve_unix_file_path(&normalized)
    }
}

/// Whether `input` is a local `file://` URL inside `roots`, both lexically (`..` / prefix boundary)
/// and physically after resolving symlinks/junctions (`policy.ts:32-43`, `#63002` `contained-file.ts`).
pub fn file_url_within(input: &str, roots: &[String]) -> bool {
    if roots.is_empty() {
        return false;
    }
    let Some(target) = local_file_path(input) else { return false };
    roots.iter().any(|root| path_within_root(&target, root))
}

pub fn allowed_destination(input: &str, _roots: &[String]) -> bool {
    // Raw `file://` URLs are banned unconditionally; local workspace files open exclusively via
    // isolated `oc-preview://` grants (`#63002`, `notes/review-rd281.md` §9).
    destination_origin(input).is_some()
}

/// Adds the missing scheme for programmatic / agent URLs (`policy.ts:86-95`), plus Rune's `:port` shorthand.
pub fn with_scheme(input: &str) -> String {
    let value = input.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("about:blank") {
        return "about:blank".into();
    }
    if is_port_shorthand(value) {
        return format!("http://localhost{value}");
    }
    if has_scheme_slash_slash(value) || has_opaque_scheme(value) {
        return value.to_owned();
    }
    let scheme = if is_loopback_authority(value) { "http" } else { "https" };
    format!("{scheme}://{value}")
}

fn refusal_raw(trimmed: &str, roots: &[String]) -> Option<Refusal> {
    if trimmed.eq_ignore_ascii_case("about:blank") || allowed_destination(trimmed, roots) {
        return None;
    }
    if is_http_with_credentials(trimmed) {
        return Some(Refusal::Credentials);
    }
    Some(if is_file_scheme(trimmed) || !roots.is_empty() { Refusal::Workspace } else { Refusal::Web })
}

/// Why the pane refuses `input`, or `None` when allowed (`policy.ts:74-83`).
pub fn refusal(input: &str, roots: &[String]) -> Option<Refusal> {
    refusal_raw(&with_scheme(input), roots)
}

/// Normalizes `input` for agent tools, returning the Official refusal error message on failure (`policy.ts:51-66`).
pub fn normalize_url(input: &str, roots: &[String]) -> Result<String, String> {
    let url = with_scheme(input);
    match refusal(&url, roots) {
        Some(why) => Err(why.agent_message(!roots.is_empty()).to_owned()),
        None => Ok(url),
    }
}

/// Frame navigation & redirect guard (`chromium.ts:432-446`): bans raw `file:` and `oc-preview:`
/// (and nested `view-source:`/`filesystem:`/`blob:` wrappers) unconditionally on all non-preview tabs.
pub fn frame_guard(url: &str, is_main_frame: bool, roots: &[String]) -> Option<Refusal> {
    let trimmed = url.trim();
    if trimmed.eq_ignore_ascii_case("about:blank") {
        return None;
    }
    if preview::has_banned_scheme(trimmed) {
        return Some(if is_file_scheme(trimmed) || !roots.is_empty() { Refusal::Workspace } else { Refusal::Web });
    }
    if is_main_frame {
        return refusal_raw(trimmed, roots);
    }
    None
}

/// Sub-resource & download guard: bans raw `file:` and `oc-preview:` (including nested prefixes)
/// unconditionally on non-preview tabs.
pub fn subresource_allowed(url: &str, _roots: &[String]) -> bool {
    !preview::has_banned_scheme(url)
}

mod agent;
mod cdp;

pub use agent::{check_typed_cdp, extension_origin, extension_page_refusal, matches_url};
pub use cdp::check_cdp;

/// Whether the user's address field treats `input` as a web search rather than an address (`address.ts:23-38`).
pub fn searches(input: &str) -> bool {
    let value = input.trim();
    if value.is_empty() {
        return false;
    }
    if !value.chars().any(char::is_whitespace) && has_opaque_scheme(value) {
        return false;
    }
    if let Some((scheme, rest)) = split_scheme_slash_slash(value) {
        if scheme.eq_ignore_ascii_case("file") {
            let path_part = rest.split(['?', '#']).next().unwrap_or(rest);
            return path_part.is_empty() || path_part.chars().take_while(|&c| c != '/').any(char::is_whitespace);
        }
        if scheme.eq_ignore_ascii_case("workspace")
            || scheme.eq_ignore_ascii_case("artifact")
            || scheme.eq_ignore_ascii_case("oc-preview")
        {
            return rest.is_empty() || rest.chars().any(char::is_whitespace);
        }
        let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
        if authority.is_empty() || authority.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return true;
        }
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
        return parse_host_port(host_port).is_none();
    }
    if value.chars().any(char::is_whitespace) {
        return true;
    }
    if is_port_shorthand(value) {
        return false;
    }
    let host = value.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') {
        return true;
    }
    let Some(parsed) = parse_host_port(host) else { return true };
    let bare_host = if parsed.starts_with('[') {
        parsed.split(']').next().map_or("", |s| &s[1..])
    } else {
        parsed.split(':').next().unwrap_or("")
    };
    let named = bare_host.eq_ignore_ascii_case("localhost")
        || parsed.starts_with('[')
        || (!parsed.starts_with('[') && parsed.contains(':'))
        || (bare_host.contains('.') && !bare_host.starts_with('.') && !bare_host.ends_with('.') && !bare_host.contains(".."));
    !named || destination_origin(&format!("https://{value}")).is_none()
}

/// Resolves user-typed address bar text to a destination or Google search (`address.ts:10-16`).
pub fn resolve_address(input: &str) -> String {
    let value = input.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("about:blank") {
        return "about:blank".into();
    }
    if searches(value) {
        return format!("https://www.google.com/search?q={}", encode_query_component(value));
    }
    with_scheme(value)
}

mod network;
pub use network::{
    is_local_or_loopback_host, is_strict_loopback_ip, loopback_host, validate_proxy_endpoint,
};

fn is_loopback_authority(value: &str) -> bool {
    let authority = value.split(['/', '\\', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let Some(end) = rest.find(']') else { return false };
        &authority[..=end + 1]
    } else {
        authority.split(':').next().unwrap_or("")
    };
    loopback_host(host)
}

fn is_port_shorthand(value: &str) -> bool {
    let Some(rest) = value.strip_prefix(':') else { return false };
    let port = rest.split(['/', '?', '#']).next().unwrap_or("");
    !port.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0)
}

fn split_http_scheme(input: &str) -> Option<(&'static str, &str)> {
    if input.len() >= 7 && input.as_bytes()[..7].eq_ignore_ascii_case(b"http://") {
        return Some(("http", &input[7..]));
    }
    if input.len() >= 8 && input.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        return Some(("https", &input[8..]));
    }
    None
}

fn split_scheme_slash_slash(input: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = input.split_once("://")?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some((scheme, rest))
}

fn has_scheme_slash_slash(input: &str) -> bool {
    split_scheme_slash_slash(input).is_some()
}

fn has_opaque_scheme(input: &str) -> bool {
    let Some((scheme, _)) = input.split_once(':') else { return false };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "about" | "data" | "javascript" | "vbscript" | "blob" | "file" | "mailto" | "tel" | "view-source" | "chrome" | "chrome-extension" | "devtools" | "ws" | "wss" | "oc-preview" | "workspace" | "artifact"
    )
}

fn parse_host_port(authority: &str) -> Option<String> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let ipv6 = &authority[1..end];
        if ipv6.is_empty() || !ipv6.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
            return None;
        }
        let tail = &authority[end + 1..];
        if tail.is_empty() {
            return Some(authority[..=end].to_ascii_lowercase());
        }
        let port_str = tail.strip_prefix(':')?;
        let port: u16 = port_str.parse().ok()?;
        return Some(format!("[{}]:{port}", ipv6.to_ascii_lowercase()));
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, "")) => (h, None),
        Some((h, p)) => (h, Some(p.parse::<u16>().ok()?)),
        None => (authority, None),
    };
    if host.is_empty() || host.contains(':') || host.chars().any(|c| matches!(c, '/' | '\\' | '?' | '#' | '@' | '[' | ']') || c.is_whitespace() || c.is_control()) {
        return None;
    }
    match port {
        Some(p) => Some(format!("{}:{p}", host.to_ascii_lowercase())),
        None => Some(host.to_ascii_lowercase()),
    }
}

fn contains_escaped_sep_or_nul(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'%' {
            let pair = [bytes[i + 1].to_ascii_lowercase(), bytes[i + 2].to_ascii_lowercase()];
            if matches!(&pair, b"2f" | b"5c" | b"00") {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

fn percent_decode(raw: &str) -> Option<String> {
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
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn resolve_windows_file_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'/' || !bytes[1].is_ascii_alphabetic() || bytes[2] != b':' {
        return None;
    }
    if bytes.len() > 3 && bytes[3] != b'/' {
        return None;
    }
    let drive = bytes[1] as char;
    let rest = if bytes.len() > 3 { &path[4..] } else { "" };
    let mut stack: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            stack.pop()?;
            continue;
        }
        if !valid_windows_segment(seg) {
            return None;
        }
        stack.push(seg);
    }
    if stack.is_empty() {
        Some(format!("{drive}:"))
    } else {
        Some(format!("{drive}:/{}", stack.join("/")))
    }
}

fn valid_windows_segment(seg: &str) -> bool {
    if seg.contains(':') || seg.ends_with('.') || seg.ends_with(' ') {
        return false;
    }
    let stem = seg.split('.').next().unwrap_or(seg);
    !is_windows_device_name(stem)
}

fn resolve_unix_file_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let mut stack: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            stack.pop()?;
            continue;
        }
        stack.push(seg);
    }
    Some(format!("/{}", stack.join("/")))
}

fn encode_query_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
