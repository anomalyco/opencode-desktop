//! Manifest parsing, `__MSG_*__` localization, verified raster icon reading, match-pattern
//! algebra, compatibility classification, and strict Chrome version comparison.
//!
//! Mirrors upstream `packages/gui-plugins/src/browser/profile/{extensions,compatibility,extension-api/match-pattern}.ts`
//! with stricter guarantees for Rune Desktop (Chromium 156):
//! - MV2 classified `Unsupported(ManifestV2)` (Chromium 156 refuses MV2).
//! - Strict Chrome dotted-integer version validation (1–4 segments, ASCII digits only).
//! - Surfaces `proxy`, `declarativeNetRequest*`, `management`, `externally_connectable`, and
//!   `web_accessible_resources` in review grants (where upstream omits them).
//! - Handles port-wildcard all-sites patterns (`https://*:*/*`, `*://*:443/*`).
//! - Sniffs icon magic bytes AND raster header dimensions (`1..=1024`) via bounded non-symlink reads.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use serde_json::Value;

use crate::crx::is_valid_ext_id;
#[cfg(not(target_arch = "wasm32"))]
pub use crate::icon::{extension_icon_data_url, inspect_raster_icon, within_root};
use crate::{
    CompatibilityLevel, ExtensionAccess, ExtensionCompatibility, ExtensionFeature, UnsupportedCause,
};

#[cfg(not(target_arch = "wasm32"))]
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

const MAX_LIST_ITEMS: usize = 256;
const MAX_ITEM_CHARS: usize = 256;

/// Native Chromium permissions still refused at install/load because they bypass the extension
/// host boundary (`debugger` attaches CDP to any tab; `proxy` MITMs all traffic).
/// `nativeMessaging`, `downloads`, and `management` are stripped from Chromium's C++
/// `PermissionsData` at load time (with fail-closed post-load verification) and answered by dead
/// stubs, so extensions declaring them install as `Partial` matching official Electron.
pub const DISALLOWED_REQUIRED_PERMS: &[&str] = &[
    "debugger",
    "proxy",
];

/// Parsed fields of an extension's `manifest.json` (`extensions.ts:15-44`).
#[derive(Clone, Debug)]
pub struct Manifest {
    pub manifest_version: u32,
    pub name: String,
    pub version: String,
    pub default_locale: Option<String>,
    pub key: Option<String>,
    pub is_theme: bool,
    pub icons: HashMap<String, String>,
    pub action_default_popup: Option<String>,
    pub action_default_icon: Option<IconSpec>,
    pub options_page: Option<String>,
    pub permissions: Vec<String>,
    pub host_permissions: Vec<String>,
    pub optional_permissions: Vec<String>,
    pub optional_host_permissions: Vec<String>,
    pub content_script_matches: Vec<String>,
    pub externally_connectable: Vec<String>,
    pub web_accessible_resources: Vec<String>,
    pub has_background: bool,
    pub has_commands: bool,
    pub has_devtools_page: bool,
    pub has_side_panel: bool,
    pub has_omnibox: bool,
    pub has_oauth2: bool,
    pub has_url_overrides: bool,
    pub has_settings_overrides: bool,
}

#[derive(Clone, Debug)]
pub enum IconSpec {
    Single(String),
    Sized(HashMap<String, String>),
}

