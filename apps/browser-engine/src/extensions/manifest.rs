//! Extension manifest parsing, match patterns, private-URL redaction, and icon loading.
//! Mirrors upstream `packages/gui-plugins/src/browser/profile/{extensions.ts,extension-api/{manifest,match-pattern,label,resource}.ts}`.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use serde_json::Value;

const ICON_TARGET: u32 = 40;
const ICON_MAX_BYTES: u64 = 65_536;

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
    pub fn read(dir: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
        let val: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
        let obj = val.as_object()?;
        let _manifest_version = obj.get("manifest_version")?.as_u64()? as u32;
        let raw_name = obj.get("name")?.as_str()?;
        let version = obj.get("version")?.as_str()?.to_owned();
        let default_locale = obj.get("default_locale").and_then(Value::as_str);
        let name = resolve_display_name(dir, raw_name, default_locale);

        let permissions: Vec<String> = obj
            .get("permissions")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        let mut hosts: Vec<String> = obj
            .get("host_permissions")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        for p in &permissions {
            if p == "<all_urls>" || p.contains("://") {
                hosts.push(p.clone());
            }
        }

        let action_obj = obj
            .get("action")
            .or_else(|| obj.get("browser_action"))
            .or_else(|| obj.get("page_action"))
            .and_then(Value::as_object);
        let action = action_obj.map(|a| ActionDefaults {
            default_title: a
                .get("default_title")
                .and_then(Value::as_str)
                .map(|s| resolve_display_name(dir, s, default_locale)),
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
                    .map(|(k, v)| CommandInfo {
                        name: k.clone(),
                        description: v
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
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
        !is_private_url(url) && (self.has_permission("tabs") || matches_host(&self.hosts, url))
    }
}

/// Whether `url` uses the `chrome-extension:` scheme (case-insensitive).
pub fn is_extension_scheme(url: &str) -> bool {
    let t = url.trim();
    t.len() >= 17 && t.as_bytes()[..17].eq_ignore_ascii_case(b"chrome-extension:")
}

/// Extension-supplied text on a browser-owned surface fits one line and cannot reorder bidi labels.
pub fn extension_label(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars().take(256) {
        let is_bidi = matches!(
            ch,
            '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        );
        if ch.is_control() || is_bidi || ch == '\u{2028}' || ch == '\u{2029}' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim().chars().take(128).collect()
}

/// Internal preview, artifact, and local file URLs that extensions must never observe.
pub fn is_private_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    let stripped = lower.strip_prefix("blob:").unwrap_or(&lower);
    stripped.starts_with("file:")
        || stripped.starts_with("oc-preview:")
        || stripped.starts_with("artifact:")
        || stripped.starts_with("workspace:")
}

pub fn extension_page_url(id: &str, file: &str) -> Option<String> {
    let clean = file.trim_start_matches(['/', '\\']);
    if clean.is_empty() || clean.contains("..") || clean.contains('\\') {
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
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > ICON_MAX_BYTES {
        return None;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
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

fn resolve_display_name(dir: &Path, raw: &str, default_locale: Option<&str>) -> String {
    let Some(key) = raw.strip_prefix("__MSG_").and_then(|s| s.strip_suffix("__")) else {
        return extension_label(raw);
    };
    let Some(locale) = default_locale else {
        return extension_label(raw);
    };
    if locale.is_empty()
        || locale.len() > 16
        || !locale.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return extension_label(raw);
    }
    let Some(msg_path) = within_root(dir, &format!("_locales/{locale}/messages.json")) else {
        return extension_label(raw);
    };
    let Ok(text) = std::fs::read_to_string(msg_path) else {
        return extension_label(raw);
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
        return extension_label(raw);
    };
    let key_lower = key.to_ascii_lowercase();
    for (k, v) in map {
        if k.to_ascii_lowercase() == key_lower
            && let Some(msg) = v.get("message").and_then(Value::as_str)
        {
            return extension_label(msg);
        }
    }
    extension_label(raw)
}

/// Chrome match-pattern matching (`match-pattern.ts`).
pub fn matches_host(patterns: &[String], input: &str) -> bool {
    let Some((scheme, rest)) = input.split_once("://") else {
        return false;
    };
    let scheme_lower = scheme.to_ascii_lowercase();
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
        if pat == "<all_urls>" {
            return matches!(scheme_lower.as_str(), "http" | "https" | "ws" | "wss" | "file" | "ftp");
        }
        let Some((p_scheme, p_rest)) = pat.split_once("://") else {
            return false;
        };
        let scheme_ok = if p_scheme == "*" {
            matches!(scheme_lower.as_str(), "http" | "https" | "ws" | "wss")
        } else {
            p_scheme.eq_ignore_ascii_case(&scheme_lower)
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

/// Linear-backtracking glob match (`*` matches any substring).
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
