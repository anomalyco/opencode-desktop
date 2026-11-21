//! Extension manifest parsing (from verified hashed bytes), match patterns, private-URL
//! redaction, bounded `_locales` loading, and magic-byte/dimension-verified icon loading.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use serde_json::Value;

use super::integrity::read_regular_file_bounded;

const ICON_TARGET: u32 = 40;
const ICON_MAX_BYTES: u64 = 65_536;
const ICON_MAX_DIM: u32 = 1024;
const MAX_MESSAGES_BYTES: u64 = 1024 * 1024;
const MAX_LIST_ITEMS: usize = 256;
const MAX_ITEM_CHARS: usize = 256;

/// Single deny-by-default allow-list of Chromium C++ API permissions permitted to reach
/// Chromium's `Extension::permissions_data()` (`compatibility.ts:14-33` + `policy.ts:93-95`,
/// excluding APIs implemented or stubbed by our own Rust host and excluding `proxy`).
///
/// DENY BY DEFAULT: every non-host-pattern permission outside this exact list — including
/// `nativeMessaging`, `downloads*`, `management`, `contentSettings`, `desktopCapture`,
/// `tabCapture`, `pageCapture`, `webAuthenticationProxy`, `history`, `bookmarks`, `sessions`,
/// `privacy`, `fontSettings`, `identity*`, `sidePanel`, `search`, `notifications`, `contextMenus`,
/// `webNavigation`, `browsingData`, `system.{storage,cpu,memory}`, `geolocation`, `userScripts`,
/// `webRequestAuthProvider`, `tts*`, `debugger`, `proxy`, and any future Chromium API — is
/// stripped from Chromium's staged manifest (both required and optional), refused at
/// `permissions.request`, and asserted absent in Chromium's `ExtensionPrefs` post-load.
pub const ALLOWED_CHROMIUM_PERMS: &[&str] = &[
    "activeTab",
    "alarms",
    "background",
    "clipboardRead",
    "clipboardWrite",
    "cookies",
    "declarativeNetRequest",
    "declarativeNetRequestFeedback",
    "declarativeNetRequestWithHostAccess",
    "idle",
    "offscreen",
    "power",
    "scripting",
    "storage",
    "system.display",
    "tabs",
    "unlimitedStorage",
    "webRequest",
    "webRequestBlocking",
];

/// Permissions implemented safely by our own extension host (stripped from Chromium C++ so child
/// frames / native Chromium APIs can never bypass our host, while still reported as supported).
pub const SHIMMED_SUPPORTED_PERMS: &[&str] = &[
    "contextMenus",
    "notifications",
    "webNavigation",
];

/// Returns `true` iff `perm` is on the deny-by-default Chromium C++ API allow-list.
pub fn is_allowed_chromium_perm(perm: &str) -> bool {
    ALLOWED_CHROMIUM_PERMS.contains(&perm.trim())
}

/// Returns `true` iff `perm` is supported in this browser (either native Chromium allow-listed or
/// implemented by our own extension host shim).
pub fn is_supported_permission(perm: &str) -> bool {
    let p = perm.trim();
    ALLOWED_CHROMIUM_PERMS.contains(&p) || SHIMMED_SUPPORTED_PERMS.contains(&p)
}

/// Native Chromium permissions still refused at load because they can bypass the extension host
/// boundary outside our C++ permission-stripping sandbox (`debugger` attaches CDP to any tab;
/// `proxy` MITMs all traffic). Checked on required `permissions` only (and stripped from
/// `optional_permissions` by deny-by-default).
pub const DISALLOWED_REQUIRED_PERMS: &[&str] = &[
    "debugger",
    "proxy",
];

fn is_non_file_host_pattern(perm: &str) -> bool {
    let p = perm.trim();
    if p == "<all_urls>" {
        return true;
    }
    p.contains("://") && !p.to_ascii_lowercase().starts_with("file:")
}