/// Parses raw `manifest.json` text (stripping UTF-8 BOM if present).
/// Enforces valid Chrome dotted-integer `version` and sanitizes/bounds permission & host lists.
pub fn parse_manifest(raw: &str) -> Option<Manifest> {
    let val: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    let obj = val.as_object()?;
    let manifest_version = u32::try_from(obj.get("manifest_version")?.as_u64()?).ok()?;
    let name = obj.get("name")?.as_str()?.to_owned();
    let version = obj.get("version")?.as_str()?.trim().to_owned();
    // Enforce strict Chrome dotted-integer version (prevents bidi/newline UI spoofing and
    // zero-coercion downgrade bypasses).
    parse_version(&version)?;
    let default_locale = obj.get("default_locale").and_then(Value::as_str).map(str::to_owned);
    let key = obj.get("key").and_then(Value::as_str).map(str::to_owned);
    let is_theme = obj.get("theme").is_some_and(|v| !v.is_null());

    let icons = string_map(obj.get("icons"));
    let action_obj = obj
        .get("action")
        .or_else(|| obj.get("browser_action"))
        .or_else(|| obj.get("page_action"))
        .and_then(Value::as_object);
    let action_default_popup = action_obj
        .and_then(|a| a.get("default_popup"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let action_default_icon = action_obj.and_then(|a| parse_icon_spec(a.get("default_icon")?));

    let options_page = obj
        .get("options_ui")
        .and_then(|v| v.get("page"))
        .and_then(Value::as_str)
        .or_else(|| obj.get("options_page").and_then(Value::as_str))
        .map(str::to_owned);

    let permissions = checked_string_array(obj.get("permissions"))?;
    let host_permissions = checked_string_array(obj.get("host_permissions"))?;
    let optional_permissions = checked_string_array(obj.get("optional_permissions"))?;
    let optional_host_permissions = checked_string_array(obj.get("optional_host_permissions"))?;

    let mut content_script_matches = Vec::new();
    if let Some(cs_val) = obj.get("content_scripts")
        && !cs_val.is_null()
    {
        let scripts = cs_val.as_array()?;
        if scripts.len() > MAX_LIST_ITEMS {
            return None;
        }
        for script in scripts {
            let script_obj = script.as_object()?;
            let matches = checked_string_array(script_obj.get("matches"))?;
            if content_script_matches.len().checked_add(matches.len())? > MAX_LIST_ITEMS {
                return None;
            }
            content_script_matches.extend(matches);
        }
    }

    let externally_connectable = parse_externally_connectable(obj.get("externally_connectable"))?;
    let web_accessible_resources = parse_web_accessible_resources(obj.get("web_accessible_resources"))?;

    Some(Manifest {
        manifest_version,
        name,
        version,
        default_locale,
        key,
        is_theme,
        icons,
        action_default_popup,
        action_default_icon,
        options_page,
        permissions,
        host_permissions,
        optional_permissions,
        optional_host_permissions,
        content_script_matches,
        externally_connectable,
        web_accessible_resources,
        has_background: obj.get("background").is_some_and(|v| !v.is_null()),
        has_commands: obj.get("commands").is_some_and(|v| !v.is_null()),
        has_devtools_page: obj.get("devtools_page").is_some_and(|v| !v.is_null()),
        has_side_panel: obj.get("side_panel").is_some_and(|v| !v.is_null()),
        has_omnibox: obj.get("omnibox").is_some_and(|v| !v.is_null()),
        has_oauth2: obj.get("oauth2").is_some_and(|v| !v.is_null()),
        has_url_overrides: obj.get("chrome_url_overrides").is_some_and(|v| !v.is_null()),
        has_settings_overrides: obj.get("chrome_settings_overrides").is_some_and(|v| !v.is_null()),
    })
}

/// Reads and parses `dir/manifest.json` using a bounded non-symlink handle (`<= 1 MiB`).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_manifest(dir: &Path) -> Option<Manifest> {
    let bytes = crate::integrity::read_regular_file_bounded(&dir.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    let text = std::str::from_utf8(&bytes).ok()?;
    parse_manifest(text)
}

/// Resolves a `__MSG_key__` display name from `_locales/<default_locale>/messages.json`.
#[cfg(not(target_arch = "wasm32"))]
pub fn display_name(dir: &Path, manifest: &Manifest) -> String {
    let Some(key) = manifest.name.strip_prefix("__MSG_").and_then(|s| s.strip_suffix("__")) else {
        return extension_label(&manifest.name);
    };
    let Some(locale) = manifest.default_locale.as_deref() else {
        return extension_label(&manifest.name);
    };
    if locale.is_empty()
        || locale.len() > 16
        || !locale.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return extension_label(&manifest.name);
    }
    let Some(msg_path) = within_root(dir, &format!("_locales/{locale}/messages.json")) else {
        return extension_label(&manifest.name);
    };
    let Some(bytes) = crate::integrity::read_regular_file_bounded(&msg_path, MAX_MANIFEST_BYTES) else {
        return extension_label(&manifest.name);
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return extension_label(&manifest.name);
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
        return extension_label(&manifest.name);
    };
    let key_lower = key.to_ascii_lowercase();
    for (k, v) in map {
        if k.to_ascii_lowercase() == key_lower
            && let Some(msg) = v.get("message").and_then(Value::as_str)
        {
            return extension_label(msg);
        }
    }
    extension_label(&manifest.name)
}

/// Sanitizes extension-supplied labels for browser-owned surfaces (single line, no bidi/control, <= 128 chars).
pub fn extension_label(text: &str) -> String {
    sanitize_label_capped(text, 128)
}

pub(crate) fn sanitize_label_capped(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().take(max_chars.saturating_mul(2)) {
        if is_disallowed_ui_char(ch) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim().chars().take(max_chars).collect()
}

pub(crate) fn is_disallowed_ui_char(ch: char) -> bool {
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

pub fn extension_page_url(id: &str, file: &str) -> Option<String> {
    if !is_valid_ext_id(id) {
        return None;
    }
    let clean = file.trim_start_matches(['/', '\\']);
    if clean.is_empty()
        || clean.len() > 512
        || clean.contains("..")
        || clean.contains('\\')
        || clean.chars().any(|c| is_disallowed_ui_char(c) || c.is_whitespace())
    {
        return None;
    }
    Some(format!("chrome-extension://{id}/{clean}"))
}

/// Strictly parses a Chrome dotted-integer extension version (1–4 segments, ASCII digits only,
/// each `<= 9` digits, total length `<= 32`). Returns `None` on non-numeric or malformed input.
pub fn parse_version(s: &str) -> Option<[u32; 4]> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.len() > 32 {
        return None;
    }
    let mut out = [0u32; 4];
    let mut count = 0usize;
    for part in trimmed.split('.') {
        if count >= 4 || part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out[count] = part.parse::<u32>().ok()?;
        count += 1;
    }
    (count >= 1).then_some(out)
}

/// Compares strict Chrome dotted numeric versions (`a.b.c.d`); returns `None` if either version
/// string is invalid (never silently coerces invalid segments to `0`).
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let pa = parse_version(a)?;
    let pb = parse_version(b)?;
    Some(pa.cmp(&pb))
}

/// Permissions supported natively in CEF 156 or via our extension host shim (`compatibility.ts:10-34`
/// plus native MV3 `webRequest` in CEF 156).
const SUPPORTED_PERMS: &[&str] = &[
    "activeTab",
    "alarms",
    "background",
    "clipboardRead",
    "clipboardWrite",
    "contextMenus",
    "cookies",
    "declarativeNetRequest",
    "declarativeNetRequestFeedback",
    "declarativeNetRequestWithHostAccess",
    "idle",
    "notifications",
    "offscreen",
    "power",
    "scripting",
    "storage",
    "system.display",
    "tabs",
    "unlimitedStorage",
    "webNavigation",
    "webRequest",
    "webRequestBlocking",
];

const STARTUP_FATAL_PERMS: &[&str] = &[
    "readingList",
    "tabGroups",
    "gcm",
    "declarativeContent",
    "debugger",
    "ttsEngine",
];

const FEATURE_ORDER: &[ExtensionFeature] = &[
    ExtensionFeature::Native,
    ExtensionFeature::PageSync,
    ExtensionFeature::Shortcuts,
    ExtensionFeature::SidePanel,
    ExtensionFeature::Devtools,
    ExtensionFeature::Signin,
    ExtensionFeature::Network,
    ExtensionFeature::BrowserData,
    ExtensionFeature::Management,
    ExtensionFeature::Settings,
    ExtensionFeature::AddressBar,
    ExtensionFeature::NewTab,
    ExtensionFeature::Access,
    ExtensionFeature::UserScripts,
    ExtensionFeature::Other,
];

/// Classifies extension compatibility for Rune Desktop's Chromium 156 engine (`compatibility.ts:93-120`).
pub fn extension_compatibility(manifest: &Manifest) -> ExtensionCompatibility {
    let mut declared = HashSet::new();
    for perm in manifest.permissions.iter().chain(manifest.optional_permissions.iter()) {
        if let Some(feat) = permission_feature(perm) {
            declared.insert(feat);
        }
    }
    if manifest.has_commands { declared.insert(ExtensionFeature::Shortcuts); }
    if manifest.has_devtools_page { declared.insert(ExtensionFeature::Devtools); }
    if manifest.has_side_panel { declared.insert(ExtensionFeature::SidePanel); }
    if manifest.has_omnibox { declared.insert(ExtensionFeature::AddressBar); }
    if manifest.has_oauth2 { declared.insert(ExtensionFeature::Signin); }
    if manifest.has_url_overrides { declared.insert(ExtensionFeature::NewTab); }
    if manifest.has_settings_overrides { declared.insert(ExtensionFeature::Settings); }
    if !manifest.optional_permissions.is_empty() || !manifest.optional_host_permissions.is_empty() {
        declared.insert(ExtensionFeature::Access);
    }

    let missing: Vec<ExtensionFeature> = FEATURE_ORDER
        .iter()
        .copied()
        .filter(|f| declared.contains(f))
        .collect();

    if manifest.manifest_version != 3 {
        return ExtensionCompatibility {
            level: CompatibilityLevel::Unsupported,
            cause: Some(UnsupportedCause::ManifestV2),
            missing,
        };
    }
    if manifest
        .permissions
        .iter()
        .any(|p| DISALLOWED_REQUIRED_PERMS.contains(&p.as_str()))
    {
        return ExtensionCompatibility {
            level: CompatibilityLevel::Unsupported,
            cause: Some(UnsupportedCause::DisallowedPermission),
            missing,
        };
    }
    if manifest.has_background && manifest.permissions.iter().any(|p| STARTUP_FATAL_PERMS.contains(&p.as_str())) {
        return ExtensionCompatibility {
            level: CompatibilityLevel::Unsupported,
            cause: Some(UnsupportedCause::Startup),
            missing,
        };
    }
    let level = if missing.is_empty() {
        CompatibilityLevel::Supported
    } else {
        CompatibilityLevel::Partial
    };
    ExtensionCompatibility { level, cause: None, missing }
}

fn permission_feature(perm: &str) -> Option<ExtensionFeature> {
    if SUPPORTED_PERMS.contains(&perm) || perm == "<all_urls>" || perm.contains("://") {
        return None;
    }
    Some(match perm {
        "webRequestAuthProvider" | "proxy" => ExtensionFeature::Network,
        "nativeMessaging" => ExtensionFeature::Native,
        "identity" | "identity.email" => ExtensionFeature::Signin,
        "sidePanel" => ExtensionFeature::SidePanel,
        "history"
        | "bookmarks"
        | "sessions"
        | "topSites"
        | "readingList"
        | "tabGroups"
        | "favicon"
        | "downloads"
        | "downloads.open"
        | "downloads.ui" => ExtensionFeature::BrowserData,
        "management" => ExtensionFeature::Management,
        "privacy" | "fontSettings" | "contentSettings" | "browsingData" => ExtensionFeature::Settings,
        "userScripts" => ExtensionFeature::UserScripts,
        _ => ExtensionFeature::Other,
    })
}

/// Deduplicated, breadth-sorted host patterns from `host_permissions`, `permissions`, and
/// `content_scripts[].matches` (`compatibility.ts:130-142`). Never truncates — `parse_manifest`
/// fails closed if any source list exceeds `MAX_LIST_ITEMS`.
pub fn review_hosts(manifest: &Manifest) -> Vec<String> {
    let mut set = HashSet::new();
    for item in manifest
        .host_permissions
        .iter()
        .chain(manifest.permissions.iter())
        .chain(manifest.content_script_matches.iter())
    {
        if item == "<all_urls>" || item.contains("://") {
            set.insert(item.clone());
        }
    }
    let mut list: Vec<String> = set.into_iter().collect();
    list.sort_by(|a, b| host_breadth(a).cmp(&host_breadth(b)).then_with(|| a.cmp(b)));
    list
}

fn host_breadth(pattern: &str) -> u8 {
    if is_all_sites_pattern(pattern) {
        0
    } else if pattern.contains("://*.") {
        1
    } else {
        2
    }
}

/// Matches `<all_urls>`, `*://*/*`, `https://*/*`, AND port-wildcard equivalents like
/// `https://*:*/*` and `*://*:443/*`.
pub fn is_all_sites_pattern(pattern: &str) -> bool {
    if pattern == "<all_urls>" {
        return true;
    }
    let Some((_, rest)) = pattern.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.split_once(':') {
        Some((h, port)) if port == "*" || (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())) => h,
        Some(_) => return false,
        None => authority,
    };
    host == "*"
}

