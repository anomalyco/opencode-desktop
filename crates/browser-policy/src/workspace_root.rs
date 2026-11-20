//! Workspace root confinement checks shared by `oc-preview://` and local `file://` routing.
//!
//! - [`is_safe_workspace_root`] is strictly lexical so it works identically on local roots and
//!   cross-OS remote workspace roots (for example, a remote Linux `/remote/work` evaluated on
//!   Windows) without touching disk or network.
//! - [`is_safe_local_workspace_root`] is for callers about to touch a known-local root on this
//!   machine: it refuses UNC/device namespaces, resolves a local symlink/junction alias once up
//!   front without following network targets, and re-checks [`is_safe_workspace_root`] against
//!   the resolved physical directory.

#[cfg(not(target_arch = "wasm32"))]
use crate::local_fs;
#[cfg(not(target_arch = "wasm32"))]
use crate::local_path::is_unc_or_device_path;

fn normalize_root_lex(root: &str) -> Option<String> {
    let trimmed = root.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    let norm_raw = trimmed.replace('\\', "/");
    // Reject Win32 device (`\\.\...`) and NT object (`\??\...`) namespaces.
    if norm_raw.starts_with("//./") || norm_raw.starts_with("/??/") {
        return None;
    }
    // Extended-length prefix `\\?\...` is accepted ONLY for local drive paths (`\\?\C:\...`);
    // `\\?\UNC\...`, `\\?\Volume{...}`, `\\?\GlobalRoot\...`, etc. are rejected.
    let stripped = if let Some(after_ext) = norm_raw.strip_prefix("//?/") {
        let ab = after_ext.as_bytes();
        if ab.len() < 3 || !ab[0].is_ascii_alphabetic() || ab[1] != b':' || ab[2] != b'/' {
            return None;
        }
        after_ext
    } else {
        norm_raw.as_str()
    };
    let stripped = if stripped.len() >= 3
        && stripped.as_bytes()[0] == b'/'
        && stripped.as_bytes()[1].is_ascii_alphabetic()
        && stripped.as_bytes()[2] == b':'
    {
        &stripped[1..]
    } else {
        stripped
    };
    // Reject leading `///...` (which Unix collapses to `/` and would evade UNC/prefix checks).
    if stripped.starts_with("///") {
        return None;
    }
    // Reject drive-relative roots (`C:Windows`) and non-absolute paths (`foo/bar`).
    let sb = stripped.as_bytes();
    let is_drive_abs = sb.len() >= 3 && sb[0].is_ascii_alphabetic() && sb[1] == b':' && sb[2] == b'/';
    let is_drive_bare = sb.len() == 2 && sb[0].is_ascii_alphabetic() && sb[1] == b':';
    let is_slash_abs = stripped.starts_with('/');
    if !(is_drive_abs || is_drive_bare || is_slash_abs) {
        return None;
    }
    let clean = if stripped == "/" { "/" } else { stripped.trim_end_matches('/') };
    if clean.is_empty() {
        return None;
    }
    // Reject internal `//` empty segments (after optional leading UNC `//`) so `C://Windows` or
    // `/etc//passwd` cannot bypass broad-root prefix matching on remote workspaces.
    let body = clean.strip_prefix("//").unwrap_or(clean);
    if body.is_empty() || body.contains("//") {
        return None;
    }
    // Reject any dot-traversal segment so non-existent remote roots like `/foo/..` cannot evade checks.
    if clean.split('/').any(|seg| seg == "." || seg == "..") {
        return None;
    }
    Some(if cfg!(windows) { clean.to_ascii_lowercase() } else { clean.to_owned() })
}

#[cfg(target_arch = "wasm32")]
fn lex_within(target: &str, root_lex: &str) -> bool {
    let cmp_target = if cfg!(windows) { target.to_ascii_lowercase() } else { target.to_owned() };
    if root_lex == "/" {
        return cmp_target.starts_with('/');
    }
    cmp_target == root_lex
        || (cmp_target.len() > root_lex.len()
            && cmp_target.starts_with(root_lex)
            && cmp_target.as_bytes()[root_lex.len()] == b'/')
}

/// Lexical check of whether `root` is a narrow enough workspace directory to confine previews
/// and `file://` URLs (matching `#63002` `disk/preview-scope.ts:25-31`).
///
/// Valid for both local and cross-OS remote workspace paths (e.g. a remote Linux `/remote/work`
/// root evaluated on Windows); touches neither disk nor network. Refuses filesystem roots (`/`),
/// Windows drive roots (`C:\`), UNC/device namespaces, OS system trees, broad container roots,
/// and (when `home` is provided) the user's home directory and its top-level system directories
/// (`Desktop`, `Documents`, `Downloads`, `AppData`, `AppData/Local`, `AppData/Roaming`, `Library`).
pub fn is_safe_workspace_root(root: &str, home: Option<&str>) -> bool {
    let Some(lex) = normalize_root_lex(root) else { return false };
    if is_broad_root_lex(&lex) {
        return false;
    }
    let home_lex = home.and_then(normalize_root_lex);
    if home_lex.as_deref().is_some_and(|h| is_home_broad_lex(&lex, h)) {
        return false;
    }
    true
}