/// Deny-by-default manifest sanitizer for Chromium:
/// - In `"permissions"`: retains ONLY entries in [`ALLOWED_CHROMIUM_PERMS`] or valid non-`file://`
///   host patterns (stripping everything else after `.trim()`).
/// - In `"optional_permissions"` and `"optional_host_permissions"`: strips ALL entries so Chromium
///   C++ can never grant any optional permission or host permission at runtime.
///
/// Returns `(stripped_any, sanitized_bytes)` or `None` on malformed JSON.
pub fn strip_disallowed_perms_from_manifest(bytes: &[u8]) -> Option<(bool, Vec<u8>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut val: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let obj = val.as_object_mut()?;
    let mut changed = false;
    if let Some(arr) = obj.get_mut("permissions").and_then(Value::as_array_mut) {
        let before = arr.len();
        arr.retain(|v| {
            v.as_str()
                .is_some_and(|p| is_allowed_chromium_perm(p) || is_non_file_host_pattern(p))
        });
        if arr.len() != before {
            changed = true;
        }
    }
    for opt_field in ["optional_permissions", "optional_host_permissions"] {
        if let Some(arr) = obj.get_mut(opt_field).and_then(Value::as_array_mut)
            && !arr.is_empty()
        {
            arr.clear();
            changed = true;
        }
    }
    if !changed {
        return Some((false, Vec::new()));
    }
    serde_json::to_vec(&val).ok().map(|b| (true, b))
}

#[derive(Clone, Debug)]
pub struct ActionDefaults {
    pub default_title: Option<String>,
    pub default_popup: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct ExtensionManifest {
    pub name: String,
    pub version: String,
    pub permissions: Vec<String>,
    pub hosts: Vec<String>,
    pub action: Option<ActionDefaults>,
    pub options_page: Option<String>,
    pub icon_data_url: Option<String>,
    pub commands: Vec<CommandInfo>,
}

impl ExtensionManifest {
    /// Parses `ExtensionManifest` strictly from already-hashed `manifest.json` bytes (zero
    /// re-read TOCTOU). Refuses non-MV3 manifests, themes, and disallowed required permissions.
    pub fn from_verified_bytes(dir: &Path, bytes: &[u8]) -> Option<Self> {
        let raw = std::str::from_utf8(bytes).ok()?;
        let val: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
        let obj = val.as_object()?;
        if obj.get("manifest_version")?.as_u64()? != 3 {
            return None;
        }
        if obj.get("theme").is_some_and(|v| !v.is_null()) {
            return None;
        }
        let raw_name = obj.get("name")?.as_str()?;
        let version = extension_label(obj.get("version")?.as_str()?);
        if version.is_empty() {
            return None;
        }
        let default_locale = obj.get("default_locale").and_then(Value::as_str);

        // Fail closed on oversized, non-string, empty, or control/bidi entries — NEVER silently
        // truncate `permissions` or `host_permissions` (`review-rd274.md` Round-2 §3).
        let permissions = checked_string_array(obj.get("permissions"))?;
        if permissions
            .iter()
            .any(|p| DISALLOWED_REQUIRED_PERMS.contains(&p.as_str()))
        {
            return None;
        }

        // MV3 host permissions come strictly from `host_permissions` (never promote MV2-style URL
        // patterns placed inside `"permissions"`, which Chromium 156 ignores; `review-rd274.md` §6).
        let hosts = checked_string_array(obj.get("host_permissions"))?;

        // Parse `_locales/<locale>/messages.json` at most ONCE (bounded <= 1 MiB, symlink-free).
        let locale_messages = load_locale_messages(dir, default_locale);
        let name = resolve_display_name(raw_name, locale_messages.as_ref());

        let action_obj = obj
            .get("action")
            .or_else(|| obj.get("browser_action"))
            .or_else(|| obj.get("page_action"))
            .and_then(Value::as_object);
        let action = action_obj.map(|a| ActionDefaults {
            default_title: a
                .get("default_title")
                .and_then(Value::as_str)
                .map(|s| resolve_display_name(s, locale_messages.as_ref())),
            default_popup: a.get("default_popup").and_then(Value::as_str).map(str::to_owned),
        });

        let options_page = obj
            .get("options_ui")
            .and_then(|v| v.get("page"))
            .and_then(Value::as_str)
            .or_else(|| obj.get("options_page").and_then(Value::as_str))
            .map(str::to_owned);

        let icons: HashMap<String, String> = obj
            .get("icons")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .take(32)
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default();

        let icon_file = closest_icon(&icons).or_else(|| {
            action_obj.and_then(|a| match a.get("default_icon") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Object(m)) => {
                    let map: HashMap<String, String> = m
                        .iter()
                        .take(32)
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                        .collect();
                    closest_icon(&map)
                }
                _ => None,
            })
        });
        let icon_data_url = icon_file.as_deref().and_then(|f| read_icon_data_url(dir, f));