/// High-level access badges for the extension list & review dialog (`compatibility.ts:144-161`,
/// plus Rune Desktop's additional disclosures for `proxy`, `declarativeNetRequest*`, `management`,
/// `externally_connectable`, and `web_accessible_resources`).
pub fn extension_access(manifest: &Manifest) -> Vec<ExtensionAccess> {
    let sites = review_hosts(manifest);
    let all = sites.iter().any(|p| is_all_sites_pattern(p));
    let mut out = Vec::new();
    if all {
        out.push(ExtensionAccess::AllSites);
    } else if !sites.is_empty() || manifest.permissions.iter().any(|p| p == "scripting") {
        out.push(ExtensionAccess::SomeSites);
    }
    if manifest.permissions.iter().any(|p| p == "cookies") {
        out.push(ExtensionAccess::Cookies);
    }
    if manifest.permissions.iter().any(|p| p == "proxy") {
        out.push(ExtensionAccess::Proxy);
    }
    if manifest.permissions.iter().any(|p| {
        matches!(
            p.as_str(),
            "declarativeNetRequest"
                | "declarativeNetRequestWithHostAccess"
                | "declarativeNetRequestFeedback"
                | "webRequest"
                | "webRequestBlocking"
        )
    }) {
        out.push(ExtensionAccess::BlockRequests);
    }
    if manifest
        .permissions
        .iter()
        .any(|p| matches!(p.as_str(), "tabs" | "webNavigation" | "history"))
    {
        out.push(ExtensionAccess::Activity);
    }
    if manifest
        .permissions
        .iter()
        .any(|p| matches!(p.as_str(), "clipboardRead" | "clipboardWrite"))
    {
        out.push(ExtensionAccess::Clipboard);
    }
    if !manifest.externally_connectable.is_empty() {
        out.push(ExtensionAccess::ExternalConnect);
    }
    if !manifest.web_accessible_resources.is_empty() {
        out.push(ExtensionAccess::WebAccessible);
    }
    out
}

