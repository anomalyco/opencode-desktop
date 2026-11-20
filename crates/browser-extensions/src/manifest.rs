//! Manifest parsing, `__MSG_*__` localization, icon reading, match-pattern algebra,
//! compatibility classification, and version comparison.
//!
//! Mirrors upstream `packages/gui-plugins/src/browser/profile/{extensions,compatibility,extension-api/match-pattern}.ts`
//! with honest Chromium 156 compatibility (MV2 classified `Unsupported(ManifestV2)`; native
//! `webRequest` and content-script `storage.sync` supported natively in CEF 156).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::{
    CompatibilityLevel, ExtensionAccess, ExtensionCompatibility, ExtensionFeature, UnsupportedCause,
};

#[cfg(not(target_arch = "wasm32"))]
const ICON_TARGET: u32 = 40;
#[cfg(not(target_arch = "wasm32"))]
const ICON_MAX_BYTES: u64 = 65_536;
#[cfg(not(target_arch = "wasm32"))]
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

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
pub fn parse_manifest(raw: &str) -> Option<Manifest> {
    let val: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    let obj = val.as_object()?;
    let manifest_version = u32::try_from(obj.get("manifest_version")?.as_u64()?).ok()?;
    let name = obj.get("name")?.as_str()?.to_owned();
    let version = obj.get("version")?.as_str()?.to_owned();
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

    let permissions = string_array(obj.get("permissions"));
    let host_permissions = string_array(obj.get("host_permissions"));
    let optional_permissions = string_array(obj.get("optional_permissions"));
    let optional_host_permissions = string_array(obj.get("optional_host_permissions"));

    let mut content_script_matches = Vec::new();
    if let Some(scripts) = obj.get("content_scripts").and_then(Value::as_array) {
        for script in scripts {
            for m in string_array(script.get("matches")) {
                content_script_matches.push(m);
            }
        }
    }

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

/// Reads and parses `dir/manifest.json` (bounded <= 1 MiB, symlink-free).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_manifest(dir: &Path) -> Option<Manifest> {
    let path = dir.join("manifest.json");
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    parse_manifest(&text)
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
    let Ok(meta) = std::fs::symlink_metadata(&msg_path) else {
        return extension_label(&manifest.name);
    };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_MANIFEST_BYTES {
        return extension_label(&manifest.name);
    }
    let Ok(text) = std::fs::read_to_string(&msg_path) else {
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
    let mut out = String::new();
    for ch in text.chars().take(256) {
        let is_bidi = matches!(
            ch,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        );
        if ch.is_control() || is_bidi || ch == '\u{2028}' || ch == '\u{2029}' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim().chars().take(128).collect()
}

pub fn extension_page_url(id: &str, file: &str) -> Option<String> {
    let clean = file.trim_start_matches(['/', '\\']);
    if clean.is_empty() || clean.contains("..") || clean.contains('\\') {
        return None;
    }
    Some(format!("chrome-extension://{id}/{clean}"))
}

/// Reads the manifest's declared raster icon as a `data:` URL (`<= 64 KiB`, no symlinks, no SVG).
#[cfg(not(target_arch = "wasm32"))]
pub fn extension_icon_data_url(dir: &Path, manifest: &Manifest) -> Option<String> {
    use base64::Engine as _;

    let file = closest_icon(&manifest.icons).or_else(|| match manifest.action_default_icon.as_ref()? {
        IconSpec::Single(s) => Some(s.clone()),
        IconSpec::Sized(map) => closest_icon(map),
    })?;
    let path = within_root(dir, &file)?;
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > ICON_MAX_BYTES {
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        _ => return None,
    };
    let bytes = std::fs::read(&path).ok()?;
    if bytes.len() as u64 > ICON_MAX_BYTES {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

#[cfg(not(target_arch = "wasm32"))]
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
    for c in rel_path.components() {
        if matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)) {
            return None;
        }
        cur.push(c);
        if std::fs::symlink_metadata(&cur).ok()?.file_type().is_symlink() {
            return None;
        }
    }
    let canon_root = std::fs::canonicalize(root).ok()?;
    let canon_target = std::fs::canonicalize(&cur).ok()?;
    canon_target.starts_with(&canon_root).then_some(canon_target)
}

#[cfg(not(target_arch = "wasm32"))]
fn closest_icon(sizes: &HashMap<String, String>) -> Option<String> {
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

/// Compares Chrome dotted numeric versions (`a.b.c.d`); returns `Less` if `a < b`.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let pa: Vec<u64> = a.split('.').map(|s| s.parse::<u64>().unwrap_or(0)).collect();
    let pb: Vec<u64> = b.split('.').map(|s| s.parse::<u64>().unwrap_or(0)).collect();
    let max_len = pa.len().max(pb.len());
    for i in 0..max_len {
        let va = pa.get(i).copied().unwrap_or(0);
        let vb = pb.get(i).copied().unwrap_or(0);
        if va != vb {
            return va.cmp(&vb);
        }
    }
    Ordering::Equal
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
    "management",
    "notifications",
    "offscreen",
    "power",
    "proxy",
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
        "webRequestAuthProvider" => ExtensionFeature::Network,
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
        "privacy" | "fontSettings" | "contentSettings" | "browsingData" => ExtensionFeature::Settings,
        "userScripts" => ExtensionFeature::UserScripts,
        _ => ExtensionFeature::Other,
    })
}