        let commands = obj
            .get("commands")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .take(MAX_LIST_ITEMS)
                    .map(|(k, v)| CommandInfo {
                        name: extension_label(k),
                        description: extension_label(
                            v.get("description").and_then(Value::as_str).unwrap_or(""),
                        ),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some(Self {
            name,
            version,
            permissions,
            hosts,
            action,
            options_page,
            icon_data_url,
            commands,
        })
    }

    pub fn has_permission(&self, perm: &str) -> bool {
        self.permissions.iter().any(|p| p == perm)
    }

    /// Whether this extension registers network filter rules (`filter-startup.ts`).
    pub fn is_filter_extension(&self) -> bool {
        self.has_permission("declarativeNetRequest")
            || self.has_permission("declarativeNetRequestWithHostAccess")
    }

    pub fn reveals_tab(&self, url: &str) -> bool {
        !is_private_url(url)
            && !is_extension_scheme(url)
            && (self.has_permission("tabs") || matches_host(&self.hosts, url))
    }
}

/// Whether `url` uses any extension scheme (`chrome-extension:`, `blob:chrome-extension:`, etc.).
pub fn is_extension_scheme(url: &str) -> bool {
    browser_policy::extension_origin(url)
}

/// Extension-supplied text on a browser-owned surface fits one line and cannot reorder bidi labels.
pub fn extension_label(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars().take(256) {
        if is_disallowed_ui_char(ch) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim().chars().take(128).collect()
}

fn is_disallowed_ui_char(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// Internal preview, artifact, and local file URLs that extensions must never observe.
pub fn is_private_url(url: &str) -> bool {
    let mut cur = url.trim();
    while cur.len() >= 5 && cur.as_bytes()[..5].eq_ignore_ascii_case(b"blob:") {
        cur = cur[5..].trim();
    }
    let lower = cur.to_ascii_lowercase();
    lower.starts_with("file:")
        || lower.starts_with("oc-preview:")
        || lower.starts_with("artifact:")
        || lower.starts_with("workspace:")
}

pub fn extension_page_url(id: &str, file: &str) -> Option<String> {
    if !is_valid_ext_id(id) {
        return None;
    }
    let clean = file.trim_start_matches(['/', '\\']);
    if clean.is_empty()
        || clean.len() > 512
        || clean.contains("..")
        || clean.contains('\\')
        || clean.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    Some(format!("chrome-extension://{id}/{clean}"))
}

pub fn is_valid_ext_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

pub fn extension_id_from_url(url: &str) -> Option<&str> {
    let trimmed = url.trim();
    if trimmed.len() < 19 || !trimmed.as_bytes()[..19].eq_ignore_ascii_case(b"chrome-extension://") {
        return None;
    }
    let rest = &trimmed[19..];
    if rest.contains('\\') {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?;
    is_valid_ext_id(host).then_some(host)
}

pub fn within_root(root: &Path, rel: &str) -> Option<PathBuf> {
    let stripped = rel.trim_start_matches(['/', '\\']);
    if stripped.is_empty() {
        return None;
    }
    let rel_path = Path::new(stripped);
    if rel_path.is_absolute() {
        return None;
    }
    let mut cur = root.to_path_buf();
    let mut pushed = false;
    for c in rel_path.components() {
        match c {
            Component::CurDir => continue,
            Component::Normal(seg) => {
                pushed = true;
                cur.push(seg);
                if std::fs::symlink_metadata(&cur).ok()?.file_type().is_symlink() {
                    return None;
                }
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if !pushed {
        return None;
    }
    let canon_root = std::fs::canonicalize(root).ok()?;
    let canon_target = std::fs::canonicalize(&cur).ok()?;
    (canon_target != canon_root && canon_target.starts_with(&canon_root)).then_some(canon_target)
}

pub fn closest_icon(sizes: &HashMap<String, String>) -> Option<String> {
    let mut list: Vec<(u32, &String)> = sizes
        .iter()
        .filter_map(|(k, v)| Some((k.parse::<u32>().ok()?, v)))
        .collect();
    list.sort_by_key(|(sz, _)| *sz);
    list.iter()
        .find(|(sz, _)| *sz >= ICON_TARGET)
        .or_else(|| list.last())
        .map(|(_, v)| (*v).clone())
}

pub fn read_icon_data_url(root: &Path, file: &str) -> Option<String> {
    let path = within_root(root, file)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let expected_mime = match ext.as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        _ => return None,
    };
    let bytes = read_regular_file_bounded(&path, ICON_MAX_BYTES)?;
    let (sniffed, w, h) = inspect_raster_icon(&bytes)?;
    if sniffed != expected_mime || w == 0 || h == 0 || w > ICON_MAX_DIM || h > ICON_MAX_DIM {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{sniffed};base64,{b64}"))
}

fn inspect_raster_icon(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    if bytes.len() >= 24 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" && &bytes[8..16] == b"\x00\x00\x00\x0dIHDR" {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some(("image/png", w, h));
    }
    if bytes.len() >= 10 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        let w = u32::from(u16::from_le_bytes([bytes[6], bytes[7]]));
        let h = u32::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        return Some(("image/gif", w, h));
    }
    if bytes.len() >= 30 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let chunk = &bytes[12..16];
        if chunk == b"VP8X" {
            let w = 1 + (u32::from(bytes[24]) | (u32::from(bytes[25]) << 8) | (u32::from(bytes[26]) << 16));
            let h = 1 + (u32::from(bytes[27]) | (u32::from(bytes[28]) << 8) | (u32::from(bytes[29]) << 16));
            return Some(("image/webp", w, h));
        }
        if chunk == b"VP8L" && bytes[20] == 0x2f {
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            return Some(("image/webp", (bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1));
        }
        if chunk == b"VP8 " && &bytes[23..26] == b"\x9d\x01\x2a" {
            let w = u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3fff);
            let h = u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3fff);
            return Some(("image/webp", w, h));
        }
        return None;
    }
    if bytes.len() >= 4 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        let mut i = 2usize;
        let mut steps = 0usize;
        while i + 4 <= bytes.len() && steps < 128 {
            steps += 1;
            if bytes[i] != 0xff { return None; }
            while i < bytes.len() && bytes[i] == 0xff { i += 1; }
            if i >= bytes.len() { return None; }
            let marker = bytes[i];
            i += 1;
            if marker == 0xd9 || marker == 0xda { return None; }
            if (0xd0..=0xd7).contains(&marker) || marker == 0x01 { continue; }
            if i + 2 > bytes.len() { return None; }
            let seg_len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
            if seg_len < 2 || i.checked_add(seg_len)? > bytes.len() { return None; }
            if matches!(marker, 0xc0..=0xc2) {
                if seg_len < 7 { return None; }
                let h = u32::from(u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]));
                let w = u32::from(u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]));
                return Some(("image/jpeg", w, h));
            }
            i += seg_len;
        }
        return None;
    }
    if bytes.len() >= 6 && &bytes[0..4] == b"\x00\x00\x01\x00" {
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        if count == 0 || count > 64 || bytes.len() < 6usize.checked_add(count.checked_mul(16)?)? {
            return None;
        }
        let mut max_w = 0u32;
        let mut max_h = 0u32;
        for idx in 0..count {
            let base = 6 + idx * 16;
            max_w = max_w.max(if bytes[base] == 0 { 256 } else { u32::from(bytes[base]) });
            max_h = max_h.max(if bytes[base + 1] == 0 { 256 } else { u32::from(bytes[base + 1]) });
        }
        return Some(("image/x-icon", max_w, max_h));
    }
    None
}

