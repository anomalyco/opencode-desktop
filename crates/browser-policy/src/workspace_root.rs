//! Workspace root confinement checks shared by `oc-preview://` and local `file://` routing.
//!
//! - [`is_safe_workspace_root`] is strictly lexical so it works identically on local roots and
//!   cross-OS remote workspace roots (for example, a remote Linux `/remote/work` evaluated on
//!   Windows) without touching disk or network.
//! - [`is_safe_local_workspace_root`] is for callers about to touch a known-local root on this
//!   machine: it refuses UNC/device namespaces, verifies `root` fail-closed via
//!   [`local_fs::verify_local_path`] (refusing symlinked/junction local roots), and re-checks
//!   [`is_safe_workspace_root`] against the verified physical directory.

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

/// Resolves and caches the local machine's `$HOME` directory once per process (per distinct raw
/// input string), avoiding repeated `home` filesystem walks when checking workspace roots while
/// never touching UNC, mapped/`subst`-to-UNC, or autofs paths.
pub fn resolve_local_home(raw_home: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        normalize_root_lex(raw_home)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static HOME_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(String, Option<String>)>>> =
            std::sync::OnceLock::new();
        let lock = HOME_CACHE.get_or_init(|| std::sync::Mutex::new(None));
        if let Ok(guard) = lock.lock()
            && let Some((cached_raw, cached_val)) = guard.as_ref()
            && cached_raw == raw_home
        {
            return cached_val.clone();
        }
        let resolved = local_fs::resolve_local_root(raw_home).ok().map(|v| v.path);
        if let Ok(mut guard) = lock.lock() {
            *guard = Some((raw_home.to_owned(), resolved.clone()));
        }
        resolved
    }
}

/// Resolves a known-local workspace root directory ONCE on first use, permitting a local symlink
/// or junction alias to another verified local fixed-drive / `MNT_LOCAL` directory (never touching
/// SMB/autofs), applying the broad/system/home/home-ancestor check to BOTH the raw path and the
/// kernel-canonical physical path (case-insensitively on Windows and macOS, including against
/// cached canonical `home`), and returning the **resolved physical root**.
///
/// Callers MUST store and pass the returned physical root — NEVER the raw alias — into every
/// later check (`file_roots`, `safe_roots`, `verify_within_root`, `verify_local_path`), so
/// re-pointing the alias after pinning cannot move the workspace.
pub fn resolve_safe_local_workspace_root(root: &str, home: Option<&str>) -> Option<String> {
    if !is_safe_workspace_root(root, home) {
        return None;
    }
    #[cfg(target_arch = "wasm32")]
    {
        normalize_root_lex(root)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if is_unc_or_device_path(root) {
            return None;
        }
        match local_fs::resolve_local_root(root) {
            Ok(verified) => {
                if !is_safe_workspace_root(&verified.path, home) {
                    return None;
                }
                if let Some(h_raw) = home
                    && let Some(canon_home) = resolve_local_home(h_raw)
                    && !is_safe_workspace_root(&verified.path, Some(&canon_home))
                {
                    return None;
                }
                Some(verified.path)
            }
            Err(_) => None,
        }
    }
}

/// Local-machine workspace root check for callers validating an already-physical root (fail-closed
/// on symlinks/junctions so `path_within_root` and the browser engine never re-resolve aliases).
pub fn is_safe_local_workspace_root(root: &str, home: Option<&str>) -> bool {
    if !is_safe_workspace_root(root, home) {
        return false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if is_unc_or_device_path(root) {
            return false;
        }
        match local_fs::verify_local_path(root) {
            Ok(verified) => {
                if !is_safe_workspace_root(&verified.path, home) {
                    return false;
                }
                if let Some(h_raw) = home
                    && let Some(canon_home) = resolve_local_home(h_raw)
                    && !is_safe_workspace_root(&verified.path, Some(&canon_home))
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
    // Exact broad container roots (users parent dirs, `/var/home` on ostree/Silverblue, `/var`, `/private/var`).
    if matches!(lower.as_str(), "/users" | "/home" | "/var/home" | "/var" | "/private/var") {
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
    // Reject any proper ancestor of `home_lex` (e.g. `/var/home`, `/export/home`, `D:/Profiles`)
    // so a workspace root or resolved alias above `$HOME` cannot expose the entire home directory.
    if home_lex.len() > lex.len()
        && home_lex.as_bytes()[lex.len()] == b'/'
        && if cfg!(any(windows, target_os = "macos")) {
            home_lex.as_bytes()[..lex.len()].eq_ignore_ascii_case(lex.as_bytes())
        } else {
            home_lex.as_bytes()[..lex.len()] == *lex.as_bytes()
        }
    {
        return true;
    }
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
        if cfg!(any(windows, target_os = "macos")) {
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