/// Deduplicated, breadth-sorted host patterns from `host_permissions`, `permissions`, and
/// `content_scripts[].matches` (`compatibility.ts:130-142`).
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

fn is_all_sites_pattern(pattern: &str) -> bool {
    if pattern == "<all_urls>" {
        return true;
    }
    let Some((_, rest)) = pattern.split_once("://") else {
        return false;
    };
    rest == "*" || rest.starts_with("*/")
}

/// High-level access badges for the extension list & review dialog (`compatibility.ts:144-161`).
pub fn extension_access(manifest: &Manifest) -> Vec<ExtensionAccess> {
    let sites = review_hosts(manifest);
    let all = sites.iter().any(|p| is_all_sites_pattern(p));
    let mut out = Vec::new();
    if all {
        out.push(ExtensionAccess::AllSites);
    } else if !sites.is_empty() {
        out.push(ExtensionAccess::SomeSites);
    }
    if manifest.permissions.iter().any(|p| p == "cookies") {
        out.push(ExtensionAccess::Cookies);
    }
    if manifest
        .permissions
        .iter()
        .any(|p| matches!(p.as_str(), "tabs" | "webNavigation" | "history"))
    {
        out.push(ExtensionAccess::Activity);
    }
    if manifest.permissions.iter().any(|p| p == "clipboardRead") {
        out.push(ExtensionAccess::Clipboard);
    }
    out
}

/// Whether match pattern `granted` covers every URL covered by `requested` (`match-pattern.ts:43-54`).
pub fn covers_pattern(granted: &str, requested: &str) -> bool {
    let (Some(outer), Some(inner)) = (parse_pattern(granted), parse_pattern(requested)) else {
        return false;
    };
    inner.schemes.iter().all(|s| outer.schemes.contains(s))
        && host_covers(&outer.host, &inner.host)
        && glob_match(&outer.path, &inner.path)
}

struct ParsedPattern {
    schemes: Vec<String>,
    host: String,
    path: String,
}

fn parse_pattern(pattern: &str) -> Option<ParsedPattern> {
    if pattern == "<all_urls>" {
        return Some(ParsedPattern {
            schemes: ["http", "https", "ws", "wss", "file", "ftp"].iter().map(|s| (*s).to_owned()).collect(),
            host: "*".into(),
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
    let host = rest[..slash].to_ascii_lowercase();
    let path = rest[slash..].to_owned();
    Some(ParsedPattern { schemes, host, path })
}

fn host_covers(pattern: &str, host: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(domain) = pattern.strip_prefix("*.") {
        let bare = host.strip_prefix("*.").unwrap_or(host);
        return bare == domain || bare.ends_with(&format!(".{domain}"));
    }
    pattern == host
}

/// Linear-time glob matcher (`*` matches any substring, `match-pattern.ts:72-98`).
pub fn glob_match(pattern: &str, text: &str) -> bool {
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

fn string_array(val: Option<&Value>) -> Vec<String> {
    val.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn string_map(val: Option<&Value>) -> HashMap<String, String> {
    val.and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        .unwrap_or_default()
}

fn parse_icon_spec(val: &Value) -> Option<IconSpec> {
    match val {
        Value::String(s) => Some(IconSpec::Single(s.clone())),
        Value::Object(_) => Some(IconSpec::Sized(string_map(Some(val)))),
        _ => None,
    }
}