fn load_locale_messages(dir: &Path, default_locale: Option<&str>) -> Option<HashMap<String, String>> {
    let locale = default_locale?;
    if locale.is_empty()
        || locale.len() > 16
        || !locale.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    let msg_path = within_root(dir, &format!("_locales/{locale}/messages.json"))?;
    let bytes = read_regular_file_bounded(&msg_path, MAX_MESSAGES_BYTES)?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let Value::Object(map) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok()? else {
        return None;
    };
    let mut out = HashMap::with_capacity(map.len().min(256));
    for (k, v) in map {
        if let Some(msg) = v.get("message").and_then(Value::as_str) {
            out.insert(k.to_ascii_lowercase(), extension_label(msg));
        }
    }
    Some(out)
}

fn resolve_display_name(raw: &str, messages: Option<&HashMap<String, String>>) -> String {
    let Some(key) = raw.strip_prefix("__MSG_").and_then(|s| s.strip_suffix("__")) else {
        return extension_label(raw);
    };
    if let Some(map) = messages
        && let Some(msg) = map.get(&key.to_ascii_lowercase())
    {
        return msg.clone();
    }
    extension_label(raw)
}

/// Chrome match-pattern matching restricted strictly to web (`http`/`https`) targets
/// (never matches `chrome-extension://` or `file://` URLs; `review-rd274.md` §6).
pub fn matches_host(patterns: &[String], input: &str) -> bool {
    let Some((scheme, rest)) = input.split_once("://") else {
        return false;
    };
    let scheme_lower = scheme.to_ascii_lowercase();
    if !matches!(scheme_lower.as_str(), "http" | "https") {
        return false;
    }
    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    let hostname = if host_port.starts_with('[') {
        host_port.split(']').next().map_or(host_port, |s| s).to_ascii_lowercase()
    } else {
        host_port.split(':').next().unwrap_or(host_port).to_ascii_lowercase()
    };
    let path_no_hash = path_and_query.split('#').next().unwrap_or(path_and_query);

    patterns.iter().any(|pat| {
        if pat.len() > MAX_ITEM_CHARS {
            return false;
        }
        if pat == "<all_urls>" {
            return true;
        }
        let Some((p_scheme, p_rest)) = pat.split_once("://") else {
            return false;
        };
        let scheme_ok = if p_scheme == "*" {
            true
        } else if matches!(p_scheme.to_ascii_lowercase().as_str(), "http" | "https") {
            p_scheme.eq_ignore_ascii_case(&scheme_lower)
        } else {
            false
        };
        if !scheme_ok {
            return false;
        }
        let Some(slash) = p_rest.find('/') else {
            return false;
        };
        let p_host = p_rest[..slash].to_ascii_lowercase();
        let p_path = &p_rest[slash..];
        let target_host = if p_host.contains(':') {
            host_port.to_ascii_lowercase()
        } else {
            hostname.clone()
        };
        host_covers(&p_host, &target_host) && glob_match(p_path, path_no_hash)
    })
}