/// Whether match pattern `granted` covers every URL covered by `requested` (`match-pattern.ts:43-54`).
/// Bounded on pattern length (`<= 256` bytes) to guarantee zero worst-case CPU DoS.
pub fn covers_pattern(granted: &str, requested: &str) -> bool {
    if granted.len() > MAX_ITEM_CHARS || requested.len() > MAX_ITEM_CHARS {
        return false;
    }
    let (Some(outer), Some(inner)) = (parse_pattern(granted), parse_pattern(requested)) else {
        return false;
    };
    inner.schemes.iter().all(|s| outer.schemes.contains(s))
        && host_covers(&outer.host, &inner.host)
        && port_covers(outer.port.as_deref(), inner.port.as_deref())
        && glob_match(&outer.path, &inner.path)
}

struct ParsedPattern {
    schemes: Vec<String>,
    host: String,
    port: Option<String>,
    path: String,
}

fn parse_pattern(pattern: &str) -> Option<ParsedPattern> {
    if pattern == "<all_urls>" {
        return Some(ParsedPattern {
            schemes: ["http", "https", "ws", "wss", "file", "ftp"].iter().map(|s| (*s).to_owned()).collect(),
            host: "*".into(),
            port: None,
            path: "/*".into(),
        });
    }
    let (scheme, rest) = pattern.split_once("://")?;
    let schemes = if scheme == "*" {
        ["http", "https", "ws", "wss"].iter().map(|s| (*s).to_owned()).collect()
    } else if matches!(scheme, "http" | "https" | "ws" | "wss" | "file" | "ftp") {
        vec![scheme.to_owned()]
    } else {
        return None;
    };
    let slash = rest.find('/')?;
    let authority = &rest[..slash];
    let (raw_host, port) = if let Some(after_open) = authority.strip_prefix('[') {
        let (inner, trailing) = after_open.split_once(']')?;
        let ip = inner.parse::<std::net::Ipv6Addr>().ok()?;
        let port = if trailing.is_empty() {
            None
        } else {
            let p = trailing.strip_prefix(':')?;
            if p == "*" || (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
                Some(p.to_owned())
            } else {
                return None;
            }
        };
        (format!("[{ip}]"), port)
    } else {
        match authority.split_once(':') {
            Some((h, p)) if p == "*" || (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) => {
                (h.to_ascii_lowercase(), Some(p.to_owned()))
            }
            Some(_) => return None,
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    let host = raw_host;
    let path = rest[slash..].to_owned();
    Some(ParsedPattern { schemes, host, port, path })
}

fn port_covers(outer: Option<&str>, inner: Option<&str>) -> bool {
    match outer {
        None | Some("*") => true,
        Some(p) => inner == Some(p),
    }
}

fn host_covers(outer: &str, inner: &str) -> bool {
    if outer == "*" {
        return true;
    }
    if inner == "*" {
        return false;
    }
    if let Some(suffix) = outer.strip_prefix("*.") {
        let target = inner.strip_prefix("*.").unwrap_or(inner);
        return target == suffix || target.ends_with(&format!(".{suffix}"));
    }
    outer == inner
}

fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern.len() > MAX_ITEM_CHARS || text.len() > MAX_ITEM_CHARS {
        return false;
    }
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star_p = Some(pi);
            star_t = ti;
            pi += 1;
        } else if let Some(sp) = star_p {
            pi = sp + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

fn parse_externally_connectable(v: Option<&Value>) -> Option<Vec<String>> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Some(Vec::new());
    };
    let obj = v.as_object()?;
    let mut out = Vec::new();
    for m in checked_string_array(obj.get("matches"))? {
        if out.len() >= MAX_LIST_ITEMS {
            return None;
        }
        out.push(format!("match:{m}"));
    }
    for id in checked_string_array(obj.get("ids"))? {
        if out.len() >= MAX_LIST_ITEMS {
            return None;
        }
        out.push(format!("id:{id}"));
    }
    if obj.get("accepts_tls_channel_id").and_then(Value::as_bool) == Some(true) {
        if out.len() >= MAX_LIST_ITEMS {
            return None;
        }
        out.push("accepts_tls_channel_id".to_owned());
    }
    out.sort();
    out.dedup();
    Some(out)
}