/// Local-machine workspace root check for callers about to touch `root` on this machine's
/// filesystem (e.g. local `file://` confinement and local sidecar `browser_file_roots`).
///
/// Applies [`is_safe_workspace_root`] lexically, refuses UNC/device paths, resolves a local
/// symlink/junction alias once up front via [`local_fs::resolve_local_root`] (never touching SMB
/// or autofs), and re-checks [`is_safe_workspace_root`] on the resolved physical directory so a
/// symlinked workspace works while an alias pointing at a broad/system folder is still refused.
pub fn is_safe_local_workspace_root(root: &str, home: Option<&str>) -> bool {
    if !is_safe_workspace_root(root, home) {
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if is_unc_or_device_path(root) {
            return false;
        }
        match local_fs::resolve_local_root(root) {
            Ok(verified) => {
                if !is_safe_workspace_root(&verified.path, home) {
                    return false;
                }
                if let Some(h_raw) = home
                    && let Ok(canon_home) = local_fs::resolve_local_root(h_raw)
                    && !is_safe_workspace_root(&verified.path, Some(&canon_home.path))
                {
                    return false;
                }
            }
            Err(local_fs::LocalPathError::NotFound) => {
                if !local_fs::is_local_fixed_drive(root) {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

fn is_broad_root_lex(lex: &str) -> bool {
    let lower = lex.to_ascii_lowercase();
    if lower == "/" {
        return true;
    }
    if lower.len() == 2 && lower.as_bytes()[0].is_ascii_alphabetic() && lower.as_bytes()[1] == b':' {
        return true;
    }
    if let Some(rest) = lower.strip_prefix("//") {
        // UNC server (`//server`) or share root (`//server/share`).
        return rest.split('/').filter(|s| !s.is_empty()).count() <= 2;
    }
    // Exact broad container roots (users parent dirs, `/var`, `/private/var`).
    if matches!(lower.as_str(), "/users" | "/home" | "/var" | "/private/var") {
        return true;
    }
    // Windows broad / OS system roots (`C:/Users`, `C:/Windows`, `C:/Program Files*`, `C:/ProgramData`).
    if lower.len() >= 3 && lower.as_bytes()[0].is_ascii_alphabetic() && lower.as_bytes()[1] == b':' && lower.as_bytes()[2] == b'/' {
        let tail = &lower[2..];
        if tail == "/users" {
            return true;
        }
        const WIN_SYSTEM_TREES: &[&str] = &[
            "/windows",
            "/program files",
            "/program files (x86)",
            "/programdata",
            "/system volume information",
        ];
        if WIN_SYSTEM_TREES.iter().any(|sys| tail == *sys || tail.starts_with(&format!("{sys}/"))) {
            return true;
        }
    }
    // Unix / macOS OS system trees (exact or subtree).
    const UNIX_SYSTEM_TREES: &[&str] = &[
        "/etc",
        "/private/etc",
        "/system",
        "/library",
        "/usr",
        "/bin",
        "/sbin",
        "/dev",
        "/proc",
        "/sys",
        "/boot",
        "/root",
    ];
    UNIX_SYSTEM_TREES.iter().any(|sys| lower == *sys || lower.starts_with(&format!("{sys}/")))
}

fn is_home_broad_lex(lex: &str, home_lex: &str) -> bool {
    const SYSTEM_SUFFIXES: &[&str] = &[
        "",
        "/Desktop",
        "/Documents",
        "/Downloads",
        "/AppData",
        "/AppData/Local",
        "/AppData/Roaming",
        "/Library",
    ];
    SYSTEM_SUFFIXES.iter().any(|suffix| {
        let candidate = if suffix.is_empty() {
            home_lex.to_owned()
        } else {
            format!("{home_lex}{suffix}")
        };
        if cfg!(windows) {
            lex.eq_ignore_ascii_case(&candidate)
        } else {
            lex == candidate
        }
    })
}

pub(crate) fn path_within_root(target_lex: &str, root: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        if !is_safe_workspace_root(root, None) {
            return false;
        }
        let Some(root_lex) = normalize_root_lex(root) else { return false };
        lex_within(target_lex, &root_lex)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        is_safe_local_workspace_root(root, None) && local_fs::verify_within_root(target_lex, root, false).is_some()
    }
}