/// Validates a security-relevant manifest string array without silent truncation (identical
/// fail-closed bounds to `crates/browser-extensions/src/manifest.rs`).
pub(crate) fn checked_string_array(v: Option<&Value>) -> Option<Vec<String>> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Some(Vec::new());
    };
    let arr = v.as_array()?;
    if arr.len() > MAX_LIST_ITEMS {
        return None;
    }
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str()?.trim();
        if s.is_empty()
            || s.len() > MAX_ITEM_CHARS
            || s.chars().any(is_disallowed_ui_char)
        {
            return None;
        }
        out.push(s.to_owned());
    }
    Some(out)
}

/// Extracts the 32-char extension ID from a `chrome-extension://<id>/...` OR wrapped
/// `blob:chrome-extension://<id>/...` CDP target URL (`review-rd274.md` Round-2 §1a).
pub fn extension_id_from_target_url(url: &str) -> Option<&str> {
    let mut cur = url.trim();
    while cur.len() >= 5 && cur.as_bytes()[..5].eq_ignore_ascii_case(b"blob:") {
        cur = cur[5..].trim();
    }
    extension_id_from_url(cur)
}

fn host_covers(pattern: &str, host: &str) -> bool {
    let (pat_host, pat_port) = match pattern.split_once(':') {
        Some((h, p)) if p == "*" || (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) => (h, Some(p)),
        Some(_) => return false,
        None => (pattern, None),
    };
    let (target_host, target_port) = match host.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (host, None),
    };
    let port_ok = match pat_port {
        None | Some("*") => true,
        Some(p) => target_port == Some(p),
    };
    if !port_ok {
        return false;
    }
    if pat_host == "*" {
        return true;
    }
    if let Some(domain) = pat_host.strip_prefix("*.") {
        let bare = target_host.strip_prefix("*.").unwrap_or(target_host);
        return bare == domain || bare.ends_with(&format!(".{domain}"));
    }
    pat_host == target_host
}

/// Linear-backtracking glob match (`*` matches any substring), bounded on length.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern.len() > MAX_ITEM_CHARS || text.len() > 4096 {
        return false;
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut resume) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            resume = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}