fn parse_web_accessible_resources(v: Option<&Value>) -> Option<Vec<String>> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Some(Vec::new());
    };
    let arr = v.as_array()?;
    if arr.len() > MAX_LIST_ITEMS {
        return None;
    }
    let mut out = Vec::new();
    for item in arr {
        if let Some(s) = item.as_str() {
            let clean = checked_item_str(s)?;
            if out.len() >= MAX_LIST_ITEMS {
                return None;
            }
            out.push(clean);
        } else {
            let obj = item.as_object()?;
            let resources = checked_string_array(obj.get("resources"))?;
            let matches = checked_string_array(obj.get("matches"))?;
            let ext_ids = checked_string_array(obj.get("extension_ids"))?;
            for r in resources {
                if matches.is_empty() && ext_ids.is_empty() {
                    if out.len() >= MAX_LIST_ITEMS {
                        return None;
                    }
                    out.push(r);
                } else {
                    for m in &matches {
                        if out.len() >= MAX_LIST_ITEMS {
                            return None;
                        }
                        out.push(format!("{r} @ {m}"));
                    }
                    for id in &ext_ids {
                        if out.len() >= MAX_LIST_ITEMS {
                            return None;
                        }
                        out.push(format!("{r} @ ext:{id}"));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    Some(out)
}

/// Strictly validates a security-relevant manifest string array: NEVER silently truncates.
/// Returns `None` (refusing the package) if the field is not an array, exceeds `MAX_LIST_ITEMS`,
/// or contains any non-string, overlong (`> MAX_ITEM_CHARS`), empty, or control/bidi element.
fn checked_string_array(v: Option<&Value>) -> Option<Vec<String>> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Some(Vec::new());
    };
    let arr = v.as_array()?;
    if arr.len() > MAX_LIST_ITEMS {
        return None;
    }
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str()?;
        out.push(checked_item_str(s)?);
    }
    Some(out)
}

fn checked_item_str(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_ITEM_CHARS
        || trimmed.chars().any(is_disallowed_ui_char)
    {
        return None;
    }
    Some(trimmed.to_owned())
}

fn string_map(v: Option<&Value>) -> HashMap<String, String> {
    v.and_then(Value::as_object)
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_icon_spec(v: &Value) -> Option<IconSpec> {
    if let Some(s) = v.as_str() {
        return Some(IconSpec::Single(s.to_owned()));
    }
    let map = string_map(Some(v));
    (!map.is_empty()).then_some(IconSpec::Sized(map))
}
